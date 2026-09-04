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

//! Octomind account client — the control-plane API behind `octomind login`.
//!
//! `octomind login` leaves two things behind, and neither is a developer API key
//! (those stay something you create deliberately in the panel):
//!
//! - `OCTOHUB_API_KEY` in the user-scope `.env` — the model-gateway credential
//!   octolib's octohub provider reads.
//! - a panel session (`auth.json`) — the same short-lived jwt + long refresh
//!   token the browser gets. This module authenticates with it and refreshes
//!   automatically on 401, so a logged-in CLI stays logged in.
//!
//! Not signed in is a normal state, not an error — the CLI works fine against
//! your own provider keys — so lookups return `None` rather than failing.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::directories::get_config_dir;

/// Control-plane API. Override for local development.
pub const API_URL_ENV: &str = "OCTOMIND_API_URL";
pub const DEFAULT_API_URL: &str = "https://api.octomind.run";
/// Panel origin for browser hand-offs. Only needed when pointing at a panel the
/// API doesn't know about (e.g. a local dev server).
pub const PANEL_URL_ENV: &str = "OCTOMIND_PANEL_URL";
/// Model-gateway credential, read by octolib's octohub provider.
pub const HUB_KEY_ENV: &str = "OCTOHUB_API_KEY";

const TIMEOUT: Duration = Duration::from_secs(20);

pub fn api_url() -> String {
	std::env::var(API_URL_ENV)
		.unwrap_or_else(|_| DEFAULT_API_URL.to_string())
		.trim_end_matches('/')
		.to_string()
}

/// The stored panel session. Sits next to the config, mode 0600 — it is a live
/// credential, not configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Session {
	pub jwt: String,
	pub refresh_token: String,
	/// Which API this session belongs to, so pointing at a different host doesn't
	/// silently reuse a session minted somewhere else.
	#[serde(default)]
	pub api_url: String,
}

pub fn session_path() -> Result<PathBuf> {
	Ok(get_config_dir()?.join("auth.json"))
}

/// A stable id for THIS machine, created once and kept next to the config.
///
/// It names the key a login mints (`octomind-cli-<id>`), so signing in here only
/// ever supersedes this machine's key and other machines keep working. Not a
/// secret and not a credential — it only has to be stable and unique-ish. It
/// deliberately does not live in `auth.json`: it must outlive signing out.
pub fn machine_id() -> Result<String> {
	let path = get_config_dir()?.join("machine-id");
	if let Ok(existing) = std::fs::read_to_string(&path) {
		let id = existing.trim().to_string();
		if !id.is_empty() {
			return Ok(id);
		}
	}
	let id: String = uuid::Uuid::new_v4()
		.simple()
		.to_string()
		.chars()
		.take(12)
		.collect();
	std::fs::write(&path, &id).with_context(|| format!("could not write {}", path.display()))?;
	Ok(id)
}

/// The current session, if a login left one for THIS api_url.
pub fn session() -> Option<Session> {
	let raw = std::fs::read_to_string(session_path().ok()?).ok()?;
	let s: Session = serde_json::from_str(&raw).ok()?;
	if !s.api_url.is_empty() && s.api_url != api_url() {
		return None;
	}
	(!s.jwt.is_empty()).then_some(s)
}

pub fn save_session(s: &Session) -> Result<PathBuf> {
	let path = session_path()?;
	std::fs::write(&path, serde_json::to_string_pretty(s)?)
		.with_context(|| format!("could not write {}", path.display()))?;
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
	}
	Ok(path)
}

fn client() -> Result<reqwest::Client> {
	Ok(reqwest::Client::builder().timeout(TIMEOUT).build()?)
}

/// kisscore answers `[err, data]` on every status, so the error CODE is what we
/// surface — an HTTP number tells the user nothing they can act on.
async fn envelope<T: for<'de> Deserialize<'de>>(
	res: reqwest::Response,
) -> Result<std::result::Result<T, String>> {
	let parsed: (Option<String>, Option<serde_json::Value>) = res
		.json()
		.await
		.context("unexpected response from the API (not an Octomind endpoint?)")?;
	match parsed {
		(Some(err), _) => Ok(Err(err)),
		(None, Some(data)) => Ok(Ok(serde_json::from_value(data)?)),
		(None, None) => bail!("empty response from the API"),
	}
}

/// Unauthenticated POST — the login flow, before any credential exists.
pub async fn post_public<T: for<'de> Deserialize<'de>>(
	path: &str,
	body: serde_json::Value,
) -> Result<std::result::Result<T, String>> {
	let res = client()?
		.post(format!("{}/api/v1{path}", api_url()))
		.json(&body)
		.send()
		.await
		.with_context(|| format!("could not reach {}", api_url()))?;
	envelope(res).await
}

#[derive(Deserialize)]
struct Refreshed {
	jwt: String,
}

/// Trade the refresh token for a fresh jwt and persist it. `false` = the refresh
/// token is dead too (idle past its window, or revoked): sign in again.
async fn refresh(s: &Session) -> Result<bool> {
	let res: std::result::Result<Refreshed, String> = post_public(
		"/auth/refresh",
		serde_json::json!({ "refresh_token": s.refresh_token }),
	)
	.await?;
	match res {
		Ok(r) => {
			save_session(&Session {
				jwt: r.jwt,
				refresh_token: s.refresh_token.clone(),
				api_url: api_url(),
			})?;
			Ok(true)
		}
		Err(_) => Ok(false),
	}
}

/// Authenticated GET, refreshing once on 401 exactly as the panel does. `None` =
/// no usable session; the caller decides whether that is worth mentioning.
pub async fn get<T: for<'de> Deserialize<'de>>(path: &str) -> Result<Option<T>> {
	let Some(s) = session() else {
		return Ok(None);
	};
	let url = format!("{}/api/v1{path}", api_url());
	let send = |jwt: String| async {
		client()?
			.get(&url)
			.bearer_auth(jwt)
			.send()
			.await
			.with_context(|| format!("could not reach {}", api_url()))
	};

	match envelope::<T>(send(s.jwt.clone()).await?).await? {
		Ok(v) => Ok(Some(v)),
		Err(e) if e == "unauthorized" || e == "token_expired" => {
			if !refresh(&s).await? {
				return Ok(None); // signed out for real — `octomind login` again
			}
			let s = session().context("session vanished mid-refresh")?;
			match envelope::<T>(send(s.jwt).await?).await? {
				Ok(v) => Ok(Some(v)),
				Err(e) if e == "unauthorized" || e == "token_expired" => Ok(None),
				Err(e) => bail!("{e}"),
			}
		}
		Err(e) => bail!("{e}"),
	}
}

#[derive(Deserialize, Debug, Clone)]
pub struct Account {
	pub email: String,
	pub plan: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Window {
	/// "billing period" on paid, "7 days" on free — the server carries the only
	/// per-plan difference so the client never branches on the plan
	/// (spec/pricing-v2.md §1.1). Absent on pre-v2 servers.
	#[serde(default)]
	pub label: Option<String>,
	pub spent_usd: f64,
	/// Pre-claimed by cloud machines' future burn over a bounded horizon
	/// (Burn::RESERVE_HORIZON_HOURS). Absent on servers that predate reservations.
	#[serde(default)]
	pub reserved_usd: Option<f64>,
	/// The plan's allowance for this window. `cap_usd` is the pre-v2 spelling;
	/// accepting both keeps `/usage` working against an API that has not been
	/// upgraded yet, which matters because the CLI ships ahead of the control
	/// plane.
	#[serde(alias = "cap_usd")]
	pub allowance_usd: f64,
	pub resets_at: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Network {
	pub used_gb: f64,
	pub included_gb: f64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Usage {
	/// ONE window since pricing-v2. Optional so a pre-v2 server (which sends
	/// window_4h/week/month) still deserializes instead of failing the command.
	#[serde(default)]
	pub window: Option<Window>,
	/// Pre-v2 shape, kept only so `/usage` degrades instead of erroring against
	/// an older control plane. Read as a fallback, never preferred.
	#[serde(default)]
	pub month: Option<Window>,
	/// Models that never draw down the allowance, on any plan. Printing these is
	/// what stops a 1x allowance reading as mean (pricing-v2 §9.2).
	#[serde(default)]
	pub always_free_models: Vec<String>,
	pub balance_usd: f64,
	pub storage_gb: f64,
	pub storage_quota_gb: f64,
	pub network: Network,
}

/// Who this process is signed in as. `None` = not signed in. Two credentials
/// count: a stored panel session (from `octomind login`), or — inside a cloud
/// machine — the injected hub key, which the control plane accepts for the
/// read-only usage surface. Holding the hub key IS being signed in there; no
/// login flow runs inside a container.
pub async fn whoami() -> Result<Option<Account>> {
	if let Some(account) = get::<Account>("/auth/me").await? {
		return Ok(Some(account));
	}
	Ok(hub_usage().await?.map(|r| r.account))
}

/// Account usage. `None` = not signed in; nothing to show and nothing wrong.
pub async fn usage() -> Result<Option<Usage>> {
	if let Some(u) = get::<Usage>("/account/usage").await? {
		return Ok(Some(u));
	}
	Ok(hub_usage().await?.map(|r| r.usage))
}

/// The hub-key response: the usage payload plus who the key belongs to.
#[derive(Deserialize)]
struct HubUsageResponse {
	#[serde(flatten)]
	usage: Usage,
	account: Account,
}

// ── Device-authorization login flow (RFC 8628) ──────────────────────────────
//
// Shared by `octomind login` (the CLI subcommand) and the ACP `/login` command
// the octoweb panel drives, so both mint credentials the exact same way.

/// Stop polling well after the server's own TTL rather than hanging forever.
pub const MAX_LOGIN_POLL: Duration = Duration::from_secs(15 * 60);

/// The start of a device-authorization flow: a short code to show the user and
/// the browser URL where they confirm it.
#[derive(Deserialize)]
pub struct LoginStart {
	pub device_code: String,
	pub user_code: String,
	pub verification_url: String,
	pub verification_url_complete: String,
	pub interval: u64,
}

/// What a confirmed login hands back: a hub key plus a panel session.
#[derive(Deserialize)]
pub struct LoginClaim {
	pub api_key: String,
	pub jwt: String,
	pub refresh_token: String,
	/// What the key ended up called in the Keys page.
	#[serde(default = "default_key_name")]
	pub key_name: String,
}

fn default_key_name() -> String {
	"octomind-cli".to_string()
}

/// Begin a device-authorization login. The device id scopes the key this mints,
/// so signing in here supersedes only this machine's key — others keep working.
pub async fn start_login() -> Result<LoginStart> {
	let device_id = machine_id()?;
	post_public(
		"/auth/cli",
		serde_json::json!({
			"client": format!("octomind/{} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS),
			"device_id": device_id,
		}),
	)
	.await?
	.map_err(|e| anyhow::anyhow!("could not start login: {e}"))
}

/// Poll the token endpoint until the user confirms the code in the browser, the
/// code expires, or the server TTL runs out.
pub async fn poll_login(device_code: &str, interval: Duration) -> Result<LoginClaim> {
	let interval = interval.clamp(Duration::from_secs(1), Duration::from_secs(30));
	let deadline = std::time::Instant::now() + MAX_LOGIN_POLL;
	loop {
		if std::time::Instant::now() >= deadline {
			bail!("login timed out — start again");
		}
		tokio::time::sleep(interval).await;
		match post_public::<LoginClaim>(
			"/auth/cli/token",
			serde_json::json!({ "device_code": device_code }),
		)
		.await?
		{
			Ok(c) => return Ok(c),
			Err(e) if e == "pending" => continue,
			Err(e) if e == "expired" => bail!("that code expired — start again"),
			Err(e) => bail!("login failed: {e}"),
		}
	}
}

/// Persist a confirmed login: hub key into the user-scope `.env`, panel session
/// into `auth.json`, and this process's env so later work in the same run picks
/// it up without a restart. Returns the `.env` path that was written.
pub fn finish_login(claim: &LoginClaim) -> Result<PathBuf> {
	let env_path = write_hub_key(&claim.api_key)?;
	save_session(&Session {
		jwt: claim.jwt.clone(),
		refresh_token: claim.refresh_token.clone(),
		api_url: api_url(),
	})?;
	std::env::set_var(HUB_KEY_ENV, &claim.api_key);
	Ok(env_path)
}

/// Repoint a server-supplied verification URL at the local panel when
/// `OCTOMIND_PANEL_URL` is set; otherwise return it unchanged.
pub fn panel_url(url: &str) -> String {
	match std::env::var(PANEL_URL_ENV).ok() {
		Some(panel) => repoint(url, &panel),
		None => url.to_string(),
	}
}

/// Swap the origin of a server-supplied verification URL for the local panel.
fn repoint(url: &str, panel: &str) -> String {
	match url.find("/app") {
		Some(i) => format!("{}{}", panel.trim_end_matches('/'), &url[i..]),
		None => url.to_string(),
	}
}

/// Upsert `OCTOHUB_API_KEY` in the user-scope `.env` — the one every octomind
/// process loads at startup. Other variables in that file are left alone; a new
/// login replaces the previous key rather than appending a second line.
fn write_hub_key(key: &str) -> Result<PathBuf> {
	let path = get_config_dir()?.join(".env");
	let existing = std::fs::read_to_string(&path).unwrap_or_default();
	std::fs::write(&path, upsert(&existing, key))
		.with_context(|| format!("could not write {}", path.display()))?;

	// It holds a live credential: keep it owner-only.
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
	}
	Ok(path)
}

/// Replace any existing `OCTOHUB_API_KEY` line, keep every other line as-is.
fn upsert(existing: &str, key: &str) -> String {
	let prefix = format!("{}=", HUB_KEY_ENV);
	let mut out: Vec<&str> = existing
		.lines()
		.filter(|l| {
			!l.trim_start()
				.trim_start_matches("export ")
				.starts_with(&prefix)
		})
		.collect();
	let line = format!("{prefix}{key}");
	out.push(&line);
	out.join("\n") + "\n"
}

/// The one control-plane read a bare hub key opens: GET /hub/usage. `None`
/// when there is no key, or the server rejects/predates it — a self-hosted
/// octohub key means nothing to this API and must degrade silently.
async fn hub_usage() -> Result<Option<HubUsageResponse>> {
	let Some(key) = std::env::var(HUB_KEY_ENV)
		.ok()
		.filter(|k| !k.trim().is_empty())
	else {
		return Ok(None);
	};
	let res = client()?
		.get(format!("{}/api/v1/hub/usage", api_url()))
		.bearer_auth(key)
		.send()
		.await;
	// A transport failure here stays quiet: the session path already surfaced
	// real errors, and this fallback must not turn "no cloud" into noise.
	let Ok(res) = res else { return Ok(None) };
	match envelope::<HubUsageResponse>(res).await {
		Ok(Ok(r)) => Ok(Some(r)),
		_ => Ok(None),
	}
}

#[cfg(test)]
#[path = "account_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "account_tests.rs"]
mod account_tests;
