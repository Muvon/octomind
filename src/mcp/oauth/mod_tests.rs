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
//! token-store integration (valid / expired / force-refresh paths) and
//! `start_oauth_flow` config validation. Complements the inline `mod tests`
//! (which covers `OAuthConfig` construction, validation, and serde).

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
	let err = get_access_token(&invalid_config(), &server, false)
		.await
		.unwrap_err();
	assert!(
		err.to_string().contains("OAuth config validation failed"),
		"{err}"
	);

	clear_token(&server, false, None, None, None).await.unwrap();
}

#[serial]
#[tokio::test]
async fn get_access_token_force_refresh_skips_valid_stored_token() {
	let server = unique_server("oauth-force");
	store_token(&server, "still-valid-token", now_secs() + 3600).await;

	// force_refresh bypasses the token check and goes straight to the flow,
	// which aborts on the invalid config instead of returning the token.
	let err = get_access_token(&invalid_config(), &server, true)
		.await
		.unwrap_err();
	assert!(
		err.to_string().contains("OAuth config validation failed"),
		"{err}"
	);

	clear_token(&server, false, None, None, None).await.unwrap();
}

#[serial]
#[tokio::test]
async fn get_access_token_missing_token_starts_flow() {
	let server = unique_server("oauth-missing");

	let err = get_access_token(&invalid_config(), &server, false)
		.await
		.unwrap_err();
	assert!(
		err.to_string().contains("OAuth config validation failed"),
		"{err}"
	);
}

// ------------------------------------------------------------------
// start_oauth_flow — validation gate
// ------------------------------------------------------------------

#[tokio::test]
async fn start_oauth_flow_rejects_invalid_config_before_binding() {
	let err = start_oauth_flow(&invalid_config(), "never-started")
		.await
		.unwrap_err();
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
