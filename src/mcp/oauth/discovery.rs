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

//! MCP Authorization Discovery (MCP Authorization spec 2025-11-25)
//!
//! Implements automatic OAuth configuration discovery for MCP servers following:
//! - RFC 9728: OAuth 2.0 Protected Resource Metadata
//! - RFC 8414 and OpenID Connect Discovery 1.0: Authorization Server Metadata
//!
//! Flow:
//! 1. Unauthenticated request to the MCP server; a 401 Bearer challenge may carry the
//!    `resource_metadata` URL and the `scope` the request needs
//! 2. Fetch Protected Resource Metadata from the challenge URL, or else from the
//!    well-known URLs (path-specific, then root)
//! 3. Extract authorization_servers[0] (primary auth server)
//! 4. Fetch Authorization Server Metadata (OAuth well-known, then OpenID Connect)
//! 5. Require PKCE S256 support
//! 6. Build OAuthConfig from discovered endpoints (client_id from CIMD/DCR)

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

/// The OAuth configuration cached by an earlier discovery of this server, if any.
pub fn cached_discovery(server_name: &str) -> Option<OAuthConfig> {
	DISCOVERED_OAUTH_CACHE.read().unwrap().get(server_name).cloned()
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

/// Parameters of a Bearer challenge in a `WWW-Authenticate` header (RFC 6750, RFC 9728)
#[derive(Debug, Default, PartialEq)]
pub struct BearerChallenge {
	/// Protected Resource Metadata URL (RFC 9728 section 5.1)
	pub resource_metadata: Option<String>,

	/// Scopes the request needs (RFC 6750 section 3); authoritative for the authorization request
	pub scopes: Vec<String>,
}

/// Parse a Bearer challenge, e.g.
/// `Bearer resource_metadata="https://example.com/.well-known/oauth-protected-resource", scope="files:read"`
pub fn parse_bearer_challenge(header_value: &str) -> BearerChallenge {
	let param = |name: &str| {
		Regex::new(&format!(r#"(?:^|[\s,]){name}="([^"]*)""#))
			.ok()
			.and_then(|re| re.captures(header_value))
			.map(|c| c[1].to_string())
	};
	BearerChallenge {
		resource_metadata: param("resource_metadata").filter(|url| !url.is_empty()),
		scopes: param("scope")
			.map(|scope| scope.split_whitespace().map(str::to_string).collect())
			.unwrap_or_default(),
	}
}

/// Well-known Protected Resource Metadata URLs for an MCP server without a challenge URL,
/// in the MCP spec's order: path-specific, then root (RFC 9728 section 3.1).
fn protected_resource_metadata_urls(server_url: &str) -> Vec<String> {
	let Ok(url) = url::Url::parse(server_url) else {
		return Vec::new();
	};
	let root = format!(
		"{}/.well-known/oauth-protected-resource",
		url.origin().ascii_serialization()
	);
	let path = url.path().trim_end_matches('/');
	if path.is_empty() {
		vec![root]
	} else {
		vec![format!("{root}{path}"), root]
	}
}

/// Fetch Protected Resource Metadata from the given URL
///
/// # Arguments
/// * `metadata_url` - URL to the protected resource metadata document
///
/// # Returns
/// * `Ok(ProtectedResourceMetadata)` - Parsed metadata
/// * `Err` - If request fails or JSON parsing fails
pub async fn fetch_protected_resource_metadata(metadata_url: &str) -> Result<ProtectedResourceMetadata> {
	crate::log_debug!("Fetching Protected Resource Metadata from: {}", metadata_url);

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

/// Authorization Server Metadata URLs in the MCP spec's priority order: RFC 8414 OAuth
/// metadata, then OpenID Connect Discovery, with path insertion for issuers that have a path.
fn auth_server_metadata_urls(issuer: &str) -> Vec<String> {
	let issuer = issuer.trim_end_matches('/');
	let Ok(url) = url::Url::parse(issuer) else {
		return vec![format!("{issuer}/.well-known/oauth-authorization-server")];
	};
	let origin = url.origin().ascii_serialization();
	let path = url.path().trim_end_matches('/');
	if path.is_empty() {
		vec![
			format!("{origin}/.well-known/oauth-authorization-server"),
			format!("{origin}/.well-known/openid-configuration"),
		]
	} else {
		vec![
			format!("{origin}/.well-known/oauth-authorization-server{path}"),
			format!("{origin}/.well-known/openid-configuration{path}"),
			format!("{issuer}/.well-known/openid-configuration"),
		]
	}
}

/// Fetch Authorization Server Metadata, trying each discovery URL in priority order
///
/// # Arguments
/// * `issuer` - The authorization server issuer URL
///
/// # Returns
/// * `Ok(AuthServerMetadata)` - Metadata from the first URL that answers with a valid document
/// * `Err` - If no discovery URL does
pub async fn fetch_auth_server_metadata(issuer: &str) -> Result<AuthServerMetadata> {
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(10))
		.build()
		.context("Failed to create HTTP client")?;

	let mut attempts = Vec::new();
	for metadata_url in auth_server_metadata_urls(issuer) {
		crate::log_debug!("Fetching Authorization Server Metadata from: {}", metadata_url);
		let outcome = match client.get(&metadata_url).send().await {
			Ok(response) if response.status().is_success() => match response.json::<AuthServerMetadata>().await {
				Ok(metadata) => {
					crate::log_debug!(
						"Authorization Server Metadata: issuer={}, auth_endpoint={}, token_endpoint={}",
						metadata.issuer,
						metadata.authorization_endpoint,
						metadata.token_endpoint
					);
					return Ok(metadata);
				}
				Err(e) => format!("invalid metadata JSON: {e}"),
			},
			Ok(response) => format!("status {}", response.status()),
			Err(e) => e.to_string(),
		};
		attempts.push(format!("{metadata_url} ({outcome})"));
	}

	Err(anyhow!(
		"Authorization Server Metadata not found (RFC 8414 / OpenID Connect Discovery); tried: {}",
		attempts.join(", ")
	))
}

/// MCP requires PKCE with S256; a server that doesn't advertise it is refused.
fn ensure_pkce_s256(auth_metadata: &AuthServerMetadata) -> Result<()> {
	let methods = auth_metadata
		.code_challenge_methods_supported
		.as_deref()
		.unwrap_or_default();
	if methods.iter().any(|method| method == "S256") {
		return Ok(());
	}
	Err(anyhow!(
		"Authorization server '{}' does not advertise PKCE S256 in code_challenge_methods_supported; \
		 MCP authorization requires it",
		auth_metadata.issuer
	))
}

/// Build OAuthConfig from discovered metadata
///
/// # Arguments
/// * `auth_metadata` - Authorization Server Metadata
/// * `resource_metadata` - Protected Resource Metadata
/// * `challenge_scopes` - Scopes from the 401 Bearer challenge, if any
///
/// # Returns
/// * `OAuthConfig` - Ready-to-use OAuth configuration
///
/// Note: client_id is set to a placeholder. It must be resolved via CIMD or DCR
/// before the OAuth flow can proceed. See cimd.rs for CIMD/DCR resolution.
pub fn build_oauth_config_from_metadata(
	auth_metadata: &AuthServerMetadata,
	resource_metadata: &ProtectedResourceMetadata,
	challenge_scopes: &[String],
) -> OAuthConfig {
	// MCP scope selection: the challenge's scopes, then the resource's scopes_supported. The
	// authorization server's list comes last: some servers (Ory Hydra) issue refresh tokens only
	// for a scope such as offline_access that only that list names.
	let scopes = if challenge_scopes.is_empty() {
		resource_metadata
			.scopes_supported
			.as_ref()
			.or(auth_metadata.scopes_supported.as_ref())
			.cloned()
			.unwrap_or_default()
	} else {
		challenge_scopes.to_vec()
	};

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
/// 2. Unauthenticated request to the MCP server; parse a 401 Bearer challenge
/// 3. Fetch Protected Resource Metadata (challenge URL, else well-known URLs)
/// 4. Extract primary authorization server
/// 5. Fetch Authorization Server Metadata (OAuth, then OpenID Connect) and require PKCE S256
/// 6. Build OAuthConfig from discovered endpoints, resolve client_id via CIMD/DCR
/// 7. Cache the result for future use
///
/// # Arguments
/// * `server_url` - The MCP server URL (e.g., "https://api.githubcopilot.com/mcp/")
/// * `server_name` - The server name for logging and caching
///
/// # Returns
/// * `Ok(OAuthConfig)` - Discovered OAuth configuration (from cache or fresh discovery)
/// * `Err` - If discovery fails at any step
pub async fn discover_oauth_from_mcp_server(server_url: &str, server_name: &str) -> Result<OAuthConfig> {
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

	// Step 1: Unauthenticated request; a 401 Bearer challenge may name the metadata URL and scopes
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
	let status = response.status();
	let challenge = if status == reqwest::StatusCode::UNAUTHORIZED {
		let header = response
			.headers()
			.get(reqwest::header::WWW_AUTHENTICATE)
			.and_then(|value| value.to_str().ok())
			.unwrap_or_default();
		crate::log_debug!("401 challenge: {}", header);
		Some(parse_bearer_challenge(header))
	} else {
		None
	};

	// Step 2: Protected Resource Metadata — the challenge URL, else the well-known URLs
	let resource_metadata = match challenge.as_ref().and_then(|c| c.resource_metadata.as_deref()) {
		Some(metadata_url) => fetch_protected_resource_metadata(metadata_url)
			.await
			.context("Failed to fetch Protected Resource Metadata")?,
		None => {
			let urls = protected_resource_metadata_urls(server_url);
			let mut found = None;
			for url in &urls {
				match fetch_protected_resource_metadata(url).await {
					Ok(metadata) => {
						found = Some(metadata);
						break;
					}
					Err(e) => crate::log_debug!("No Protected Resource Metadata at {}: {}", url, e),
				}
			}
			found.ok_or_else(|| match challenge {
				Some(_) => anyhow!(
					"MCP server returned 401 without a resource_metadata WWW-Authenticate challenge, \
					 and no Protected Resource Metadata was found at {}",
					urls.join(", ")
				),
				None => anyhow!(
					"MCP Authorization discovery requires a 401 Unauthorized response or Protected Resource \
					 Metadata (RFC 9728); got {} and none at {}",
					status,
					urls.join(", ")
				),
			})?
		}
	};

	// Step 3: Extract primary authorization server
	let auth_server_issuer = resource_metadata
		.authorization_servers
		.first()
		.ok_or_else(|| anyhow!("Protected Resource Metadata contains no authorization servers"))?;

	crate::log_debug!("Using authorization server: {}", auth_server_issuer);

	// Step 4: Fetch Authorization Server Metadata
	let auth_metadata = fetch_auth_server_metadata(auth_server_issuer)
		.await
		.context("Failed to fetch Authorization Server Metadata")?;
	ensure_pkce_s256(&auth_metadata)?;

	// Step 5: Build OAuthConfig from discovered metadata (client_id is placeholder)
	let challenge_scopes = challenge.map(|c| c.scopes).unwrap_or_default();
	let oauth_config = build_oauth_config_from_metadata(&auth_metadata, &resource_metadata, &challenge_scopes);

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
