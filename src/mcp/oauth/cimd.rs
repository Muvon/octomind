// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Client ID Metadata Documents (CIMD) and Dynamic Client Registration (DCR)
//!
//! Implements client_id resolution per the MCP Authorization specification:
//! 1. CIMD: Host a local HTTP endpoint serving client metadata, use its URL as client_id
//! 2. DCR: Register the client via the authorization server's registration_endpoint
//! 3. If neither is available, discovery fails with a clear error
//!
//! CIMD is preferred when the authorization server advertises
//! `client_id_metadata_document_supported: true` in its metadata.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::discovery::AuthServerMetadata;
use super::OAuthConfig;

/// Client metadata document served at `/.well-known/oauth-client.json`
///
/// This is the CIMD document that describes our OAuth client capabilities.
/// The URL where this document is hosted becomes the `client_id` value.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientMetadataDocument {
	/// Client name displayed to the user during authorization
	pub client_name: String,

	/// Redirect URIs where the auth server can send the user after authorization
	pub redirect_uris: Vec<String>,

	/// OAuth grant types this client supports
	pub grant_types: Vec<String>,

	/// Token endpoint authentication method (none = public client with PKCE)
	pub token_endpoint_auth_method: String,

	/// OAuth scopes this client requests
	#[serde(skip_serializing_if = "Option::is_none")]
	pub scope: Option<String>,
}

/// DCR registration response (RFC 7591)
#[derive(Debug, Deserialize)]
pub struct DcrRegistrationResponse {
	/// The assigned client_id
	pub client_id: String,

	/// Optional client_secret (for confidential clients)
	#[serde(default)]
	pub client_secret: Option<String>,

	/// Client ID issued at timestamp
	#[serde(default)]
	pub client_id_issued_at: Option<u64>,

	/// Client secret expires at timestamp (0 = never)
	#[serde(default)]
	pub client_secret_expires_at: Option<u64>,
}

// Global state for the CIMD server
lazy_static::lazy_static! {
	static ref CIMD_SERVER: Arc<Mutex<Option<CimdServerState>>> = Arc::new(Mutex::new(None));
}

#[allow(dead_code)]
struct CimdServerState {
	/// Shutdown signal
	shutdown: Arc<tokio::sync::Notify>,
}

/// Build the client metadata document for this Octomind instance
fn build_client_metadata(callback_url: &str, scopes: &[String]) -> ClientMetadataDocument {
	ClientMetadataDocument {
		client_name: "Octomind".to_string(),
		redirect_uris: vec![callback_url.to_string()],
		grant_types: vec!["authorization_code".to_string()],
		token_endpoint_auth_method: "none".to_string(),
		scope: if scopes.is_empty() {
			None
		} else {
			Some(scopes.join(" "))
		},
	}
}

/// Start the CIMD server on a local port, serving `/.well-known/oauth-client.json`
///
/// Returns the URL of the metadata document, which becomes the `client_id`.
///
/// # Arguments
/// * `callback_url` - The OAuth callback URL (used in redirect_uris)
/// * `scopes` - OAuth scopes to advertise
async fn start_cimd_server(callback_url: &str, scopes: &[String]) -> Result<String> {
	let metadata = build_client_metadata(callback_url, scopes);

	// Find a free port for the CIMD server
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
	let cimd_port = listener
		.local_addr()
		.context("Failed to get CIMD server port")?
		.port();

	let client_id_url = format!(
		"http://localhost:{}/.well-known/oauth-client.json",
		cimd_port
	);

	crate::log_debug!("Starting CIMD server at {} (client_id URL)", client_id_url);

	let shutdown = Arc::new(tokio::sync::Notify::new());
	let shutdown_clone = shutdown.clone();
	let metadata_clone = metadata.clone();

	// Spawn the CIMD HTTP server
	tokio::spawn(async move {
		run_cimd_server(&listener, &metadata_clone, shutdown_clone).await;
	});

	// Store server state for cleanup
	let mut state = CIMD_SERVER.lock().await;
	*state = Some(CimdServerState { shutdown });

	Ok(client_id_url)
}

/// Stop the CIMD server if running
pub async fn stop_cimd_server() {
	let mut state = CIMD_SERVER.lock().await;
	if let Some(s) = state.take() {
		s.shutdown.notify_one();
		crate::log_debug!("CIMD server stopped");
	}
}

/// Run the CIMD HTTP server, serving the client metadata document
async fn run_cimd_server(
	listener: &tokio::net::TcpListener,
	metadata: &ClientMetadataDocument,
	shutdown: Arc<tokio::sync::Notify>,
) {
	let metadata_json = serde_json::to_string(metadata).unwrap_or_default();

	loop {
		tokio::select! {
			result = listener.accept() => {
				match result {
					Ok((stream, _addr)) => {
						let json = metadata_json.clone();
						tokio::spawn(async move {
							if let Err(e) = handle_cimd_request(stream, &json).await {
								crate::log_debug!("CIMD request error: {}", e);
							}
						});
					}
					Err(e) => {
						crate::log_debug!("CIMD accept error: {}", e);
					}
				}
			}
			_ = shutdown.notified() => {
				crate::log_debug!("CIMD server shutting down");
				break;
			}
		}
	}
}

/// Handle a single CIMD HTTP request
async fn handle_cimd_request(mut stream: tokio::net::TcpStream, metadata_json: &str) -> Result<()> {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	let mut buf = [0u8; 4096];
	let bytes_read = stream.read(&mut buf).await?;
	if bytes_read == 0 {
		return Ok(());
	}

	let request = String::from_utf8_lossy(&buf[..bytes_read]);
	let request_line = match request.lines().next() {
		Some(line) => line.trim(),
		None => return Ok(()),
	};

	// CORS headers for cross-origin metadata document access
	let cors_headers = "Access-Control-Allow-Origin: *\r\n\
	                    Access-Control-Allow-Methods: GET, OPTIONS\r\n\
	                    Access-Control-Allow-Headers: Content-Type\r\n";

	if request_line.starts_with("OPTIONS") {
		let response = format!("HTTP/1.1 204 No Content\r\n{}\r\n", cors_headers);
		stream.write_all(response.as_bytes()).await?;
	} else if request_line.starts_with("GET /.well-known/oauth-client.json") {
		let response = format!(
			"HTTP/1.1 200 OK\r\n\
			 Content-Type: application/json\r\n\
			 Content-Length: {}\r\n\
			 {}\r\n{}",
			metadata_json.len(),
			cors_headers,
			metadata_json
		);
		stream.write_all(response.as_bytes()).await?;
	} else {
		let body = "404 Not Found";
		let response = format!(
			"HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{}",
			body.len(),
			body
		);
		stream.write_all(response.as_bytes()).await?;
	}

	Ok(())
}

/// Register the client via Dynamic Client Registration (RFC 7591)
///
/// # Arguments
/// * `registration_endpoint` - The DCR endpoint URL from auth server metadata
/// * `callback_url` - The OAuth callback URL
/// * `scopes` - OAuth scopes to request
async fn register_via_dcr(
	registration_endpoint: &str,
	callback_url: &str,
	scopes: &[String],
) -> Result<DcrRegistrationResponse> {
	crate::log_debug!("Registering client via DCR at: {}", registration_endpoint);

	let client_metadata = build_client_metadata(callback_url, scopes);

	let client = reqwest::Client::builder()
		.timeout(std::time::Duration::from_secs(10))
		.build()
		.context("Failed to create HTTP client for DCR")?;

	let response = client
		.post(registration_endpoint)
		.header("Content-Type", "application/json")
		.json(&client_metadata)
		.send()
		.await
		.context(format!(
			"Failed to register client at {}",
			registration_endpoint
		))?;

	if !response.status().is_success() {
		let status = response.status();
		let body = response.text().await.unwrap_or_default();
		return Err(anyhow!(
			"DCR registration failed with status {}: {}",
			status,
			body
		));
	}

	let dcr_response: DcrRegistrationResponse = response
		.json()
		.await
		.context("Failed to parse DCR registration response")?;

	crate::log_debug!(
		"DCR registration successful: client_id={}",
		dcr_response.client_id
	);

	Ok(dcr_response)
}

/// Resolve the client_id for an OAuth configuration using CIMD or DCR.
///
/// # Strategy (per MCP Authorization spec):
/// 1. If auth server supports CIMD (`client_id_metadata_document_supported: true`),
///    start a local CIMD server and use its URL as the client_id
/// 2. If auth server provides a `registration_endpoint`, use DCR (RFC 7591)
/// 3. If neither is available, return an error — no fallback to hardcoded IDs
///
/// # Arguments
/// * `oauth_config` - The OAuth config with empty client_id (will be populated)
/// * `auth_metadata` - Authorization server metadata (drives CIMD/DCR decision)
///
/// # Returns
/// * `Ok(OAuthConfig)` - Config with resolved client_id
/// * `Err` - If neither CIMD nor DCR is available
pub async fn resolve_client_id(
	oauth_config: OAuthConfig,
	auth_metadata: &AuthServerMetadata,
) -> Result<OAuthConfig> {
	// Strategy 1: CIMD — if auth server supports it, use metadata document URL as client_id
	if auth_metadata
		.client_id_metadata_document_supported
		.unwrap_or(false)
	{
		crate::log_debug!("Auth server supports CIMD, starting local metadata server...");

		match start_cimd_server(&oauth_config.callback_url, &oauth_config.scopes).await {
			Ok(client_id_url) => {
				crate::log_debug!("CIMD server started, using client_id: {}", client_id_url);
				return Ok(OAuthConfig {
					client_id: client_id_url,
					..oauth_config
				});
			}
			Err(e) => {
				crate::log_debug!(
					"CIMD server failed: {}, falling back to DCR if available",
					e
				);
				// Fall through to DCR
			}
		}
	}

	// Strategy 2: DCR — register client via registration_endpoint
	if let Some(ref registration_endpoint) = auth_metadata.registration_endpoint {
		crate::log_debug!(
			"Auth server provides DCR endpoint: {}",
			registration_endpoint
		);

		match register_via_dcr(
			registration_endpoint,
			&oauth_config.callback_url,
			&oauth_config.scopes,
		)
		.await
		{
			Ok(dcr_response) => {
				return Ok(OAuthConfig {
					client_id: dcr_response.client_id,
					client_secret: dcr_response.client_secret.unwrap_or_default(),
					..oauth_config
				});
			}
			Err(e) => {
				return Err(anyhow!(
					"DCR registration failed: {}. Auth server at {} provides registration_endpoint but registration failed.",
					e,
					auth_metadata.issuer
				));
			}
		}
	}

	// Neither CIMD nor DCR available — cannot proceed
	Err(anyhow!(
		"Cannot resolve OAuth client_id: auth server '{}' does not support CIMD \
		 (client_id_metadata_document_supported not true) and provides no \
		 registration_endpoint for DCR. MCP Authorization requires one of these \
		 mechanisms to obtain a client_id.",
		auth_metadata.issuer
	))
}

#[cfg(test)]
#[path = "cimd_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "cimd_tests.rs"]
mod cimd_tests;
