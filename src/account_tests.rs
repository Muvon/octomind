// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Unit tests for the account control-plane client. Network paths run against
//! a local HTTP stub that speaks the kisscore `[err, data]` envelope, so the
//! refresh/retry logic is exercised without any real API. Every test that
//! touches a process-global env var is `#[serial]`; async ones also hold
//! `ENV_LOCK` for the duration of the awaited round trips.

use super::*;
use serial_test::serial;
use std::collections::VecDeque;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::session::chat::test_support::ENV_LOCK;

const DATA_DIR_KEY: &str = "OCTOMIND_DATA_DIR";

/// Snapshot env vars and restore them on drop — a failed assert must not leak
/// a sandboxed data dir or API URL into the next test.
struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
	fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}

impl Drop for EnvGuard {
	fn drop(&mut self) {
		for (key, saved) in &self.0 {
			match saved {
				Some(v) => std::env::set_var(key, v),
				None => std::env::remove_var(key),
			}
		}
	}
}

/// A fresh per-test data dir under the system temp dir.
fn sandbox(tag: &str) -> PathBuf {
	let dir = std::env::temp_dir().join(format!("octomind-acct-{tag}-{}", std::process::id()));
	std::fs::create_dir_all(&dir).expect("create sandbox data dir");
	dir
}

/// A kisscore success envelope: `[null, data]`.
fn env_ok(data: serde_json::Value) -> String {
	serde_json::json!([null, data]).to_string()
}

/// A kisscore error envelope: `["code", null]`.
fn env_err(code: &str) -> String {
	serde_json::json!([code, null]).to_string()
}

fn account_json() -> serde_json::Value {
	serde_json::json!({"email": "dev@example.com", "plan": "pro"})
}

fn usage_json() -> serde_json::Value {
	serde_json::json!({
		// pricing-v2 sends ONE window; the legacy trio rides along so this
		// fixture also proves the pre-v2 fallback still deserializes.
		"window": {"label": "billing period", "spent_usd": 3.0, "reserved_usd": 0.5, "allowance_usd": 20.0, "resets_at": "m"},
		"always_free_models": ["glm-5.3-flash"],
		"month": {"spent_usd": 3.0, "cap_usd": 100.0, "resets_at": "m"},
		"balance_usd": 9.0,
		"storage_gb": 1.0,
		"storage_quota_gb": 2.0,
		"network": {"used_gb": 0.5, "included_gb": 1.0}
	})
}

/// One-shot-per-connection HTTP stub serving the given raw bodies in order.
/// Returns the base URL to point `OCTOMIND_API_URL` at.
async fn spawn_api(bodies: Vec<String>) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind stub");
	let addr = listener.local_addr().expect("stub addr");
	let queue = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::from(bodies)));

	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			let queue = queue.clone();
			tokio::spawn(async move {
				let mut buf = Vec::new();
				let mut tmp = [0u8; 8192];
				let header_end = loop {
					let n = sock.read(&mut tmp).await.unwrap_or(0);
					if n == 0 {
						return;
					}
					buf.extend_from_slice(&tmp[..n]);
					if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
						break pos + 4;
					}
				};
				let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
				let content_length: usize = headers
					.lines()
					.find_map(|l| l.strip_prefix("content-length:"))
					.and_then(|v| v.trim().parse().ok())
					.unwrap_or(0);
				while buf.len() < header_end + content_length {
					let n = sock.read(&mut tmp).await.unwrap_or(0);
					if n == 0 {
						break;
					}
					buf.extend_from_slice(&tmp[..n]);
				}
				let body = queue
					.lock()
					.expect("stub queue")
					.pop_front()
					.unwrap_or_else(|| "[null, null]".to_string());
				let response = format!(
					"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
					body.len(),
					body
				);
				let _ = sock.write_all(response.as_bytes()).await;
				let _ = sock.shutdown().await;
			});
		}
	});

	format!("http://{addr}")
}

// ── pure / filesystem-only paths ────────────────────────────────────────────

#[test]
fn max_login_poll_is_fifteen_minutes() {
	assert_eq!(MAX_LOGIN_POLL, Duration::from_secs(15 * 60));
}

#[test]
#[serial]
fn api_url_defaults_without_env_and_trims_trailing_slashes() {
	let _env = EnvGuard::new(&[API_URL_ENV]);
	std::env::remove_var(API_URL_ENV);
	assert_eq!(api_url(), DEFAULT_API_URL);

	std::env::set_var(API_URL_ENV, "http://127.0.0.1:9999///");
	assert_eq!(api_url(), "http://127.0.0.1:9999");
}

#[test]
#[serial]
fn session_path_lives_in_the_config_dir() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("paths");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let path = session_path().expect("session path");
	assert_eq!(path, dir.join("config").join("auth.json"));
}

#[test]
#[serial]
fn machine_id_is_created_once_and_stays_stable() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("machine-id");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let id = machine_id().expect("first machine id");
	assert_eq!(id.len(), 12, "machine id is a 12-char uuid slice: {id}");
	assert_eq!(
		machine_id().expect("second machine id"),
		id,
		"stable on disk"
	);
	assert!(dir.join("config").join("machine-id").exists());
}

#[test]
#[serial]
fn machine_id_regenerates_when_the_stored_file_is_blank() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("machine-id-blank");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let config = dir.join("config");
	std::fs::create_dir_all(&config).expect("config dir");
	std::fs::write(config.join("machine-id"), "   \n").expect("seed blank file");

	let id = machine_id().expect("regenerated machine id");
	assert_eq!(id.len(), 12);
	assert_eq!(
		std::fs::read_to_string(config.join("machine-id")).expect("persisted"),
		id
	);
}

#[test]
#[serial]
fn session_is_none_for_missing_garbage_empty_jwt_or_other_api() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV]);
	let dir = sandbox("session-none");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(API_URL_ENV);

	assert!(session().is_none(), "no auth.json yet");

	std::fs::write(session_path().expect("path"), "not json").expect("garbage");
	assert!(session().is_none(), "unparseable auth.json");

	save_session(&Session {
		jwt: String::new(),
		refresh_token: "r".into(),
		api_url: String::new(),
	})
	.expect("save empty-jwt session");
	assert!(session().is_none(), "empty jwt is not a session");

	save_session(&Session {
		jwt: "j".into(),
		refresh_token: "r".into(),
		api_url: "https://elsewhere.example".into(),
	})
	.expect("save mismatched session");
	assert!(session().is_none(), "session minted for another api_url");

	// An empty api_url (serde default, older sessions) matches any host.
	save_session(&Session {
		jwt: "j".into(),
		refresh_token: "r".into(),
		api_url: String::new(),
	})
	.expect("save legacy session");
	assert_eq!(session().expect("legacy session counts").jwt, "j");
}

#[test]
#[serial]
fn save_session_roundtrips_and_restricts_permissions() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV]);
	let dir = sandbox("save-session");
	std::env::set_var(DATA_DIR_KEY, &dir);
	// session() only accepts a session minted for the CURRENT api_url.
	std::env::set_var(API_URL_ENV, "https://api.example");

	let path = save_session(&Session {
		jwt: "jwt-1".into(),
		refresh_token: "refresh-1".into(),
		api_url: "https://api.example".into(),
	})
	.expect("save session");

	let s = session().expect("read back");
	assert_eq!(s.jwt, "jwt-1");
	assert_eq!(s.refresh_token, "refresh-1");
	assert_eq!(s.api_url, "https://api.example");

	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		let mode = std::fs::metadata(&path)
			.expect("metadata")
			.permissions()
			.mode();
		assert_eq!(mode & 0o777, 0o600, "auth.json is owner-only");
	}
}

#[test]
#[serial]
fn panel_url_passthrough_without_env_and_repoints_with_it() {
	let _env = EnvGuard::new(&[PANEL_URL_ENV]);
	let url = "https://octomind.run/app/login/cli?code=AB12-CD34";

	std::env::remove_var(PANEL_URL_ENV);
	assert_eq!(panel_url(url), url);

	std::env::set_var(PANEL_URL_ENV, "http://localhost:5199/");
	assert_eq!(
		panel_url(url),
		"http://localhost:5199/app/login/cli?code=AB12-CD34"
	);
}

// ── network paths against the local stub ────────────────────────────────────

#[tokio::test]
#[serial]
async fn post_public_parses_both_envelope_shapes() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV]);
	let dir = sandbox("post-public");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let url = spawn_api(vec![env_ok(serde_json::json!({"x": 1})), env_err("nope")]).await;
	std::env::set_var(API_URL_ENV, &url);

	let ok: std::result::Result<serde_json::Value, String> =
		post_public("/anything", serde_json::json!({}))
			.await
			.expect("call");
	assert_eq!(ok.expect("data"), serde_json::json!({"x": 1}));

	let err: std::result::Result<serde_json::Value, String> =
		post_public("/anything", serde_json::json!({}))
			.await
			.expect("call");
	assert_eq!(err.expect_err("error code"), "nope");
}

#[tokio::test]
#[serial]
async fn envelope_rejects_empty_and_non_json_bodies() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV]);
	let dir = sandbox("envelope-bad");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let url = spawn_api(vec!["[null, null]".to_string(), "hello there".to_string()]).await;
	std::env::set_var(API_URL_ENV, &url);

	let e = post_public::<serde_json::Value>("/x", serde_json::json!({}))
		.await
		.expect_err("empty envelope bails");
	assert!(e.to_string().contains("empty response"), "{e}");

	let e = post_public::<serde_json::Value>("/x", serde_json::json!({}))
		.await
		.expect_err("non-json body bails");
	assert!(e.to_string().contains("unexpected response"), "{e}");
}

#[tokio::test]
#[serial]
async fn get_returns_none_when_not_signed_in() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, HUB_KEY_ENV]);
	let dir = sandbox("get-anon");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(HUB_KEY_ENV);

	assert!(get::<Account>("/auth/me").await.expect("call").is_none());
}

#[tokio::test]
#[serial]
async fn get_returns_the_payload_when_signed_in() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV, HUB_KEY_ENV]);
	let dir = sandbox("get-ok");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(HUB_KEY_ENV);
	let url = spawn_api(vec![env_ok(account_json())]).await;
	std::env::set_var(API_URL_ENV, &url);
	save_session(&Session {
		jwt: "j1".into(),
		refresh_token: "r1".into(),
		api_url: url.clone(),
	})
	.expect("save session");

	let account = get::<Account>("/auth/me")
		.await
		.expect("call")
		.expect("signed in");
	assert_eq!(account.email, "dev@example.com");
	assert_eq!(account.plan, "pro");
}

#[tokio::test]
#[serial]
async fn get_refreshes_once_on_unauthorized_and_retries() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV, HUB_KEY_ENV]);
	let dir = sandbox("get-refresh");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(HUB_KEY_ENV);
	let url = spawn_api(vec![
		env_err("unauthorized"),
		env_ok(serde_json::json!({"jwt": "j2"})),
		env_ok(account_json()),
	])
	.await;
	std::env::set_var(API_URL_ENV, &url);
	save_session(&Session {
		jwt: "j1".into(),
		refresh_token: "r1".into(),
		api_url: url.clone(),
	})
	.expect("save session");

	let account = get::<Account>("/auth/me")
		.await
		.expect("call")
		.expect("refreshed");
	assert_eq!(account.email, "dev@example.com");
	let s = session().expect("session persisted after refresh");
	assert_eq!(s.jwt, "j2", "fresh jwt stored");
	assert_eq!(s.refresh_token, "r1", "refresh token kept");
}

#[tokio::test]
#[serial]
async fn get_returns_none_when_the_refresh_token_is_dead() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV, HUB_KEY_ENV]);
	let dir = sandbox("get-dead-refresh");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(HUB_KEY_ENV);
	let url = spawn_api(vec![env_err("token_expired"), env_err("revoked")]).await;
	std::env::set_var(API_URL_ENV, &url);
	save_session(&Session {
		jwt: "j1".into(),
		refresh_token: "r1".into(),
		api_url: url.clone(),
	})
	.expect("save session");

	assert!(get::<Account>("/auth/me").await.expect("call").is_none());
}

#[tokio::test]
#[serial]
async fn get_returns_none_when_still_unauthorized_after_refresh() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV, HUB_KEY_ENV]);
	let dir = sandbox("get-stale-again");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(HUB_KEY_ENV);
	let url = spawn_api(vec![
		env_err("unauthorized"),
		env_ok(serde_json::json!({"jwt": "j2"})),
		env_err("token_expired"),
	])
	.await;
	std::env::set_var(API_URL_ENV, &url);
	save_session(&Session {
		jwt: "j1".into(),
		refresh_token: "r1".into(),
		api_url: url.clone(),
	})
	.expect("save session");

	assert!(get::<Account>("/auth/me").await.expect("call").is_none());
}

#[tokio::test]
#[serial]
async fn get_surfaces_api_error_codes() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV, HUB_KEY_ENV]);
	let dir = sandbox("get-error");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(HUB_KEY_ENV);
	let url = spawn_api(vec![env_err("forbidden")]).await;
	std::env::set_var(API_URL_ENV, &url);
	save_session(&Session {
		jwt: "j1".into(),
		refresh_token: "r1".into(),
		api_url: url.clone(),
	})
	.expect("save session");

	let e = get::<Account>("/auth/me")
		.await
		.expect_err("error code surfaces");
	assert!(e.to_string().contains("forbidden"), "{e}");
}

#[tokio::test]
#[serial]
async fn refresh_persists_the_new_jwt_and_keeps_the_refresh_token() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV]);
	let dir = sandbox("refresh-direct");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let url = spawn_api(vec![env_ok(serde_json::json!({"jwt": "fresh"}))]).await;
	std::env::set_var(API_URL_ENV, &url);
	let s = Session {
		jwt: "old".into(),
		refresh_token: "keep-me".into(),
		api_url: url.clone(),
	};

	assert!(refresh(&s).await.expect("refresh call"));
	let stored = session().expect("persisted");
	assert_eq!(stored.jwt, "fresh");
	assert_eq!(stored.refresh_token, "keep-me");
}

#[tokio::test]
#[serial]
async fn refresh_reports_a_dead_token_as_false() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV]);
	let dir = sandbox("refresh-dead");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let url = spawn_api(vec![env_err("revoked")]).await;
	std::env::set_var(API_URL_ENV, &url);
	let s = Session {
		jwt: "old".into(),
		refresh_token: "dead".into(),
		api_url: url.clone(),
	};

	assert!(!refresh(&s).await.expect("refresh call"));
}

#[tokio::test]
#[serial]
async fn whoami_and_usage_read_the_signed_in_account() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV, HUB_KEY_ENV]);
	let dir = sandbox("whoami-session");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(HUB_KEY_ENV);
	let url = spawn_api(vec![env_ok(account_json()), env_ok(usage_json())]).await;
	std::env::set_var(API_URL_ENV, &url);
	save_session(&Session {
		jwt: "j1".into(),
		refresh_token: "r1".into(),
		api_url: url.clone(),
	})
	.expect("save session");

	let who = whoami().await.expect("whoami").expect("signed in");
	assert_eq!(who.email, "dev@example.com");

	let u = usage().await.expect("usage").expect("usage payload");
	assert_eq!(u.balance_usd, 9.0);
	let w = u.window.as_ref().expect("v2 payload carries one window");
	assert_eq!(w.label.as_deref(), Some("billing period"));
	assert_eq!(w.reserved_usd, Some(0.5));
	assert_eq!(w.allowance_usd, 20.0);
	assert_eq!(u.always_free_models, vec!["glm-5.3-flash".to_string()]);
	assert_eq!(u.network.included_gb, 1.0);
}

#[tokio::test]
#[serial]
async fn hub_usage_serves_a_bare_hub_key() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV, HUB_KEY_ENV]);
	let dir = sandbox("hub-key");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let mut payload = usage_json();
	payload["account"] = account_json();
	let url = spawn_api(vec![env_ok(payload.clone()), env_ok(payload)]).await;
	std::env::set_var(API_URL_ENV, &url);
	// No auth.json: the hub key is the only credential.
	std::env::set_var(HUB_KEY_ENV, "hk-live-key");

	let who = whoami()
		.await
		.expect("whoami")
		.expect("hub key identifies the account");
	assert_eq!(who.plan, "pro");
	let u = usage().await.expect("usage").expect("hub usage payload");
	// `cap_usd` is the pre-v2 spelling and must still land on allowance_usd —
	// the CLI ships ahead of the control plane.
	assert_eq!(
		u.month.as_ref().expect("legacy month window").allowance_usd,
		100.0
	);
}

#[tokio::test]
#[serial]
async fn hub_usage_ignores_blank_keys_without_calling_the_api() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, HUB_KEY_ENV]);
	let dir = sandbox("hub-blank");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::set_var(HUB_KEY_ENV, "   ");

	// No stub is spawned: a blank key must degrade before any request.
	assert!(hub_usage().await.expect("call").is_none());
	assert!(whoami().await.expect("whoami").is_none());
}

#[tokio::test]
#[serial]
async fn start_login_returns_the_device_flow() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV]);
	let dir = sandbox("start-login");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let url = spawn_api(vec![env_ok(serde_json::json!({
		"device_code": "dc-1",
		"user_code": "AB12-CD34",
		"verification_url": "https://octomind.run/app/login/cli?code=AB12-CD34",
		"verification_url_complete": "https://octomind.run/app/login/cli?code=AB12-CD34&complete",
		"interval": 1
	}))])
	.await;
	std::env::set_var(API_URL_ENV, &url);

	let start = start_login().await.expect("login starts");
	assert_eq!(start.device_code, "dc-1");
	assert_eq!(start.user_code, "AB12-CD34");
	assert_eq!(start.interval, 1);
}

#[tokio::test]
#[serial]
async fn poll_login_waits_through_pending_then_claims() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV]);
	let dir = sandbox("poll-login");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let url = spawn_api(vec![
		env_err("pending"),
		env_ok(serde_json::json!({"api_key": "hk-1", "jwt": "j-1", "refresh_token": "r-1"})),
	])
	.await;
	std::env::set_var(API_URL_ENV, &url);

	let claim = poll_login("dc-1", Duration::from_secs(1))
		.await
		.expect("claim");
	assert_eq!(claim.api_key, "hk-1");
	assert_eq!(claim.jwt, "j-1");
	assert_eq!(claim.refresh_token, "r-1");
	assert_eq!(
		claim.key_name, "octomind-cli",
		"absent key_name uses the default"
	);
}

#[tokio::test]
#[serial]
async fn poll_login_maps_expired_and_other_errors() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV]);
	let dir = sandbox("poll-login-err");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let url = spawn_api(vec![env_err("expired"), env_err("denied")]).await;
	std::env::set_var(API_URL_ENV, &url);

	let e = match poll_login("dc-1", Duration::from_secs(1)).await {
		Err(e) => e,
		Ok(_) => panic!("expired code must fail the poll"),
	};
	assert!(e.to_string().contains("expired"), "{e}");

	let e = match poll_login("dc-1", Duration::from_secs(1)).await {
		Err(e) => e,
		Ok(_) => panic!("denied code must fail the poll"),
	};
	assert!(e.to_string().contains("login failed: denied"), "{e}");
}

#[test]
#[serial]
fn finish_login_writes_the_env_file_and_the_session() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY, API_URL_ENV, HUB_KEY_ENV]);
	let dir = sandbox("finish-login");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(API_URL_ENV);

	let claim = LoginClaim {
		api_key: "hk-final".into(),
		jwt: "jwt-final".into(),
		refresh_token: "r-final".into(),
		key_name: "octomind-cli-x1".into(),
	};

	let env_path = finish_login(&claim).expect("finish login");
	assert_eq!(env_path, dir.join("config").join(".env"));
	let env_body = std::fs::read_to_string(&env_path).expect(".env written");
	assert!(env_body.contains("OCTOHUB_API_KEY=hk-final"), "{env_body}");
	assert_eq!(std::env::var(HUB_KEY_ENV).as_deref(), Ok("hk-final"));

	let s = session().expect("session stored");
	assert_eq!(s.jwt, "jwt-final");
	assert_eq!(s.refresh_token, "r-final");
}
