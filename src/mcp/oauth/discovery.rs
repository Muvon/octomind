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
#[path = "discovery_tests.rs"]
mod tests;
