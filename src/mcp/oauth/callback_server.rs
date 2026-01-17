// Copyright 2025 Muvon Un Limited
//
// OAuth 2.1 Callback Server

use crate::config::OAuthConfig;
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
				server_name: state.config.client_id.clone(),
				access_token: access_token.clone(),
				refresh_token: refresh_token.clone(),
				expires_at,
				scopes: scopes.clone(),
			};
			let _ = save_token(&state.config.client_id, &metadata).await;

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
