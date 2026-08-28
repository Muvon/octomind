// Copyright 2026 Muvon Un Limited
//
// OAuth 2.1 Callback Server

use super::OAuthConfig;
use crate::mcp::oauth::flow::exchange_code_for_token;
use crate::mcp::oauth::token_store::{save_token, TokenMetadata};
use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use url::Url;

#[derive(Clone)]
struct CallbackServerState {
	auth_state: Arc<Mutex<Option<String>>>,
	result_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<OAuthCallbackResult>>>>,
	shutdown: Arc<AtomicBool>,
	config: OAuthConfig,
	/// MCP config server name — the token-store key. Tokens are looked up by
	/// server name (get_valid_token), so they MUST be saved under it too; the
	/// client_id is ephemeral (CIMD URL / DCR-assigned) and never matches.
	server_name: String,
	code_verifier: String,
	redirect_uri: String,
}

#[derive(Debug, Clone)]
pub enum OAuthCallbackResult {
	Success {
		access_token: String,
		refresh_token: Option<String>,
		expires_in: u64,
		scopes: Vec<String>,
	},
	Error {
		error: String,
		description: Option<String>,
	},
	Cancelled,
	Timeout,
}

pub async fn start_callback_server(
	config: &OAuthConfig,
	server_name: &str,
	auth_state: String,
	code_verifier: String,
) -> Result<OAuthCallbackResult> {
	// Parse the configured callback_url to extract host and port
	let callback_url = &config.callback_url;
	let parsed_url = Url::parse(callback_url)
		.map_err(|e| anyhow!("Invalid callback_url '{}': {}", callback_url, e))?;

	let host = parsed_url
		.host_str()
		.ok_or_else(|| anyhow!("callback_url must have a host"))?;

	let port = parsed_url
		.port()
		.or_else(|| {
			// Default port based on scheme
			match parsed_url.scheme() {
				"http" => Some(80),
				"https" => Some(443),
				_ => None,
			}
		})
		.ok_or_else(|| anyhow!("callback_url must have a valid port"))?;

	// Bind to the configured host:port
	let listener = TcpListener::bind((host, port)).await?;

	// Use the exact callback_url as configured by user
	let redirect_uri = callback_url.clone();

	// Build authorization URL with the configured redirect_uri
	// Use the code_verifier passed to this function to generate the challenge
	let code_challenge = {
		use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
		use sha2::{Digest, Sha256};
		URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()))
	};
	let authorization_url = crate::mcp::oauth::build_authorization_url(
		config,
		&code_challenge,
		&auth_state,
		&redirect_uri,
	);

	let callback_state = CallbackServerState {
		auth_state: Arc::new(Mutex::new(Some(auth_state))),
		result_tx: Arc::new(Mutex::new(None)),
		shutdown: Arc::new(AtomicBool::new(false)),
		config: config.clone(),
		server_name: server_name.to_string(),
		code_verifier,
		redirect_uri: redirect_uri.clone(),
	};

	let (result_tx_channel, result_rx) = tokio::sync::oneshot::channel();

	{
		let mut tx = callback_state.result_tx.lock().await;
		*tx = Some(result_tx_channel);
	}

	let server_state = callback_state.clone();
	let server_handle = tokio::spawn(async move {
		run_http_server(&listener, server_state).await;
	});

	let auth_url_str = authorization_url.clone();
	open_browser(&authorization_url).map_err(|e| {
		anyhow!(
			"Failed to open browser: {}. Please manually visit: {}",
			e,
			auth_url_str
		)
	})?;

	let timeout_seconds = 300;
	let result = tokio::time::timeout(std::time::Duration::from_secs(timeout_seconds), async {
		result_rx
			.await
			.map_err(|e| anyhow!("Result channel closed: {}", e))
	})
	.await
	.map_err(|_| {
		callback_state.shutdown.store(true, Ordering::Relaxed);
		anyhow!("OAuth callback timed out after {} seconds", timeout_seconds)
	})?;

	callback_state.shutdown.store(true, Ordering::Relaxed);
	let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle).await;

	result.map_err(|e| anyhow!("Failed to receive OAuth result: {}", e))
}

async fn run_http_server(listener: &TcpListener, state: CallbackServerState) {
	loop {
		if state.shutdown.load(Ordering::Relaxed) {
			break;
		}

		let accept_result =
			tokio::time::timeout(std::time::Duration::from_secs(1), listener.accept()).await;

		match accept_result {
			Ok(Ok((stream, _addr))) => {
				let state_clone = state.clone();
				tokio::spawn(async move {
					let _ = handle_request(stream, state_clone).await;
				});
			}
			Ok(Err(e)) => tracing::debug!("Accept error: {}", e),
			Err(_) => continue,
		}
	}
}

async fn handle_request(
	mut stream: tokio::net::TcpStream,
	state: CallbackServerState,
) -> Result<()> {
	let mut buf = [0u8; 4096];
	let bytes_read = stream.read(&mut buf).await?;
	if bytes_read == 0 {
		return Ok(());
	}

	let request = String::from_utf8_lossy(&buf[..bytes_read]);

	// Parse the request line to extract path and query
	// Format: "GET /path?query HTTP/1.1"
	let request_line = match request.lines().next() {
		Some(line) => line.trim(),
		None => return Ok(()),
	};

	if request_line.starts_with("GET /oauth/callback") {
		// Extract query parameters - stop at HTTP protocol (space before HTTP/1.1)
		let query = if let Some(q_pos) = request_line.find('?') {
			let after_q = &request_line[q_pos + 1..];
			// Stop at space (end of query string, before HTTP/1.1)
			if let Some(space_pos) = after_q.find(' ') {
				&after_q[..space_pos]
			} else {
				after_q
			}
		} else {
			""
		};

		crate::log_debug!("OAuth callback query: {:?}", query);
		let result = process_callback(query, &state).await;

		let body = match &result {
			OAuthCallbackResult::Success { .. } => {
				"<html><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
				<h1 style='color: #28a745;'>OK - Authorization Successful!</h1>\
				<p>You can close this window and return to Octomind.</p></body></html>"
					.to_string()
			}
			OAuthCallbackResult::Error { error, description } => {
				format!(
					"<html><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
					<h1 style='color: #dc3545;'>ERROR - Authorization Failed</h1>\
					<p style='color: #dc3545;'>{}</p>\
					<p>{}</p></body></html>",
					error,
					description.as_deref().unwrap_or("")
				)
			}
			OAuthCallbackResult::Cancelled => {
				"<html><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
				<h1 style='color: #ffc107;'>WARNING - Authorization Cancelled</h1></body></html>"
					.to_string()
			}
			OAuthCallbackResult::Timeout => {
				"<html><body style='font-family: sans-serif; text-align: center; padding: 50px;'>\
				<h1 style='color: #6c757d;'>TIMEOUT - Authorization Timed Out</h1></body></html>"
					.to_string()
			}
		};

		let response = format!(
			"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
			body.len(),
			body
		);
		stream.write_all(response.as_bytes()).await?;

		let mut tx = state.result_tx.lock().await;
		if let Some(tx) = tx.take() {
			let _ = tx.send(result);
		}
	} else {
		let body = "<html><body><h1>404 Not Found</h1></body></html>";
		let response = format!(
			"HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{}",
			body.len(),
			body
		);
		stream.write_all(response.as_bytes()).await?;
	}

	Ok(())
}

async fn process_callback(query: &str, state: &CallbackServerState) -> OAuthCallbackResult {
	let mut code = None;
	let mut callback_state = None;
	let mut error = None;
	let mut error_description = None;

	for pair in query.split('&') {
		let parts: Vec<&str> = pair.splitn(2, '=').collect();
		if parts.len() == 2 {
			let key = parts[0];
			let value = urlencoding::decode(parts[1])
				.unwrap_or_default()
				.into_owned();
			crate::log_debug!("OAuth callback param: {} = {:?}", key, value);
			match key {
				"code" => code = Some(value),
				"state" => callback_state = Some(value),
				"error" => error = Some(value),
				"error_description" => error_description = Some(value),
				_ => {}
			}
		}
	}

	if let Some(e) = error {
		return OAuthCallbackResult::Error {
			error: e,
			description: error_description,
		};
	}

	let expected_state = state.auth_state.lock().await.take();

	match (callback_state, expected_state) {
		(Some(got), Some(expected)) if got.trim() == expected.trim() => {}
		(Some(got), Some(expected)) => {
			return OAuthCallbackResult::Error {
				error: "invalid_state".to_string(),
				description: Some(format!(
					"Expected: {}, Got: {} (len: {} vs {})",
					expected,
					got,
					expected.len(),
					got.len()
				)),
			};
		}
		(None, Some(_)) => {
			return OAuthCallbackResult::Error {
				error: "missing_state".to_string(),
				description: Some("State parameter missing from callback".to_string()),
			};
		}
		_ => {
			return OAuthCallbackResult::Error {
				error: "state_already_used".to_string(),
				description: Some("Callback already processed".to_string()),
			};
		}
	}

	let code = match code {
		Some(c) if !c.trim().is_empty() => c,
		_ => {
			return OAuthCallbackResult::Error {
				error: "missing_code".to_string(),
				description: Some("Authorization code missing from callback".to_string()),
			};
		}
	};

	match exchange_code_for_token(
		&state.config,
		&code,
		&state.code_verifier,
		&state.redirect_uri,
	)
	.await
	{
		Ok(token_response) => {
			// Clone all values before consuming the struct
			let refresh_token = token_response.refresh_token.clone();
			let scopes = token_response.scope.clone().unwrap_or_default();
			let access_token = token_response.access_token.clone();
			let expires_in = token_response.expires_in;

			// GitHub tokens don't expire, so use a far-future date if expires_in is 0
			let expires_at = if expires_in > 0 {
				std::time::SystemTime::now()
					.checked_add(std::time::Duration::from_secs(expires_in))
					.map(|t| {
						t.duration_since(std::time::UNIX_EPOCH)
							.unwrap_or_default()
							.as_secs()
					})
					.unwrap_or(0)
			} else {
				// GitHub tokens don't expire - set to 1 year from now
				std::time::SystemTime::now()
					.checked_add(std::time::Duration::from_secs(365 * 24 * 60 * 60))
					.map(|t| {
						t.duration_since(std::time::UNIX_EPOCH)
							.unwrap_or_default()
							.as_secs()
					})
					.unwrap_or(0)
			};

			let metadata = TokenMetadata {
				server_name: state.server_name.clone(),
				access_token: access_token.clone(),
				refresh_token: refresh_token.clone(),
				expires_at,
				scopes: scopes.clone(),
			};
			// Save under the server name — the key get_valid_token looks up.
			// A failed save means re-auth next time; say so instead of hiding it.
			if let Err(e) = save_token(&state.server_name, &metadata).await {
				crate::log_error!(
					"Failed to persist OAuth token for '{}' (re-auth will be required): {}",
					state.server_name,
					e
				);
			}

			OAuthCallbackResult::Success {
				access_token,
				refresh_token,
				expires_in: if expires_in > 0 {
					expires_in
				} else {
					365 * 24 * 60 * 60
				},
				scopes,
			}
		}
		Err(e) => OAuthCallbackResult::Error {
			error: "token_exchange_failed".to_string(),
			description: Some(format!("Failed to exchange code: {}", e)),
		},
	}
}

fn open_browser(url: &str) -> Result<()> {
	#[cfg(target_os = "macos")]
	{
		std::process::Command::new("open").arg(url).spawn()?;
	}
	#[cfg(target_os = "linux")]
	{
		std::process::Command::new("xdg-open").arg(url).spawn()?;
	}
	#[cfg(target_os = "windows")]
	{
		std::process::Command::new("cmd")
			.args(&["/c", "start", url])
			.spawn()?;
	}
	Ok(())
}

#[cfg(test)]
mod tests {
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
		let token_url =
			spawn_token_stub(r#"{"access_token":"gho_token","scope":"repo, user"}"#).await;
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
}
