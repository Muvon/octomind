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

//! Handler-level tests for the `/usage` session command. The account client
//! runs against a local HTTP stub speaking the kisscore `[err, data]`
//! envelope, so all three handler outcomes (signed in, anonymous, API error)
//! are exercised without any real API. Env-mutating tests are `#[serial]` and
//! hold `ENV_LOCK` across the awaited round trips.

use super::*;
use serial_test::serial;
use std::collections::VecDeque;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::account;
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

fn sandbox(tag: &str) -> std::path::PathBuf {
	let dir = std::env::temp_dir().join(format!("octomind-usage-{tag}-{}", std::process::id()));
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

/// The pricing-v2 shape: ONE window that carries its own label.
fn usage_json() -> serde_json::Value {
	serde_json::json!({
		"window": {
			"label": "billing period",
			"spent_usd": 3.0,
			"reserved_usd": 0.5,
			"allowance_usd": 20.0,
			"resets_at": "m"
		},
		"always_free_models": ["glm-5.3-flash", "gemma-4-31b"],
		"balance_usd": 9.0,
		"storage_gb": 1.0,
		"storage_quota_gb": 2.0,
		"network": {"used_gb": 0.5, "included_gb": 1.0}
	})
}

/// The PRE-v2 shape. The CLI ships ahead of the control plane, so `/usage` must
/// still render against an API that has not been upgraded — three windows, and
/// `cap_usd` where v2 sends `allowance_usd`.
fn legacy_usage_json() -> serde_json::Value {
	serde_json::json!({
		"window_4h": {"spent_usd": 1.0, "cap_usd": 10.0, "resets_at": "soon"},
		"week": {"spent_usd": 2.0, "reserved_usd": 0.5, "cap_usd": 50.0, "resets_at": "w"},
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
				let _header_end = loop {
					let n = sock.read(&mut tmp).await.unwrap_or(0);
					if n == 0 {
						return;
					}
					buf.extend_from_slice(&tmp[..n]);
					if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
						break pos + 4;
					}
				};
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

/// Unwrap the typed output the handler must produce.
fn usage_output(result: CommandResult) -> Box<CommandOutput> {
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected HandledWithOutput, got {result:?}");
	};
	output
}

#[tokio::test]
#[serial]
async fn anonymous_state_reports_signed_out_with_zeroed_numbers() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, account::API_URL_ENV, account::HUB_KEY_ENV]);
	let dir = sandbox("anon");
	std::env::set_var(DATA_DIR_KEY, &dir);
	// No auth.json and no hub key: both credential lookups return None without
	// touching the network, so no stub is needed.
	std::env::remove_var(account::API_URL_ENV);
	std::env::remove_var(account::HUB_KEY_ENV);

	let output = usage_output(handle_usage().await.expect("handler runs"));

	let CommandOutput::Usage {
		signed_in,
		account,
		windows,
		always_free_models,
		balance_usd,
		storage_gb,
		storage_quota_gb,
		network_used_gb,
		network_included_gb,
	} = *output
	else {
		panic!("expected Usage output");
	};
	assert!(!signed_in, "no credentials means signed out");
	assert!(account.is_none());
	assert!(
		windows.is_empty(),
		"nothing to report when signed out: {windows:?}"
	);
	assert!(
		always_free_models.is_empty(),
		"signed out has no roster to show"
	);
	assert_eq!(balance_usd, 0.0);
	assert_eq!(storage_gb, 0.0);
	assert_eq!(storage_quota_gb, 0.0);
	assert_eq!(network_used_gb, 0.0);
	assert_eq!(network_included_gb, 0.0);
}

#[tokio::test]
#[serial]
async fn signed_in_state_maps_the_window_and_the_account_label() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, account::API_URL_ENV, account::HUB_KEY_ENV]);
	let dir = sandbox("signed-in");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(account::HUB_KEY_ENV);
	// /account/usage is queried first, then /auth/me for the label.
	let url = spawn_api(vec![env_ok(usage_json()), env_ok(account_json())]).await;
	std::env::set_var(account::API_URL_ENV, &url);
	account::save_session(&account::Session {
		jwt: "j1".into(),
		refresh_token: "r1".into(),
		api_url: url.clone(),
	})
	.expect("save session");

	let output = usage_output(handle_usage().await.expect("handler runs"));

	let CommandOutput::Usage {
		signed_in,
		account,
		windows,
		always_free_models,
		balance_usd,
		storage_gb,
		storage_quota_gb,
		network_used_gb,
		network_included_gb,
	} = *output
	else {
		panic!("expected Usage output");
	};
	assert!(signed_in, "a stored session means signed in");
	assert_eq!(account.as_deref(), Some("dev@example.com (pro)"));
	// ONE window, and its label comes from the SERVER — the client must never
	// have to know whether this account bills on a period or a 7-day free slice.
	assert_eq!(windows.len(), 1, "exactly one window: {windows:?}");
	assert_eq!(windows[0].label, "billing period");
	assert_eq!(windows[0].spent_usd, 3.0);
	assert_eq!(windows[0].allowance_usd, 20.0);
	assert_eq!(windows[0].resets_at, "m");
	assert_eq!(windows[0].reserved_usd, Some(0.5));
	assert_eq!(
		always_free_models,
		vec!["glm-5.3-flash".to_string(), "gemma-4-31b".to_string()],
		"the free roster must reach the renderer — it is what keeps a 1x allowance from reading as mean"
	);
	assert_eq!(balance_usd, 9.0);
	assert_eq!(storage_gb, 1.0);
	assert_eq!(storage_quota_gb, 2.0);
	assert_eq!(network_used_gb, 0.5);
	assert_eq!(network_included_gb, 1.0);
}

/// The CLI ships ahead of the control plane, so a machine on a new binary
/// talking to an un-upgraded API must still print usage rather than erroring.
#[tokio::test]
#[serial]
async fn pre_v2_server_still_renders_via_the_month_window() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, account::API_URL_ENV, account::HUB_KEY_ENV]);
	let dir = sandbox("legacy");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(account::HUB_KEY_ENV);
	let url = spawn_api(vec![env_ok(legacy_usage_json()), env_ok(account_json())]).await;
	std::env::set_var(account::API_URL_ENV, &url);
	account::save_session(&account::Session {
		jwt: "j1".into(),
		refresh_token: "r1".into(),
		api_url: url.clone(),
	})
	.expect("save session");

	let output = usage_output(handle_usage().await.expect("handler runs"));
	let CommandOutput::Usage {
		windows,
		always_free_models,
		..
	} = *output
	else {
		panic!("expected Usage output");
	};
	// Falls back to `month`, and `cap_usd` deserializes into allowance_usd.
	assert_eq!(
		windows.len(),
		1,
		"legacy payload must collapse to one window"
	);
	assert_eq!(windows[0].label, "this period", "no server label to use");
	assert_eq!(windows[0].spent_usd, 3.0);
	assert_eq!(
		windows[0].allowance_usd, 100.0,
		"cap_usd alias did not apply"
	);
	assert!(
		always_free_models.is_empty(),
		"a pre-v2 server sends no roster, and that must not panic"
	);
}

#[tokio::test]
#[serial]
async fn api_error_surfaces_as_error_output_with_login_hint() {
	let _lock = ENV_LOCK.lock().await;
	let _env = EnvGuard::new(&[DATA_DIR_KEY, account::API_URL_ENV, account::HUB_KEY_ENV]);
	let dir = sandbox("api-error");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(account::HUB_KEY_ENV);
	let url = spawn_api(vec![env_err("boom")]).await;
	std::env::set_var(account::API_URL_ENV, &url);
	account::save_session(&account::Session {
		jwt: "j1".into(),
		refresh_token: "r1".into(),
		api_url: url.clone(),
	})
	.expect("save session");

	let output = usage_output(handle_usage().await.expect("handler runs"));

	let CommandOutput::Error { error, context } = *output else {
		panic!("expected Error output");
	};
	assert!(
		error.contains("Could not read account usage"),
		"error must name the failure: {error}"
	);
	assert!(
		error.contains("boom"),
		"error must carry the API error code: {error}"
	);
	let hint = context
		.as_ref()
		.and_then(|c| c.get("hint"))
		.and_then(|h| h.as_str())
		.expect("context carries the login hint");
	assert!(hint.contains("octomind login"), "hint: {hint}");
}
