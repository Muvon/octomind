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

//! External unit tests for the OAuth callback HTTP server: URL validation in
//! `start_callback_server`, request parsing and response rendering in
//! `handle_request`, the accept loop in `run_http_server`, and the remaining
//! `process_callback` query-shapes. Complements the inline `mod tests`.

use super::*;
use crate::mcp::oauth::token_store::clear_token;
use serial_test::serial;

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn state_with(auth_state: Option<&str>, token_url: &str) -> CallbackServerState {
	CallbackServerState {
		auth_state: Arc::new(Mutex::new(auth_state.map(|s| s.to_string()))),
		result_tx: Arc::new(Mutex::new(None)),
		shutdown: Arc::new(AtomicBool::new(false)),
		config: OAuthConfig::new(
			"cb-ext-test-client".to_string(),
			String::new(),
			"https://auth.example.com/oauth/authorize".to_string(),
			token_url.to_string(),
			"http://127.0.0.1:34567/oauth/callback".to_string(),
			vec!["mcp:read".to_string()],
		),
		server_name: format!("cb-ext-{}", uuid::Uuid::new_v4()),
		code_verifier: "test-verifier".to_string(),
		redirect_uri: "http://127.0.0.1:34567/oauth/callback".to_string(),
	}
}

fn config_with_callback(callback_url: &str) -> OAuthConfig {
	OAuthConfig::new(
		"cb-ext-test-client".to_string(),
		String::new(),
		"https://auth.example.com/oauth/authorize".to_string(),
		"https://auth.example.com/oauth/token".to_string(),
		callback_url.to_string(),
		vec!["mcp:read".to_string()],
	)
}

/// Minimal loopback HTTP stub answering the token exchange POST with `body`.
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

/// A connected loopback TCP pair for feeding `handle_request` directly.
async fn tcp_pair() -> (tokio::net::TcpStream, tokio::net::TcpStream) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
		.await
		.unwrap();
	let (server, _) = listener.accept().await.unwrap();
	(server, client)
}

async fn send_and_read(client: &mut tokio::net::TcpStream, request: &str) -> String {
	client.write_all(request.as_bytes()).await.unwrap();
	let mut buf = vec![0u8; 8192];
	let n = client.read(&mut buf).await.unwrap();
	String::from_utf8_lossy(&buf[..n]).to_string()
}

// ------------------------------------------------------------------
// start_callback_server — callback_url validation (fails before binding)
// ------------------------------------------------------------------

#[tokio::test]
async fn start_callback_server_rejects_unparseable_url() {
	let config = config_with_callback("not a url");
	let err = start_callback_server(&config, "srv", "state".to_string(), "verifier".to_string())
		.await
		.unwrap_err();
	assert!(err.to_string().contains("Invalid callback_url"), "{err}");
}

#[tokio::test]
async fn start_callback_server_rejects_url_without_host() {
	// Scheme-only URL: parses, but has no host component.
	let config = config_with_callback("unix:/run/missing-host");
	let err = start_callback_server(&config, "srv", "state".to_string(), "verifier".to_string())
		.await
		.unwrap_err();
	assert!(err.to_string().contains("must have a host"), "{err}");
}

#[tokio::test]
async fn start_callback_server_rejects_url_without_port() {
	// Non-HTTP scheme has no default port mapping.
	let config = config_with_callback("ftp://localhost/oauth/callback");
	let err = start_callback_server(&config, "srv", "state".to_string(), "verifier".to_string())
		.await
		.unwrap_err();
	assert!(err.to_string().contains("must have a valid port"), "{err}");
}

#[tokio::test]
async fn start_callback_server_fails_when_port_is_already_bound() {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let port = listener.local_addr().unwrap().port();
	let config = config_with_callback(&format!("http://127.0.0.1:{port}/oauth/callback"));
	let err = start_callback_server(&config, "srv", "state".to_string(), "verifier".to_string())
		.await
		.unwrap_err();
	// URL parsing succeeded — the failure comes from the bind itself.
	assert!(!err.to_string().contains("callback_url"), "{err}");
}

// ------------------------------------------------------------------
// handle_request — HTTP request parsing and response rendering
// ------------------------------------------------------------------

#[tokio::test]
async fn handle_request_returns_404_for_non_callback_paths() {
	let (server, mut client) = tcp_pair().await;
	let state = state_with(Some("s1"), "https://auth.example.com/oauth/token");
	let handler = tokio::spawn(async move { handle_request(server, state).await });
	let response = send_and_read(
		&mut client,
		"GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n",
	)
	.await;
	handler.await.unwrap().unwrap();
	assert!(response.starts_with("HTTP/1.1 404"), "{response}");
	assert!(response.contains("404 Not Found"), "{response}");
}

#[tokio::test]
async fn handle_request_error_callback_renders_error_page_and_delivers_result() {
	let (server, mut client) = tcp_pair().await;
	let state = state_with(Some("s1"), "https://auth.example.com/oauth/token");
	let (tx, rx) = tokio::sync::oneshot::channel();
	*state.result_tx.lock().await = Some(tx);
	let handler = tokio::spawn(async move { handle_request(server, state).await });
	let response = send_and_read(
		&mut client,
		"GET /oauth/callback?error=access_denied&error_description=Nope&state=s1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
	)
	.await;
	handler.await.unwrap().unwrap();
	assert!(response.starts_with("HTTP/1.1 200"), "{response}");
	assert!(
		response.contains("ERROR - Authorization Failed"),
		"{response}"
	);
	assert!(response.contains("access_denied"), "{response}");
	match rx.await.unwrap() {
		OAuthCallbackResult::Error { error, description } => {
			assert_eq!(error, "access_denied");
			assert_eq!(description.as_deref(), Some("Nope"));
		}
		other => panic!("expected Error, got {other:?}"),
	}
}

#[serial]
#[tokio::test]
async fn handle_request_success_callback_renders_ok_page_and_delivers_token() {
	let token_url = spawn_token_stub(
		r#"{"access_token":"ext-ok-token","token_type":"bearer","expires_in":3600,"refresh_token":"ext-refresh","scope":["mcp:read"]}"#,
	)
	.await;
	let (server, mut client) = tcp_pair().await;
	let state = state_with(Some("s1"), &token_url);
	let server_name = state.server_name.clone();
	let (tx, rx) = tokio::sync::oneshot::channel();
	*state.result_tx.lock().await = Some(tx);
	let handler = tokio::spawn(async move { handle_request(server, state).await });
	let response = send_and_read(
		&mut client,
		"GET /oauth/callback?code=abc&state=s1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
	)
	.await;
	handler.await.unwrap().unwrap();
	assert!(
		response.contains("OK - Authorization Successful"),
		"{response}"
	);
	match rx.await.unwrap() {
		OAuthCallbackResult::Success {
			access_token,
			refresh_token,
			expires_in,
			scopes,
		} => {
			assert_eq!(access_token, "ext-ok-token");
			assert_eq!(refresh_token.as_deref(), Some("ext-refresh"));
			assert_eq!(expires_in, 3600);
			assert_eq!(scopes, vec!["mcp:read".to_string()]);
		}
		other => panic!("expected Success, got {other:?}"),
	}
	clear_token(&server_name, false, None, None, None)
		.await
		.unwrap();
}

#[tokio::test]
async fn handle_request_tolerates_immediate_disconnect() {
	let (server, client) = tcp_pair().await;
	drop(client); // close without sending anything
	let state = state_with(Some("s1"), "https://auth.example.com/oauth/token");
	handle_request(server, state)
		.await
		.expect("zero-byte read is a no-op");
}

// ------------------------------------------------------------------
// run_http_server — accept loop lifecycle
// ------------------------------------------------------------------

#[tokio::test]
async fn run_http_server_serves_requests_until_shutdown_flag_set() {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	let state = state_with(Some("s1"), "https://auth.example.com/oauth/token");
	let (tx, rx) = tokio::sync::oneshot::channel();
	*state.result_tx.lock().await = Some(tx);
	let shutdown = state.shutdown.clone();
	let server_task = tokio::spawn(async move { run_http_server(&listener, state).await });

	let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
	let response = send_and_read(
		&mut client,
		"GET /oauth/callback?error=temporarily_unavailable&state=s1 HTTP/1.1\r\n\r\n",
	)
	.await;
	assert!(
		response.contains("ERROR - Authorization Failed"),
		"{response}"
	);
	match rx.await.unwrap() {
		OAuthCallbackResult::Error { error, .. } => assert_eq!(error, "temporarily_unavailable"),
		other => panic!("expected Error, got {other:?}"),
	}

	shutdown.store(true, Ordering::Relaxed);
	tokio::time::timeout(std::time::Duration::from_secs(3), server_task)
		.await
		.expect("server must observe shutdown within one accept tick")
		.unwrap();
}

// ------------------------------------------------------------------
// process_callback — remaining query shapes
// ------------------------------------------------------------------

#[tokio::test]
async fn process_callback_skips_malformed_and_unknown_pairs() {
	let state = state_with(Some("s1"), "https://auth.example.com/oauth/token");
	// "novalue" has no '=' and is skipped; unknown keys are ignored.
	match process_callback("novalue&foo=bar&error=access_denied&state=s1", &state).await {
		OAuthCallbackResult::Error { error, .. } => assert_eq!(error, "access_denied"),
		other => panic!("expected access_denied, got {other:?}"),
	}
}

#[tokio::test]
async fn process_callback_state_matching_trims_surrounding_whitespace() {
	let state = state_with(Some("s1"), "https://auth.example.com/oauth/token");
	// URL-encoded spaces around the state still match after trim; the request
	// then fails on the missing code — proving the state check passed.
	match process_callback("state=%20s1%20", &state).await {
		OAuthCallbackResult::Error { error, .. } => assert_eq!(error, "missing_code"),
		other => panic!("expected missing_code, got {other:?}"),
	}
}

#[tokio::test]
async fn process_callback_token_exchange_failure_is_reported() {
	// Token endpoint on a closed port → exchange fails → token_exchange_failed.
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let port = listener.local_addr().unwrap().port();
	drop(listener);
	let state = state_with(Some("s1"), &format!("http://127.0.0.1:{port}/oauth/token"));
	match process_callback("code=abc&state=s1", &state).await {
		OAuthCallbackResult::Error { error, description } => {
			assert_eq!(error, "token_exchange_failed");
			assert!(description.unwrap().contains("Failed to exchange code"));
		}
		other => panic!("expected token_exchange_failed, got {other:?}"),
	}
}
