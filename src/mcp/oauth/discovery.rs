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

//! MCP Authorization Discovery (RFC 9728)
//!
//! Implements automatic OAuth configuration discovery for MCP servers following:
//! - RFC 9728: OAuth 2.0 Protected Resource Metadata
//! - RFC 8414: OAuth 2.0 Authorization Server Metadata Discovery
//!
//! Flow:
//! 1. Try pre-discovery: GET {server_url}/.well-known/oauth-protected-resource
//! 2. If no pre-discovery, make request to MCP server → expect 401 Unauthorized
//! 3. Parse WWW-Authenticate header for resource_metadata URL
//! 4. Fetch Protected Resource Metadata document
//! 5. Extract authorization_servers[0] (primary auth server)
//! 6. Fetch Authorization Server Metadata from {issuer}/.well-known/oauth-authorization-server
//! 7. Build OAuthConfig from discovered endpoints (client_id from CIMD/DCR)

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use reqwest;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use super::cimd::resolve_client_id;
use super::OAuthConfig;

// Cache for discovered OAuth configurations to avoid repeated discovery
// Key: server_name, Value: discovered OAuthConfig
lazy_static::lazy_static! {
	static ref DISCOVERED_OAUTH_CACHE: RwLock<HashMap<String, OAuthConfig>> = RwLock::new(HashMap::new());
}

/// Check if a server has a cached OAuth discovery configuration
pub fn has_cached_discovery(server_name: &str) -> bool {
	DISCOVERED_OAUTH_CACHE
		.read()
		.map(|cache| cache.contains_key(server_name))
		.unwrap_or(false)
}

/// Protected Resource Metadata (RFC 9728)
/// Describes the OAuth requirements for a protected resource (MCP server)
#[derive(Debug, Deserialize)]
pub struct ProtectedResourceMetadata {
	/// The protected resource identifier
	pub resource: String,

	/// List of authorization servers that can issue tokens for this resource
	/// First entry is the primary authorization server
	pub authorization_servers: Vec<String>,

	/// Optional list of OAuth scopes supported by this resource
	#[serde(default)]
	pub scopes_supported: Option<Vec<String>>,
}

/// Authorization Server Metadata (RFC 8414)
/// Describes the OAuth endpoints and capabilities of an authorization server
#[derive(Debug, Deserialize)]
pub struct AuthServerMetadata {
	/// The authorization server's issuer identifier
	pub issuer: String,

	/// URL of the authorization endpoint (for user authorization)
	pub authorization_endpoint: String,

	/// URL of the token endpoint (for token exchange)
	pub token_endpoint: String,

	/// Optional list of OAuth scopes supported by this server
	#[serde(default)]
	pub scopes_supported: Option<Vec<String>>,

	/// Optional list of PKCE code challenge methods supported
	#[serde(default)]
	pub code_challenge_methods_supported: Option<Vec<String>>,

	/// Optional URL for Dynamic Client Registration (RFC 7591)
	#[serde(default)]
	pub registration_endpoint: Option<String>,

	/// Whether the authorization server supports Client ID Metadata Documents (CIMD)
	/// When true, client_id can be a URL pointing to a client metadata document.
	#[serde(default)]
	pub client_id_metadata_document_supported: Option<bool>,
}

/// Parse WWW-Authenticate header to extract resource_metadata URL
///
/// Expected format: `Bearer resource_metadata="https://example.com/.well-known/oauth-protected-resource"`
///
/// # Arguments
/// * `header_value` - The WWW-Authenticate header value
///
/// # Returns
/// * `Ok(String)` - The resource_metadata URL
/// * `Err` - If header format is invalid or URL not found
pub fn parse_www_authenticate_header(header_value: &str) -> Result<String> {
	// Pattern: resource_metadata="<URL>"
	let re = Regex::new(r#"resource_metadata="([^"]+)""#)
		.context("Failed to compile regex for WWW-Authenticate parsing")?;

	let captures = re.captures(header_value).ok_or_else(|| {
		anyhow!(
			"WWW-Authenticate header does not contain resource_metadata URL. Header: {}",
			header_value
		)
	})?;

	let url = captures
		.get(1)
		.ok_or_else(|| anyhow!("Failed to extract resource_metadata URL from captures"))?
		.as_str()
		.to_string();

	crate::log_debug!("Extracted resource_metadata URL: {}", url);
	Ok(url)
}

/// Fetch Protected Resource Metadata from the given URL
///
/// # Arguments
/// * `metadata_url` - URL to the protected resource metadata document
///
/// # Returns
/// * `Ok(ProtectedResourceMetadata)` - Parsed metadata
/// * `Err` - If request fails or JSON parsing fails
pub async fn fetch_protected_resource_metadata(
	metadata_url: &str,
) -> Result<ProtectedResourceMetadata> {
	crate::log_debug!(
		"Fetching Protected Resource Metadata from: {}",
		metadata_url
	);

	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(10))
		.build()
		.context("Failed to create HTTP client")?;

	let response = client.get(metadata_url).send().await.context(format!(
		"Failed to fetch Protected Resource Metadata from {}",
		metadata_url
	))?;

	if !response.status().is_success() {
		return Err(anyhow!(
			"Protected Resource Metadata request failed with status: {}",
			response.status()
		));
	}

	let metadata: ProtectedResourceMetadata = response
		.json()
		.await
		.context("Failed to parse Protected Resource Metadata JSON")?;

	crate::log_debug!(
		"Protected Resource Metadata: resource={}, auth_servers={:?}",
		metadata.resource,
		metadata.authorization_servers
	);

	Ok(metadata)
}

/// Fetch Authorization Server Metadata via RFC 8414 discovery
///
/// GET {issuer}/.well-known/oauth-authorization-server
///
/// # Arguments
/// * `issuer` - The authorization server issuer URL
///
/// # Returns
/// * `Ok(AuthServerMetadata)` - Discovered metadata
/// * `Err` - If RFC 8414 discovery fails
pub async fn fetch_auth_server_metadata(issuer: &str) -> Result<AuthServerMetadata> {
	let issuer_trimmed = issuer.trim_end_matches('/');

	// RFC 8414: Authorization server metadata is at
	// {issuer}/.well-known/oauth-authorization-server
	let metadata_url = format!("{}/.well-known/oauth-authorization-server", issuer_trimmed);

	crate::log_debug!(
		"Fetching Authorization Server Metadata from: {}",
		metadata_url
	);

	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(10))
		.build()
		.context("Failed to create HTTP client")?;

	let response = client.get(&metadata_url).send().await.context(format!(
		"Failed to fetch Authorization Server Metadata from {}",
		metadata_url
	))?;

	if !response.status().is_success() {
		return Err(anyhow!(
			"Authorization Server Metadata request failed with status: {} (RFC 8414 discovery at {})",
			response.status(),
			metadata_url
		));
	}

	let metadata: AuthServerMetadata = response
		.json()
		.await
		.context("Failed to parse Authorization Server Metadata JSON")?;

	crate::log_debug!(
		"Authorization Server Metadata: issuer={}, auth_endpoint={}, token_endpoint={}",
		metadata.issuer,
		metadata.authorization_endpoint,
		metadata.token_endpoint
	);

	Ok(metadata)
}

/// Build OAuthConfig from discovered metadata
///
/// # Arguments
/// * `auth_metadata` - Authorization Server Metadata
/// * `resource_metadata` - Protected Resource Metadata
///
/// # Returns
/// * `OAuthConfig` - Ready-to-use OAuth configuration
///
/// Note: client_id is set to a placeholder. It must be resolved via CIMD or DCR
/// before the OAuth flow can proceed. See cimd.rs for CIMD/DCR resolution.
pub fn build_oauth_config_from_metadata(
	auth_metadata: &AuthServerMetadata,
	resource_metadata: &ProtectedResourceMetadata,
) -> OAuthConfig {
	// Combine scopes from both metadata documents
	let scopes = resource_metadata
		.scopes_supported
		.as_ref()
		.or(auth_metadata.scopes_supported.as_ref())
		.cloned()
		.unwrap_or_default();

	crate::log_debug!("Building OAuthConfig: scopes={:?}", scopes);

	OAuthConfig {
		client_id: String::new(), // Placeholder — resolved by CIMD/DCR
		client_secret: String::new(),
		authorization_url: auth_metadata.authorization_endpoint.clone(),
		token_url: auth_metadata.token_endpoint.clone(),
		callback_url: "http://localhost:34567/oauth/callback".to_string(),
		scopes,
		state: None,
		refresh_buffer_seconds: 300,
		resource: Some(resource_metadata.resource.clone()),
	}
}

/// Discover OAuth configuration from MCP server using RFC 9728 flow
///
/// This is the main entry point for MCP Authorization discovery.
/// Results are cached per server to avoid repeated discovery attempts.
///
/// # Flow
/// 1. Check cache for previously discovered config
/// 2. Try pre-discovery: GET {server_url}/.well-known/oauth-protected-resource
/// 3. If no pre-discovery, make request to MCP server → expect 401
/// 4. Parse WWW-Authenticate header for resource_metadata URL
/// 5. Fetch Protected Resource Metadata
/// 6. Extract primary authorization server
/// 7. Fetch Authorization Server Metadata via RFC 8414
/// 8. Build OAuthConfig from discovered endpoints
/// 9. Cache the result for future use
///
/// # Arguments
/// * `server_url` - The MCP server URL (e.g., "https://api.githubcopilot.com/mcp/")
/// * `server_name` - The server name for logging and caching
///
/// # Returns
/// * `Ok(OAuthConfig)` - Discovered OAuth configuration (from cache or fresh discovery)
/// * `Err` - If discovery fails at any step
pub async fn discover_oauth_from_mcp_server(
	server_url: &str,
	server_name: &str,
) -> Result<OAuthConfig> {
	// Check cache first to avoid repeated discovery
	{
		let cache = DISCOVERED_OAUTH_CACHE.read().unwrap();
		if let Some(cached_config) = cache.get(server_name) {
			crate::log_debug!(
				"Using cached OAuth config for server '{}' (skipping discovery)",
				server_name
			);
			return Ok(cached_config.clone());
		}
	}

	crate::log_debug!(
		"Starting MCP Authorization discovery for server '{}' at {}",
		server_name,
		server_url
	);

	// Create HTTP client with timeout
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(10))
		.build()
		.context("Failed to create HTTP client for MCP discovery")?;

	// Step 1: Try pre-discovery via .well-known endpoint
	// RFC 9728: Protected resource metadata may be available without auth
	let server_url_trimmed = server_url.trim_end_matches('/');
	let pre_discovery_url = format!(
		"{}/.well-known/oauth-protected-resource",
		server_url_trimmed
	);

	crate::log_debug!("Trying pre-discovery at: {}", pre_discovery_url);

	let resource_metadata = match fetch_protected_resource_metadata(&pre_discovery_url).await {
		Ok(metadata) => {
			crate::log_debug!("Pre-discovery successful for server '{}'", server_name);
			Some(metadata)
		}
		Err(e) => {
			crate::log_debug!(
				"Pre-discovery failed for server '{}': {}, falling back to 401 flow",
				server_name,
				e
			);
			None
		}
	};

	// Step 2: If pre-discovery failed, make initial request expecting 401
	let resource_metadata = match resource_metadata {
		Some(m) => m,
		None => {
			crate::log_debug!("Making initial JSON-RPC request to MCP server (expecting 401)...");

			// Create a tools/list JSON-RPC request (same as health check)
			let jsonrpc_request = serde_json::json!({
				"jsonrpc": "2.0",
				"id": 1,
				"method": "tools/list",
				"params": {}
			});

			let response = client
				.post(server_url)
				.header("Content-Type", "application/json")
				.json(&jsonrpc_request)
				.send()
				.await
				.context(format!("Failed to connect to MCP server at {}", server_url))?;

			// Check for 401 Unauthorized
			if response.status() != reqwest::StatusCode::UNAUTHORIZED {
				return Err(anyhow!(
					"MCP Authorization discovery requires 401 Unauthorized response, got: {}. \
                    Server may not support MCP Authorization (RFC 9728).",
					response.status()
				));
			}

			crate::log_debug!("Received 401 Unauthorized, proceeding with discovery...");

			// Extract WWW-Authenticate header
			let www_auth_header = response
				.headers()
				.get("WWW-Authenticate")
				.ok_or_else(|| {
					anyhow!(
						"MCP server returned 401 but missing WWW-Authenticate header. \
                        Server does not support MCP Authorization (RFC 9728)."
					)
				})?
				.to_str()
				.context("WWW-Authenticate header contains invalid UTF-8")?;

			crate::log_debug!("WWW-Authenticate header: {}", www_auth_header);

			// Parse resource_metadata URL
			let resource_metadata_url = parse_www_authenticate_header(www_auth_header)
				.context("Failed to parse WWW-Authenticate header")?;

			// Fetch Protected Resource Metadata
			fetch_protected_resource_metadata(&resource_metadata_url)
				.await
				.context("Failed to fetch Protected Resource Metadata")?
		}
	};

	// Step 3: Extract primary authorization server
	let auth_server_issuer = resource_metadata
		.authorization_servers
		.first()
		.ok_or_else(|| anyhow!("Protected Resource Metadata contains no authorization servers"))?;

	crate::log_debug!("Using authorization server: {}", auth_server_issuer);

	// Step 4: Fetch Authorization Server Metadata via RFC 8414
	let auth_metadata = fetch_auth_server_metadata(auth_server_issuer)
		.await
		.context("Failed to fetch Authorization Server Metadata via RFC 8414")?;

	// Step 5: Build OAuthConfig from discovered metadata (client_id is placeholder)
	let oauth_config = build_oauth_config_from_metadata(&auth_metadata, &resource_metadata);

	// Step 6: Resolve client_id via CIMD or DCR
	let oauth_config = resolve_client_id(oauth_config, &auth_metadata)
		.await
		.context("Failed to resolve OAuth client_id via CIMD/DCR")?;

	crate::log_debug!(
		"MCP Authorization discovery completed successfully for '{}' (client_id: {})",
		server_name,
		if oauth_config.client_id.len() > 50 {
			format!("{}...", &oauth_config.client_id[..50])
		} else {
			oauth_config.client_id.clone()
		}
	);

	// Cache the discovered config for future use
	{
		let mut cache = DISCOVERED_OAUTH_CACHE.write().unwrap();
		cache.insert(server_name.to_string(), oauth_config.clone());
		crate::log_debug!(
			"Cached OAuth config for server '{}' to avoid repeated discovery",
			server_name
		);
	}

	Ok(oauth_config)
}

/// Clear cached OAuth discovery for a specific server
///
/// Useful when OAuth configuration changes or for manual reset
///
/// # Arguments
/// * `server_name` - The server name to clear from cache
pub fn clear_discovered_oauth_cache(server_name: &str) {
	let mut cache = DISCOVERED_OAUTH_CACHE.write().unwrap();
	if cache.remove(server_name).is_some() {
		crate::log_debug!("Cleared cached OAuth config for server '{}'", server_name);
	}
}

/// Clear all cached OAuth discoveries
///
/// Useful for cleanup or forcing fresh discovery for all servers
pub fn clear_all_discovered_oauth_cache() {
	let mut cache = DISCOVERED_OAUTH_CACHE.write().unwrap();
	let count = cache.len();
	cache.clear();
	crate::log_debug!("Cleared all {} cached OAuth configs", count);
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_www_authenticate_header() {
		let header = r#"Bearer resource_metadata="https://api.example.com/.well-known/oauth-protected-resource""#;
		let result = parse_www_authenticate_header(header).unwrap();
		assert_eq!(
			result,
			"https://api.example.com/.well-known/oauth-protected-resource"
		);
	}

	#[test]
	fn test_parse_www_authenticate_header_invalid() {
		let header = "Bearer realm=\"example\"";
		let result = parse_www_authenticate_header(header);
		assert!(result.is_err());
	}

	#[test]
	fn test_build_oauth_config() {
		let auth_metadata = AuthServerMetadata {
			issuer: "https://api.example.com".to_string(),
			authorization_endpoint: "https://api.example.com/oauth/authorize".to_string(),
			token_endpoint: "https://api.example.com/oauth/token".to_string(),
			scopes_supported: Some(vec!["read".to_string(), "write".to_string()]),
			code_challenge_methods_supported: Some(vec!["S256".to_string()]),
			registration_endpoint: None,
			client_id_metadata_document_supported: None,
		};

		let resource_metadata = ProtectedResourceMetadata {
			resource: "https://api.example.com".to_string(),
			authorization_servers: vec!["https://api.example.com".to_string()],
			scopes_supported: None,
		};

		let config = build_oauth_config_from_metadata(&auth_metadata, &resource_metadata);

		// client_id is empty placeholder — resolved by CIMD/DCR
		assert!(config.client_id.is_empty());
		assert_eq!(
			config.authorization_url,
			"https://api.example.com/oauth/authorize"
		);
		assert_eq!(config.token_url, "https://api.example.com/oauth/token");
		assert_eq!(config.scopes, vec!["read", "write"]);
		// Public client - no secret
		assert!(config.client_secret.is_empty());
	}

	// ------------------------------------------------------------------
	// WWW-Authenticate header edge cases
	// ------------------------------------------------------------------

	#[test]
	fn test_parse_header_edge_cases() {
		// Empty header
		assert!(parse_www_authenticate_header("").is_err());
		// Multiple params — resource_metadata still extracted
		let header = "Bearer realm=\"example\", resource_metadata=\"https://auth.example.com/.well-known/oauth-protected-resource\"";
		assert_eq!(
			parse_www_authenticate_header(header).unwrap(),
			"https://auth.example.com/.well-known/oauth-protected-resource"
		);
		// Unquoted value does not match the RFC 9728 format
		assert!(
			parse_www_authenticate_header("Bearer resource_metadata=https://example.com/prm")
				.is_err()
		);
		// Empty quoted value does not match
		assert!(parse_www_authenticate_header("Bearer resource_metadata=\"\"").is_err());
		// URL with query string survives intact
		let header = "Bearer resource_metadata=\"https://example.com/prm?foo=bar&baz=1\"";
		assert_eq!(
			parse_www_authenticate_header(header).unwrap(),
			"https://example.com/prm?foo=bar&baz=1"
		);
	}

	// ------------------------------------------------------------------
	// Metadata struct deserialization
	// ------------------------------------------------------------------

	#[test]
	fn test_protected_resource_metadata_deserialization() {
		let minimal: ProtectedResourceMetadata = serde_json::from_str(
			r#"{"resource":"https://api.example.com","authorization_servers":["https://auth.example.com"]}"#,
		)
		.unwrap();
		assert_eq!(minimal.resource, "https://api.example.com");
		assert_eq!(
			minimal.authorization_servers,
			vec!["https://auth.example.com".to_string()]
		);
		assert!(minimal.scopes_supported.is_none(), "scopes are optional");

		let full: ProtectedResourceMetadata = serde_json::from_str(
			r#"{"resource":"https://api.example.com","authorization_servers":["https://a.example.com","https://b.example.com"],"scopes_supported":["mcp:read","mcp:write"]}"#,
		)
		.unwrap();
		assert_eq!(
			full.scopes_supported,
			Some(vec!["mcp:read".to_string(), "mcp:write".to_string()])
		);
		assert_eq!(full.authorization_servers.len(), 2);

		// authorization_servers is required
		assert!(serde_json::from_str::<ProtectedResourceMetadata>(
			r#"{"resource":"https://api.example.com"}"#
		)
		.is_err());
	}

	#[test]
	fn test_auth_server_metadata_deserialization() {
		let minimal: AuthServerMetadata = serde_json::from_str(
			r#"{"issuer":"https://auth.example.com","authorization_endpoint":"https://auth.example.com/authorize","token_endpoint":"https://auth.example.com/token"}"#,
		)
		.unwrap();
		assert_eq!(minimal.issuer, "https://auth.example.com");
		assert!(minimal.scopes_supported.is_none());
		assert!(minimal.code_challenge_methods_supported.is_none());
		assert!(minimal.registration_endpoint.is_none());
		assert!(minimal.client_id_metadata_document_supported.is_none());

		let full: AuthServerMetadata = serde_json::from_str(
			r#"{"issuer":"https://auth.example.com","authorization_endpoint":"https://auth.example.com/authorize","token_endpoint":"https://auth.example.com/token","scopes_supported":["read"],"code_challenge_methods_supported":["S256"],"registration_endpoint":"https://auth.example.com/register","client_id_metadata_document_supported":true}"#,
		)
		.unwrap();
		assert_eq!(full.scopes_supported, Some(vec!["read".to_string()]));
		assert_eq!(
			full.code_challenge_methods_supported,
			Some(vec!["S256".to_string()])
		);
		assert_eq!(
			full.registration_endpoint.as_deref(),
			Some("https://auth.example.com/register")
		);
		assert_eq!(full.client_id_metadata_document_supported, Some(true));

		// token_endpoint is required
		assert!(serde_json::from_str::<AuthServerMetadata>(
			r#"{"issuer":"https://auth.example.com","authorization_endpoint":"https://auth.example.com/authorize"}"#
		)
		.is_err());
	}

	// ------------------------------------------------------------------
	// build_oauth_config_from_metadata — scope precedence and defaults
	// ------------------------------------------------------------------

	fn auth_metadata_fixture(
		scopes: Option<Vec<String>>,
		registration_endpoint: Option<String>,
		cimd_supported: Option<bool>,
	) -> AuthServerMetadata {
		AuthServerMetadata {
			issuer: "https://auth.example.com".to_string(),
			authorization_endpoint: "https://auth.example.com/oauth/authorize".to_string(),
			token_endpoint: "https://auth.example.com/oauth/token".to_string(),
			scopes_supported: scopes,
			code_challenge_methods_supported: Some(vec!["S256".to_string()]),
			registration_endpoint,
			client_id_metadata_document_supported: cimd_supported,
		}
	}

	fn resource_metadata_fixture(scopes: Option<Vec<String>>) -> ProtectedResourceMetadata {
		ProtectedResourceMetadata {
			resource: "https://api.example.com".to_string(),
			authorization_servers: vec!["https://auth.example.com".to_string()],
			scopes_supported: scopes,
		}
	}

	#[test]
	fn test_build_oauth_config_scope_precedence() {
		let auth = auth_metadata_fixture(Some(vec!["read".to_string()]), None, None);

		// Resource scopes win over auth server scopes
		let config = build_oauth_config_from_metadata(
			&auth,
			&resource_metadata_fixture(Some(vec!["mcp:read".to_string()])),
		);
		assert_eq!(config.scopes, vec!["mcp:read".to_string()]);

		// Falls back to auth server scopes when the resource lists none
		let config = build_oauth_config_from_metadata(&auth, &resource_metadata_fixture(None));
		assert_eq!(config.scopes, vec!["read".to_string()]);

		// Empty when neither document lists scopes
		let auth_no_scopes = auth_metadata_fixture(None, None, None);
		let config =
			build_oauth_config_from_metadata(&auth_no_scopes, &resource_metadata_fixture(None));
		assert!(config.scopes.is_empty());
	}

	#[test]
	fn test_build_oauth_config_defaults() {
		let auth = auth_metadata_fixture(
			None,
			Some("https://auth.example.com/register".to_string()),
			Some(true),
		);
		let config = build_oauth_config_from_metadata(&auth, &resource_metadata_fixture(None));

		// client_id is a placeholder resolved later by CIMD/DCR
		assert!(config.client_id.is_empty());
		assert!(config.client_secret.is_empty());
		assert_eq!(
			config.authorization_url,
			"https://auth.example.com/oauth/authorize"
		);
		assert_eq!(config.token_url, "https://auth.example.com/oauth/token");
		assert_eq!(config.callback_url, "http://localhost:34567/oauth/callback");
		assert_eq!(config.refresh_buffer_seconds, 300);
		assert!(config.state.is_none());
		assert_eq!(config.resource.as_deref(), Some("https://api.example.com"));
	}

	// ------------------------------------------------------------------
	// Cache management
	// ------------------------------------------------------------------

	#[test]
	#[serial_test::serial]
	fn test_cache_clear_helpers_never_panic() {
		// Clearing an untouched entry and an empty cache must be safe
		clear_discovered_oauth_cache("never-populated-server");
		clear_all_discovered_oauth_cache();
		assert!(!has_cached_discovery("never-populated-server"));
	}

	// ------------------------------------------------------------------
	// HTTP stub for RFC 9728 / RFC 8414 discovery flows
	// ------------------------------------------------------------------

	/// Spawn a local HTTP server routing (method, path, host) to a response.
	/// Returns the base URL, e.g. "http://127.0.0.1:12345".
	async fn spawn_http_stub<F>(handler: F) -> String
	where
		F: Fn(&str, &str, &str) -> (u16, Vec<(String, String)>, String) + Send + Sync + 'static,
	{
		use tokio::io::{AsyncReadExt, AsyncWriteExt};

		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let handler = std::sync::Arc::new(handler);
		tokio::spawn(async move {
			loop {
				let Ok((mut sock, _)) = listener.accept().await else {
					break;
				};
				let handler = handler.clone();
				tokio::spawn(async move {
					let mut buf = vec![0u8; 16384];
					let n = match sock.read(&mut buf).await {
						Ok(n) if n > 0 => n,
						_ => return,
					};
					let request = String::from_utf8_lossy(&buf[..n]).to_string();
					let mut parts = request.lines().next().unwrap_or("").split_whitespace();
					let method = parts.next().unwrap_or("").to_string();
					let path = parts.next().unwrap_or("").to_string();
					let host = request
						.lines()
						.find_map(|l| {
							let (key, value) = l.split_once(':')?;
							key.eq_ignore_ascii_case("host")
								.then(|| value.trim().to_string())
						})
						.unwrap_or_default();
					let (status, headers, body) = handler(&method, &path, &host);
					let reason = match status {
						200 => "OK",
						401 => "Unauthorized",
						404 => "Not Found",
						_ => "Error",
					};
					let mut response = format!(
						"HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
						body.len()
					);
					for (name, value) in &headers {
						response.push_str(&format!("{name}: {value}\r\n"));
					}
					response.push_str("\r\n");
					response.push_str(&body);
					let _ = sock.write_all(response.as_bytes()).await;
				});
			}
		});
		format!("http://{addr}")
	}

	fn unique_server_name(prefix: &str) -> String {
		format!("{prefix}-{}", uuid::Uuid::new_v4())
	}

	#[tokio::test]
	async fn test_fetch_protected_resource_metadata_success_and_errors() {
		let body =
			r#"{"resource":"https://api.example.com","authorization_servers":["https://auth.example.com"],"scopes_supported":["mcp"]}"#
				.to_string();
		let url = spawn_http_stub(move |_m, _p, _h| (200, vec![], body.clone())).await;
		let metadata = fetch_protected_resource_metadata(&url).await.unwrap();
		assert_eq!(metadata.resource, "https://api.example.com");
		assert_eq!(metadata.scopes_supported, Some(vec!["mcp".to_string()]));

		// Non-success status is an error naming the status
		let url = spawn_http_stub(move |_m, _p, _h| (404, vec![], String::new())).await;
		let err = fetch_protected_resource_metadata(&url).await.unwrap_err();
		assert!(err.to_string().contains("status"), "{err}");

		// Invalid JSON body is a parse error
		let url = spawn_http_stub(move |_m, _p, _h| (200, vec![], "not json".to_string())).await;
		let err = fetch_protected_resource_metadata(&url).await.unwrap_err();
		assert!(err.to_string().contains("parse"), "{err}");
	}

	#[tokio::test]
	async fn test_fetch_auth_server_metadata_builds_rfc8414_url() {
		let body =
			r#"{"issuer":"https://auth.example.com","authorization_endpoint":"https://auth.example.com/authorize","token_endpoint":"https://auth.example.com/token"}"#
				.to_string();
		let url = spawn_http_stub(move |method, path, _host| {
			if method == "GET" && path == "/.well-known/oauth-authorization-server" {
				(200, vec![], body.clone())
			} else {
				(404, vec![], String::new())
			}
		})
		.await;

		// Trailing slash on the issuer is trimmed before building the well-known URL
		let metadata = fetch_auth_server_metadata(&format!("{url}/"))
			.await
			.unwrap();
		assert_eq!(metadata.issuer, "https://auth.example.com");
		assert_eq!(metadata.token_endpoint, "https://auth.example.com/token");
		assert!(metadata.registration_endpoint.is_none());

		// Non-success status surfaces the RFC 8414 discovery URL
		let url = spawn_http_stub(move |_m, _p, _h| (500, vec![], String::new())).await;
		let err = fetch_auth_server_metadata(&url).await.unwrap_err();
		assert!(err.to_string().contains("RFC 8414"), "{err}");
	}

	#[tokio::test]
	#[serial_test::serial]
	async fn test_discover_pre_discovery_with_cimd_caches_result() {
		let base = spawn_http_stub(move |method, path, host| {
			let prm = format!(
				"{{\"resource\":\"http://{host}/mcp\",\"authorization_servers\":[\"http://{host}\"],\"scopes_supported\":[\"mcp:read\"]}}"
			);
			let asm = format!(
				"{{\"issuer\":\"http://{host}\",\"authorization_endpoint\":\"http://{host}/oauth/authorize\",\"token_endpoint\":\"http://{host}/oauth/token\",\"scopes_supported\":[\"read\"],\"client_id_metadata_document_supported\":true}}"
			);
			if method == "GET" && path.ends_with("/.well-known/oauth-protected-resource") {
				(200, vec![], prm)
			} else if method == "GET" && path == "/.well-known/oauth-authorization-server" {
				(200, vec![], asm)
			} else {
				(404, vec![], String::new())
			}
		})
		.await;
		let name = unique_server_name("cimd");
		clear_discovered_oauth_cache(&name);

		let config = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
			.await
			.unwrap();

		// client_id is the local CIMD metadata document URL
		assert!(
			config.client_id.starts_with("http://127.0.0.1")
				|| config.client_id.starts_with("http://localhost"),
			"unexpected client_id: {}",
			config.client_id
		);
		assert_eq!(config.authorization_url, format!("{base}/oauth/authorize"));
		assert_eq!(config.token_url, format!("{base}/oauth/token"));
		// Resource scopes take precedence over auth server scopes
		assert_eq!(config.scopes, vec!["mcp:read".to_string()]);

		// Result is cached: a second discovery returns the identical config
		assert!(has_cached_discovery(&name));
		let again = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
			.await
			.unwrap();
		assert_eq!(again.client_id, config.client_id);

		clear_discovered_oauth_cache(&name);
		assert!(!has_cached_discovery(&name));

		// Stop the local CIMD server so its port does not leak into other tests
		super::super::cimd::stop_cimd_server().await;
	}

	#[tokio::test]
	#[serial_test::serial]
	async fn test_discover_401_flow_with_dcr() {
		let base = spawn_http_stub(move |method, path, host| {
			let prm = format!(
				"{{\"resource\":\"http://{host}/mcp\",\"authorization_servers\":[\"http://{host}\"]}}"
			);
			let asm = format!(
				"{{\"issuer\":\"http://{host}\",\"authorization_endpoint\":\"http://{host}/oauth/authorize\",\"token_endpoint\":\"http://{host}/oauth/token\",\"registration_endpoint\":\"http://{host}/register\"}}"
			);
			if method == "POST" && path.starts_with("/mcp") {
				(
					401,
					vec![(
						"WWW-Authenticate".to_string(),
						format!("Bearer resource_metadata=\"http://{host}/prm\""),
					)],
					String::new(),
				)
			} else if method == "GET" && path == "/prm" {
				(200, vec![], prm)
			} else if method == "GET" && path == "/.well-known/oauth-authorization-server" {
				(200, vec![], asm)
			} else if method == "POST" && path == "/register" {
				(
					200,
					vec![],
					"{\"client_id\":\"dcr-client-123\",\"client_secret\":\"dcr-secret\"}".to_string(),
				)
			} else {
				(404, vec![], String::new())
			}
		})
		.await;
		let name = unique_server_name("dcr");
		clear_discovered_oauth_cache(&name);

		let config = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
			.await
			.unwrap();

		// client_id/secret came from the DCR registration response
		assert_eq!(config.client_id, "dcr-client-123");
		assert_eq!(config.client_secret, "dcr-secret");
		assert_eq!(config.token_url, format!("{base}/oauth/token"));
		assert!(has_cached_discovery(&name));

		clear_discovered_oauth_cache(&name);
	}

	#[tokio::test]
	#[serial_test::serial]
	async fn test_discover_errors_without_401() {
		// Pre-discovery 404 + POST returns 200 — not an RFC 9728 server
		let base = spawn_http_stub(move |_m, _p, _h| (200, vec![], String::new())).await;
		let name = unique_server_name("no401");
		clear_discovered_oauth_cache(&name);

		let err = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
			.await
			.unwrap_err();
		assert!(err.to_string().contains("401"), "{err}");
		assert!(!has_cached_discovery(&name), "failures must not be cached");
	}

	#[tokio::test]
	#[serial_test::serial]
	async fn test_discover_errors_on_401_without_header() {
		let base = spawn_http_stub(move |method, path, _host| {
			if method == "POST" && path.starts_with("/mcp") {
				(401, vec![], String::new())
			} else {
				(404, vec![], String::new())
			}
		})
		.await;
		let name = unique_server_name("nohdr");
		clear_discovered_oauth_cache(&name);

		let err = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
			.await
			.unwrap_err();
		assert!(err.to_string().contains("WWW-Authenticate"), "{err}");
	}

	#[tokio::test]
	#[serial_test::serial]
	async fn test_discover_errors_on_empty_authorization_servers() {
		let base = spawn_http_stub(move |method, path, host| {
			if method == "GET" && path.ends_with("/.well-known/oauth-protected-resource") {
				(
					200,
					vec![],
					format!("{{\"resource\":\"http://{host}/mcp\",\"authorization_servers\":[]}}"),
				)
			} else {
				(404, vec![], String::new())
			}
		})
		.await;
		let name = unique_server_name("emptyas");
		clear_discovered_oauth_cache(&name);

		let err = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
			.await
			.unwrap_err();
		assert!(
			err.to_string().contains("no authorization servers"),
			"{err}"
		);
	}

	#[tokio::test]
	#[serial_test::serial]
	async fn test_discover_errors_without_cimd_or_dcr() {
		let base = spawn_http_stub(move |method, path, host| {
			let prm = format!(
				"{{\"resource\":\"http://{host}/mcp\",\"authorization_servers\":[\"http://{host}\"]}}"
			);
			let asm = format!(
				"{{\"issuer\":\"http://{host}\",\"authorization_endpoint\":\"http://{host}/oauth/authorize\",\"token_endpoint\":\"http://{host}/oauth/token\"}}"
			);
			if method == "GET" && path.ends_with("/.well-known/oauth-protected-resource") {
				(200, vec![], prm)
			} else if method == "GET" && path == "/.well-known/oauth-authorization-server" {
				(200, vec![], asm)
			} else {
				(404, vec![], String::new())
			}
		})
		.await;
		let name = unique_server_name("nocimd");
		clear_discovered_oauth_cache(&name);

		let err = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
			.await
			.unwrap_err();
		assert!(err.to_string().contains("client_id"), "{err}");
		assert!(!has_cached_discovery(&name), "failures must not be cached");
	}
}
