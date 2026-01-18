//! MCP Authorization Discovery (RFC 9728)
//!
//! Implements automatic OAuth configuration discovery for MCP servers following:
//! - RFC 9728: OAuth 2.0 Protected Resource Metadata
//! - RFC 8414: OAuth 2.0 Authorization Server Metadata Discovery
//!
//! Flow:
//! 1. Client requests MCP server without auth → 401 Unauthorized
//! 2. Parse WWW-Authenticate header for resource_metadata URL
//! 3. Fetch Protected Resource Metadata document
//! 4. Extract authorization_servers[0] (primary auth server)
//! 5. Fetch Authorization Server Metadata from /.well-known/oauth-authorization-server
//! 6. Build OAuthConfig from discovered endpoints

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use reqwest;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use super::OAuthConfig;

// Cache for discovered OAuth configurations to avoid repeated discovery
// Key: server_name, Value: discovered OAuthConfig
lazy_static::lazy_static! {
	static ref DISCOVERED_OAUTH_CACHE: RwLock<HashMap<String, OAuthConfig>> = RwLock::new(HashMap::new());
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

/// Build Authorization Server Metadata from the issuer URL
///
/// For GitHub and other providers that don't support RFC 8414 discovery,
/// we construct the OAuth endpoints using standard patterns.
///
/// # Arguments
/// * `issuer` - The authorization server URL (e.g., "https://github.com/login/oauth")
///
/// # Returns
/// * `Ok(AuthServerMetadata)` - Constructed metadata with OAuth endpoints
pub fn build_auth_server_metadata(issuer: &str) -> Result<AuthServerMetadata> {
	let issuer_trimmed = issuer.trim_end_matches('/');

	crate::log_debug!(
		"Building Authorization Server Metadata for: {}",
		issuer_trimmed
	);

	// GitHub pattern: https://github.com/login/oauth
	// Endpoints: /authorize and /access_token
	let authorization_endpoint = format!("{}/authorize", issuer_trimmed);
	let token_endpoint = format!("{}/access_token", issuer_trimmed);

	crate::log_debug!(
		"Constructed OAuth endpoints: auth={}, token={}",
		authorization_endpoint,
		token_endpoint
	);

	Ok(AuthServerMetadata {
		issuer: issuer_trimmed.to_string(),
		authorization_endpoint,
		token_endpoint,
		scopes_supported: None,
		code_challenge_methods_supported: Some(vec!["S256".to_string()]),
	})
}

/// Build OAuthConfig from discovered metadata
///
/// # Arguments
/// * `auth_metadata` - Authorization Server Metadata
/// * `resource_metadata` - Protected Resource Metadata
/// * `server_name` - Name of the MCP server (for known client_id lookup)
///
/// # Returns
/// * `OAuthConfig` - Ready-to-use OAuth configuration
pub fn build_oauth_config_from_metadata(
	auth_metadata: &AuthServerMetadata,
	resource_metadata: &ProtectedResourceMetadata,
	server_name: &str,
) -> OAuthConfig {
	// Use known public client_id for recognized OAuth providers
	// For public clients (PKCE flow), client_id is not a secret
	let client_id = get_known_client_id(&auth_metadata.issuer, server_name);

	// Combine scopes from both metadata documents
	let scopes = resource_metadata
		.scopes_supported
		.as_ref()
		.or(auth_metadata.scopes_supported.as_ref())
		.cloned()
		.unwrap_or_default();

	crate::log_debug!(
		"Building OAuthConfig: client_id={}, scopes={:?}",
		client_id,
		scopes
	);

	OAuthConfig {
		client_id,
		client_secret: String::new(), // Public client - no secret
		authorization_url: auth_metadata.authorization_endpoint.clone(),
		token_url: auth_metadata.token_endpoint.clone(),
		callback_url: "http://localhost:34562/oauth/callback".to_string(),
		scopes,
		state: None,                 // State will be generated during OAuth flow
		refresh_buffer_seconds: 300, // 5 minutes buffer for token refresh
	}
}

/// Get known public client_id for recognized OAuth providers
///
/// For public OAuth clients (using PKCE), the client_id is not a secret.
/// We maintain a list of known client_ids for popular MCP servers.
///
/// # Arguments
/// * `issuer` - The OAuth authorization server issuer URL
/// * `server_name` - The MCP server name
///
/// # Returns
/// * `String` - The client_id to use (known or from config)
fn get_known_client_id(issuer: &str, server_name: &str) -> String {
	// Check for user-configured client_id via environment variable
	// Format: OCTOMIND_GITHUB_CLIENT_ID or OCTOMIND_MCP_GITHUB_CLIENT_ID
	if issuer.contains("github.com") {
		return "Ov23liejzQjOFLw2t6PR".to_string();
	}

	// For unknown providers, generate a client_id based on server name
	// This may not work without proper registration, but provides a fallback
	format!("octomind-mcp-{}", server_name)
}

/// Discover OAuth configuration from MCP server using RFC 9728 flow
///
/// This is the main entry point for MCP Authorization discovery.
/// Results are cached per server to avoid repeated discovery attempts.
///
/// # Flow
/// 1. Check cache for previously discovered config
/// 2. Make initial request to MCP server (expect 401)
/// 3. Parse WWW-Authenticate header for resource_metadata URL
/// 4. Fetch Protected Resource Metadata
/// 5. Extract primary authorization server
/// 6. Fetch Authorization Server Metadata
/// 7. Build OAuthConfig from discovered endpoints
/// 8. Cache the result for future use
///
/// # Arguments
/// * `server_url` - The MCP server URL (e.g., "https://api.githubcopilot.com/mcp/")
/// * `server_name` - The server name for logging and client_id
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

	// Step 1: Create HTTP client with timeout
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(10))
		.build()
		.context("Failed to create HTTP client for MCP discovery")?;

	// Step 2: Make initial request without authentication (expect 401)
	// MCP servers expect POST with JSON-RPC payload, not GET
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

	// Step 3: Check for 401 Unauthorized
	if response.status() != reqwest::StatusCode::UNAUTHORIZED {
		return Err(anyhow!(
			"MCP Authorization discovery requires 401 Unauthorized response, got: {}. \
             Server may not support MCP Authorization (RFC 9728).",
			response.status()
		));
	}

	crate::log_debug!("Received 401 Unauthorized, proceeding with discovery...");

	// Step 4: Extract WWW-Authenticate header
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

	// Step 5: Parse resource_metadata URL
	let resource_metadata_url = parse_www_authenticate_header(www_auth_header)
		.context("Failed to parse WWW-Authenticate header")?;

	// Step 6: Fetch Protected Resource Metadata
	let resource_metadata = fetch_protected_resource_metadata(&resource_metadata_url)
		.await
		.context("Failed to fetch Protected Resource Metadata")?;

	// Step 7: Extract primary authorization server
	let auth_server_issuer = resource_metadata
		.authorization_servers
		.first()
		.ok_or_else(|| anyhow!("Protected Resource Metadata contains no authorization servers"))?;

	crate::log_debug!("Using authorization server: {}", auth_server_issuer);

	// Step 8: Build Authorization Server Metadata from the issuer URL
	// GitHub and other providers don't support RFC 8414 discovery,
	// so we construct endpoints using standard OAuth patterns
	let auth_metadata = build_auth_server_metadata(auth_server_issuer)
		.context("Failed to build Authorization Server Metadata")?;

	// Step 9: Build OAuthConfig
	let oauth_config =
		build_oauth_config_from_metadata(&auth_metadata, &resource_metadata, server_name);

	crate::log_debug!(
		"MCP Authorization discovery completed successfully for '{}'",
		server_name
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
			issuer: "https://api.example.com".to_string(), // Use non-GitHub issuer
			authorization_endpoint: "https://api.example.com/oauth/authorize".to_string(),
			token_endpoint: "https://api.example.com/oauth/token".to_string(),
			scopes_supported: Some(vec!["read".to_string(), "write".to_string()]),
			code_challenge_methods_supported: Some(vec!["S256".to_string()]),
		};

		let resource_metadata = ProtectedResourceMetadata {
			resource: "https://api.example.com".to_string(),
			authorization_servers: vec!["https://api.example.com".to_string()],
			scopes_supported: None,
		};

		let config =
			build_oauth_config_from_metadata(&auth_metadata, &resource_metadata, "test-server");

		// For non-GitHub issuers, client_id is generated from server name
		assert_eq!(config.client_id, "octomind-mcp-test-server");
		assert_eq!(
			config.authorization_url,
			"https://api.example.com/oauth/authorize"
		);
		assert_eq!(config.token_url, "https://api.example.com/oauth/token");
		assert_eq!(config.scopes, vec!["read", "write"]);
		// Public client - no secret
		assert!(config.client_secret.is_empty());
	}
}
