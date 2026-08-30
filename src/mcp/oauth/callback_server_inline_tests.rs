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

use super::*;
use crate::mcp::oauth::token_store::{clear_token, load_token};

fn test_state(auth_state: Option<&str>, token_url: &str) -> CallbackServerState {
	CallbackServerState {
		auth_state: Arc::new(Mutex::new(auth_state.map(|s| s.to_string()))),
		result_tx: Arc::new(Mutex::new(None)),
		shutdown: Arc::new(AtomicBool::new(false)),
		config: OAuthConfig::new(
			"callback-test-client".to_string(),
			String::new(),
			"https://auth.example.com/oauth/authorize".to_string(),
			token_url.to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["mcp:read".to_string()],
		),
		server_name: format!("callback-test-{}", uuid::Uuid::new_v4()),
		code_verifier: "test-verifier".to_string(),
		redirect_uri: "http://localhost:34567/oauth/callback".to_string(),
	}
}

// ------------------------------------------------------------------
// OAuthCallbackResult — Clone + Debug derives
// ------------------------------------------------------------------

#[test]
fn callback_result_supports_clone_and_debug() {
	let success = OAuthCallbackResult::Success {
		access_token: "token".to_string(),
		refresh_token: Some("refresh".to_string()),
		expires_in: 3600,
		scopes: vec!["mcp:read".to_string()],
	};
	match success.clone() {
		OAuthCallbackResult::Success {
			access_token,
			refresh_token,
			expires_in,
			scopes,
		} => {
			assert_eq!(access_token, "token");
			assert_eq!(refresh_token.as_deref(), Some("refresh"));
			assert_eq!(expires_in, 3600);
			assert_eq!(scopes, vec!["mcp:read".to_string()]);
		}
		other => panic!("clone should stay Success, got {other:?}"),
	}
	assert!(format!("{success:?}").contains("Success"));

	let error = OAuthCallbackResult::Error {
		error: "access_denied".to_string(),
		description: Some("nope".to_string()),
	};
	assert!(format!("{:?}", error.clone()).contains("access_denied"));

	assert!(format!("{:?}", OAuthCallbackResult::Cancelled.clone()).contains("Cancelled"));
	assert!(format!("{:?}", OAuthCallbackResult::Timeout.clone()).contains("Timeout"));
}

// ------------------------------------------------------------------
// process_callback — query parsing and state validation
// (all cases below return before the token exchange: no HTTP)
// ------------------------------------------------------------------

#[tokio::test]
async fn process_callback_rejects_mismatched_state() {
	let state = test_state(
		Some("expected-state"),
		"https://auth.example.com/oauth/token",
	);
	match process_callback("code=abc&state=wrong-state", &state).await {
		OAuthCallbackResult::Error { error, description } => {
			assert_eq!(error, "invalid_state");
			assert!(description.unwrap().contains("Expected: expected-state"));
		}
		other => panic!("expected invalid_state, got {other:?}"),
	}
}

#[tokio::test]
async fn process_callback_rejects_missing_state() {
	let state = test_state(
		Some("expected-state"),
		"https://auth.example.com/oauth/token",
	);
	match process_callback("code=abc", &state).await {
		OAuthCallbackResult::Error { error, .. } => assert_eq!(error, "missing_state"),
		other => panic!("expected missing_state, got {other:?}"),
	}
}

#[tokio::test]
async fn process_callback_rejects_missing_or_blank_code() {
	let state = test_state(
		Some("expected-state"),
		"https://auth.example.com/oauth/token",
	);
	match process_callback("state=expected-state", &state).await {
		OAuthCallbackResult::Error { error, .. } => assert_eq!(error, "missing_code"),
		other => panic!("expected missing_code, got {other:?}"),
	}

	// Whitespace-only code is treated as missing
	let state = test_state(
		Some("expected-state"),
		"https://auth.example.com/oauth/token",
	);
	match process_callback("code=%20%20&state=expected-state", &state).await {
		OAuthCallbackResult::Error { error, .. } => assert_eq!(error, "missing_code"),
		other => panic!("expected missing_code, got {other:?}"),
	}
}

#[tokio::test]
async fn process_callback_returns_error_from_provider() {
	let state = test_state(
		Some("expected-state"),
		"https://auth.example.com/oauth/token",
	);
	// error_description arrives URL-encoded and must be decoded
	match process_callback(
		"error=access_denied&error_description=User%20denied%20access&state=expected-state",
		&state,
	)
	.await
	{
		OAuthCallbackResult::Error { error, description } => {
			assert_eq!(error, "access_denied");
			assert_eq!(description.as_deref(), Some("User denied access"));
		}
		other => panic!("expected access_denied, got {other:?}"),
	}
}

#[tokio::test]
async fn process_callback_rejects_reuse_of_consumed_state() {
	// A first callback consumes the expected state (take()); a second
	// callback must be rejected as already processed.
	let state = test_state(
		Some("expected-state"),
		"https://auth.example.com/oauth/token",
	);
	match process_callback("state=expected-state", &state).await {
		OAuthCallbackResult::Error { error, .. } => assert_eq!(error, "missing_code"),
		other => panic!("expected missing_code, got {other:?}"),
	}
	match process_callback("code=abc&state=expected-state", &state).await {
		OAuthCallbackResult::Error { error, description } => {
			assert_eq!(error, "state_already_used");
			assert_eq!(description.as_deref(), Some("Callback already processed"));
		}
		other => panic!("expected state_already_used, got {other:?}"),
	}
}

// ------------------------------------------------------------------
// process_callback — success path over a loopback token stub
// (same pattern as discovery.rs tests; the token persists to the
// cfg(test)-isolated keystore under the MCP server name)
// ------------------------------------------------------------------

/// Minimal loopback HTTP stub answering the token exchange POST with `body`.
/// Returns the token endpoint URL to use in the config.
async fn spawn_token_stub(body: &'static str) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	tokio::spawn(async move {
		loop {
			let Ok((mut sock, _)) = listener.accept().await else {
				break;
			};
			tokio::spawn(async move {
				let mut buf = vec![0u8; 8192];
				let Ok(n) = sock.read(&mut buf).await else {
					return;
				};
				if n == 0 {
					return;
				}
				let response = format!(
						"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
						body.len(),
						body
					);
				let _ = sock.write_all(response.as_bytes()).await;
			});
		}
	});
	format!("http://{addr}/oauth/token")
}

#[tokio::test]
async fn process_callback_success_exchanges_and_persists_token() {
	let token_url = spawn_token_stub(
			r#"{"access_token":"stub-access-token","token_type":"bearer","expires_in":3600,"refresh_token":"stub-refresh","scope":["mcp:read","mcp:write"]}"#,
		)
		.await;
	let state = test_state(Some("expected-state"), &token_url);

	match process_callback("code=abc123&state=expected-state", &state).await {
		OAuthCallbackResult::Success {
			access_token,
			refresh_token,
			expires_in,
			scopes,
		} => {
			assert_eq!(access_token, "stub-access-token");
			assert_eq!(refresh_token.as_deref(), Some("stub-refresh"));
			assert_eq!(expires_in, 3600);
			assert_eq!(scopes, vec!["mcp:read", "mcp:write"]);
		}
		other => panic!("expected Success, got {other:?}"),
	}

	// The token must be saved under the MCP server name — the key
	// get_valid_token looks up.
	let saved = load_token(&state.server_name)
		.await
		.unwrap()
		.expect("token should be persisted");
	assert_eq!(saved.access_token, "stub-access-token");
	assert_eq!(saved.refresh_token.as_deref(), Some("stub-refresh"));
	assert!(saved.expires_at > 0);

	clear_token(&state.server_name, false, None, None, None)
		.await
		.unwrap();
}

#[tokio::test]
async fn process_callback_zero_expires_in_defaults_to_one_year() {
	// GitHub-style response: no expires_in (tokens don't expire),
	// comma-separated scope string.
	let token_url = spawn_token_stub(r#"{"access_token":"gho_token","scope":"repo, user"}"#).await;
	let state = test_state(Some("expected-state"), &token_url);

	match process_callback("code=abc&state=expected-state", &state).await {
		OAuthCallbackResult::Success {
			expires_in, scopes, ..
		} => {
			assert_eq!(expires_in, 365 * 24 * 60 * 60);
			assert_eq!(scopes, vec!["repo", "user"]);
		}
		other => panic!("expected Success, got {other:?}"),
	}

	clear_token(&state.server_name, false, None, None, None)
		.await
		.unwrap();
}
