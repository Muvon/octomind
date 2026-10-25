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

//! External unit tests for OAuth flow orchestration: `get_access_token`
//! token-store integration (valid / expired / force-refresh / refresh-token
//! paths) and `start_oauth_flow` config validation. Complements the inline
//! `mod tests` (which covers `OAuthConfig` construction, validation, and serde).

use super::token_store::{clear_token, save_token, TokenMetadata};
use super::*;
use serial_test::serial;

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn valid_config() -> OAuthConfig {
	OAuthConfig::new(
		"client-123".to_string(),
		String::new(),
		"https://auth.example.com/oauth/authorize".to_string(),
		"https://auth.example.com/oauth/token".to_string(),
		"http://localhost:34567/oauth/callback".to_string(),
		vec!["mcp:read".to_string()],
	)
}

fn invalid_config() -> OAuthConfig {
	// Empty client_id fails validate() before any server is bound or
	// browser opened — the flow aborts deterministically.
	let mut config = valid_config();
	config.client_id = String::new();
	config
}

fn now_secs() -> u64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap()
		.as_secs()
}

fn unique_server(prefix: &str) -> String {
	format!("{prefix}-{}", uuid::Uuid::new_v4())
}

async fn store_token(server: &str, access_token: &str, expires_at: u64) {
	save_token(
		server,
		&TokenMetadata {
			server_name: server.to_string(),
			access_token: access_token.to_string(),
			refresh_token: None,
			expires_at,
			scopes: vec!["mcp:read".to_string()],
		},
	)
	.await
	.expect("save token");
}

// ------------------------------------------------------------------
// get_access_token — token store integration
// ------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn get_access_token_returns_stored_valid_token_without_flow() {
	let server = unique_server("oauth-valid");
	store_token(&server, "stored-access-token", now_secs() + 3600).await;

	let token = get_access_token(&valid_config(), &server, false)
		.await
		.expect("valid stored token must be returned");
	assert_eq!(token.as_deref(), Some("stored-access-token"));

	clear_token(&server, false, None, None, None).await.unwrap();
}

#[serial]
#[tokio::test]
async fn get_access_token_expired_token_falls_through_to_flow() {
	let server = unique_server("oauth-expired");
	// Expired well beyond the 300s refresh buffer.
	store_token(&server, "stale-token", now_secs() - 3600).await;

	// Flow starts (no valid token) and aborts on the invalid config —
	// proving the expired token was NOT returned.
	let err = get_access_token(&invalid_config(), &server, false).await.unwrap_err();
	assert!(err.to_string().contains("OAuth config validation failed"), "{err}");

	clear_token(&server, false, None, None, None).await.unwrap();
}

#[serial]
#[tokio::test]
async fn get_access_token_force_refresh_skips_valid_stored_token() {
	let server = unique_server("oauth-force");
	store_token(&server, "still-valid-token", now_secs() + 3600).await;

	// force_refresh bypasses the token check and goes straight to the flow,
	// which aborts on the invalid config instead of returning the token.
	let err = get_access_token(&invalid_config(), &server, true).await.unwrap_err();
	assert!(err.to_string().contains("OAuth config validation failed"), "{err}");

	clear_token(&server, false, None, None, None).await.unwrap();
}

#[serial]
#[tokio::test]
async fn get_access_token_missing_token_starts_flow() {
	let server = unique_server("oauth-missing");

	let err = get_access_token(&invalid_config(), &server, false).await.unwrap_err();
	assert!(err.to_string().contains("OAuth config validation failed"), "{err}");
}

// ------------------------------------------------------------------
// get_access_token — refresh-token grant
// ------------------------------------------------------------------

/// One-shot token endpoint: answers the first request with `status` + `body`
/// and hands back the request body it received.
async fn spawn_token_endpoint(status: &'static str, body: String) -> (String, tokio::task::JoinHandle<String>) {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind token stub");
	let url = format!("http://{}/oauth/token", listener.local_addr().expect("addr"));
	let handle = tokio::spawn(async move {
		let (mut sock, _) = listener.accept().await.expect("accept");
		let mut buf = Vec::new();
		let mut tmp = [0u8; 4096];
		let header_end = loop {
			let n = sock.read(&mut tmp).await.expect("read");
			assert!(n > 0, "client closed before sending headers");
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
			.expect("content-length header");
		while buf.len() < header_end + content_length {
			let n = sock.read(&mut tmp).await.expect("read body");
			assert!(n > 0, "client closed mid-body");
			buf.extend_from_slice(&tmp[..n]);
		}
		let response = format!(
			"HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
			body.len()
		);
		sock.write_all(response.as_bytes()).await.expect("write response");
		String::from_utf8_lossy(&buf[header_end..header_end + content_length]).into_owned()
	});
	(url, handle)
}

async fn store_refreshable_token(server: &str, expires_at: u64) {
	save_token(
		server,
		&TokenMetadata {
			server_name: server.to_string(),
			access_token: "near-expiry-token".to_string(),
			refresh_token: Some("stored-refresh-token".to_string()),
			expires_at,
			scopes: vec!["mcp:read".to_string()],
		},
	)
	.await
	.expect("save token");
}

#[serial]
#[tokio::test]
async fn get_access_token_renews_near_expiry_token_with_refresh_token() {
	let server = unique_server("oauth-refresh");
	// Inside the 300s refresh buffer: no longer "valid", but refreshable.
	store_refreshable_token(&server, now_secs() + 60).await;
	let (token_url, endpoint) = spawn_token_endpoint(
		"200 OK",
		r#"{"access_token":"refreshed-token","token_type":"Bearer","expires_in":3600}"#.to_string(),
	)
	.await;
	let mut config = valid_config();
	config.token_url = token_url;

	let token = get_access_token(&config, &server, false)
		.await
		.expect("refresh must succeed without a browser flow");
	assert_eq!(token.as_deref(), Some("refreshed-token"));

	let request: std::collections::HashMap<String, String> =
		serde_urlencoded::from_str(&endpoint.await.expect("endpoint")).expect("form body");
	assert_eq!(request["grant_type"], "refresh_token");
	assert_eq!(request["refresh_token"], "stored-refresh-token");
	assert!(
		!request.contains_key("client_secret"),
		"public client must not send a secret"
	);

	// The renewed token is persisted; the non-rotated refresh token is kept.
	let stored = token_store::load_token(&server).await.unwrap().expect("stored token");
	assert_eq!(stored.access_token, "refreshed-token");
	assert_eq!(stored.refresh_token.as_deref(), Some("stored-refresh-token"));
	assert!(stored.expires_at > now_secs() + 3000, "expiry must move forward");

	clear_token(&server, false, None, None, None).await.unwrap();
}

#[serial]
#[tokio::test]
async fn get_access_token_rejected_refresh_falls_back_to_sign_in() {
	let server = unique_server("oauth-refresh-rejected");
	store_refreshable_token(&server, now_secs() + 60).await;
	let (token_url, endpoint) =
		spawn_token_endpoint("400 Bad Request", r#"{"error":"invalid_grant"}"#.to_string()).await;
	// Empty client_id: the refresh request still goes out, then the sign-in
	// flow aborts at validation — proving the fallback was reached.
	let mut config = invalid_config();
	config.token_url = token_url;

	let err = get_access_token(&config, &server, false).await.unwrap_err();
	assert!(err.to_string().contains("OAuth config validation failed"), "{err}");
	let request = endpoint.await.expect("refresh was attempted first");
	assert!(request.contains("refresh_token"), "{request}");

	clear_token(&server, false, None, None, None).await.unwrap();
}

// ------------------------------------------------------------------
// get_access_token_non_interactive — never reaches the browser flow
// ------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn non_interactive_token_renews_near_expiry_token() {
	let server = unique_server("oauth-noninteractive-refresh");
	store_refreshable_token(&server, now_secs() + 60).await;
	let (token_url, endpoint) = spawn_token_endpoint(
		"200 OK",
		r#"{"access_token":"refreshed-token","refresh_token":"rotated-refresh-token","expires_in":3600}"#.to_string(),
	)
	.await;
	let mut config = valid_config();
	config.token_url = token_url;

	let token = get_access_token_non_interactive(&config, &server)
		.await
		.expect("refresh");
	assert_eq!(token.as_deref(), Some("refreshed-token"));
	endpoint.await.expect("refresh request served");
	// A rotated refresh token replaces the stored one.
	let stored = token_store::load_token(&server).await.unwrap().expect("stored token");
	assert_eq!(stored.refresh_token.as_deref(), Some("rotated-refresh-token"));

	clear_token(&server, false, None, None, None).await.unwrap();
}

#[serial]
#[tokio::test]
async fn non_interactive_token_rejected_refresh_yields_none_without_sign_in() {
	let server = unique_server("oauth-noninteractive-rejected");
	store_refreshable_token(&server, now_secs() + 60).await;
	let (token_url, endpoint) =
		spawn_token_endpoint("400 Bad Request", r#"{"error":"invalid_grant"}"#.to_string()).await;
	// Empty client_id: reaching the sign-in flow would fail validation with an
	// Err, so Ok(None) proves the browser flow was never entered.
	let mut config = invalid_config();
	config.token_url = token_url;

	let token = get_access_token_non_interactive(&config, &server)
		.await
		.expect("rejection is Ok(None), not an error");
	assert_eq!(token, None);
	let request = endpoint.await.expect("refresh was attempted");
	assert!(request.contains("refresh_token"), "{request}");

	clear_token(&server, false, None, None, None).await.unwrap();
}

// ------------------------------------------------------------------
// start_oauth_flow — validation gate
// ------------------------------------------------------------------

#[tokio::test]
async fn start_oauth_flow_rejects_invalid_config_before_binding() {
	let err = start_oauth_flow(&invalid_config(), "never-started").await.unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("OAuth config validation failed"), "{msg}");
	assert!(msg.contains("client_id"), "{msg}");
}

// ------------------------------------------------------------------
// is_authenticated — stored token state
// ------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn is_authenticated_reflects_stored_token_lifecycle() {
	let server = unique_server("oauth-authed");
	assert!(
		!is_authenticated(&server, 300).await,
		"unknown server must be unauthenticated"
	);

	store_token(&server, "live-token", now_secs() + 3600).await;
	assert!(
		is_authenticated(&server, 300).await,
		"valid stored token must authenticate"
	);

	clear_token(&server, false, None, None, None).await.unwrap();
	assert!(
		!is_authenticated(&server, 300).await,
		"cleared token must deauthenticate"
	);
}
