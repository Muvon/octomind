//! OAuth 2.1 + PKCE Authentication for MCP Servers
//!
//! Supports MCP Authorization Discovery (RFC 9728) - automatic OAuth endpoint discovery.
//! All OAuth configuration is discovered automatically via RFC 9728/8414 metadata;
//! no manual user configuration is needed.

pub mod callback_server;
pub mod cimd;
pub mod discovery;
pub mod flow;
pub mod token_store;

// Re-export commonly used types
pub use callback_server::{start_callback_server, OAuthCallbackResult};
pub use cimd::{resolve_client_id, stop_cimd_server};
pub use discovery::{
	clear_all_discovered_oauth_cache, clear_discovered_oauth_cache, discover_oauth_from_mcp_server,
};
pub use flow::{
	build_authorization_url, exchange_code_for_token, generate_pkce_pair, generate_state,
	is_token_expired, refresh_access_token, OAuthTokenResponse, PkcePair,
};
pub use token_store::{
	clear_token, get_valid_token, load_token, save_token, TokenMetadata, TokenResult,
};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use url::Url;

/// Internal OAuth 2.1 + PKCE configuration for MCP servers.
///
/// This struct is built automatically from RFC 9728/8414 discovery metadata.
/// It is NOT user-configurable — all fields come from the authorization server.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct OAuthConfig {
	/// The OAuth client ID (from CIMD, DCR, or known public client).
	pub client_id: String,

	/// The OAuth client secret (empty for public clients using PKCE).
	#[serde(default)]
	pub client_secret: String,

	/// The OAuth authorization endpoint URL.
	pub authorization_url: String,

	/// The OAuth token endpoint URL.
	pub token_url: String,

	/// The OAuth authorization callback URL (local callback server).
	pub callback_url: String,

	/// List of OAuth scopes to request.
	#[serde(default)]
	pub scopes: Vec<String>,

	/// Optional state parameter for CSRF protection.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub state: Option<String>,

	/// Token refresh buffer in seconds before expiry.
	#[serde(default = "default_refresh_buffer")]
	pub refresh_buffer_seconds: u64,

	/// Protected resource identifier (RFC 9728 §2.1). Sent as the `resource`
	/// parameter in authorization and token requests so the authorization
	/// server can issue audience-scoped tokens.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resource: Option<String>,
}

fn default_refresh_buffer() -> u64 {
	300
}

impl OAuthConfig {
	/// Creates a new OAuthConfig with all required fields.
	pub fn new(
		client_id: String,
		client_secret: String,
		authorization_url: String,
		token_url: String,
		callback_url: String,
		scopes: Vec<String>,
	) -> Self {
		Self {
			client_id,
			client_secret,
			authorization_url,
			token_url,
			callback_url,
			scopes,
			state: None,
			refresh_buffer_seconds: default_refresh_buffer(),
			resource: None,
		}
	}

	/// Validates the OAuth configuration.
	pub fn validate(&self) -> Result<(), String> {
		if self.client_id.trim().is_empty() {
			return Err("OAuth client_id cannot be empty".to_string());
		}

		let auth_url = Url::parse(&self.authorization_url).map_err(|e| {
			format!(
				"OAuth authorization_url is invalid: {}. Must be a valid URL",
				e
			)
		})?;

		if auth_url.scheme() != "https"
			&& auth_url.host_str() != Some("localhost")
			&& auth_url.host_str() != Some("127.0.0.1")
		{
			return Err(
				"OAuth authorization_url must use HTTPS or http://localhost/http://127.0.0.1"
					.to_string(),
			);
		}

		let token_url = Url::parse(&self.token_url)
			.map_err(|e| format!("OAuth token_url is invalid: {}. Must be a valid URL", e))?;

		if token_url.scheme() != "https"
			&& token_url.host_str() != Some("localhost")
			&& token_url.host_str() != Some("127.0.0.1")
		{
			return Err(
				"OAuth token_url must use HTTPS or http://localhost/http://127.0.0.1".to_string(),
			);
		}

		let callback_url = Url::parse(&self.callback_url)
			.map_err(|e| format!("OAuth callback_url is invalid: {}. Must be a valid URL", e))?;

		if callback_url.scheme() != "http" && callback_url.scheme() != "https" {
			return Err("OAuth callback_url must use HTTP or HTTPS".to_string());
		}

		for scope in &self.scopes {
			if scope.trim().is_empty() {
				return Err("OAuth scopes cannot contain empty strings".to_string());
			}
		}

		if self.refresh_buffer_seconds < 60 {
			return Err("OAuth refresh_buffer_seconds must be at least 60 seconds".to_string());
		}

		Ok(())
	}
}

// Global lock to prevent concurrent OAuth flows for the same client_id
lazy_static::lazy_static! {
	static ref OAUTH_LOCKS: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>> = Arc::new(Mutex::new(HashMap::new()));
}

/// Starts the complete OAuth authorization flow. `server_name` keys the token
/// store — it must be the MCP config server name that lookups use.
pub async fn start_oauth_flow(
	config: &OAuthConfig,
	server_name: &str,
) -> Result<OAuthCallbackResult, anyhow::Error> {
	config
		.validate()
		.map_err(|e| anyhow::anyhow!("OAuth config validation failed: {}", e))?;

	let pkce = generate_pkce_pair();
	let state = generate_state();

	// Start callback server using the configured callback_url
	let result = start_callback_server(config, server_name, state, pkce.code_verifier).await?;

	Ok(result)
}

/// Gets an access token for the given OAuth configuration and server name.
/// Uses a per-server lock to prevent concurrent OAuth flows.
///
/// # Arguments
/// * `config` - OAuth configuration (discovered via RFC 9728)
/// * `server_name` - Server name for token storage (each server has separate token)
/// * `force_refresh` - Force token refresh even if valid token exists
pub async fn get_access_token(
	config: &OAuthConfig,
	server_name: &str,
	force_refresh: bool,
) -> Result<Option<String>, anyhow::Error> {
	// Get or create a lock for this server to prevent concurrent OAuth flows
	let lock = {
		let mut locks = OAUTH_LOCKS.lock().await;
		locks
			.entry(server_name.to_string())
			.or_insert_with(|| Arc::new(Mutex::new(())))
			.clone()
	};

	// Acquire the lock for this specific server
	let _guard = lock.lock().await;

	// Check for valid token again after acquiring lock (another thread might have completed OAuth)
	if !force_refresh {
		crate::log_debug!(
			"🔍 GET_ACCESS_TOKEN: Checking for existing valid token for server_name='{}'",
			server_name
		);
		let valid_token = get_valid_token(server_name, config.refresh_buffer_seconds)
			.await
			.map_err(|e| anyhow::anyhow!("Failed to check token: {}", e))?;

		if let Some(metadata) = valid_token {
			crate::log_debug!(
				"✅ GET_ACCESS_TOKEN: Found valid token, token_prefix='{}...'",
				metadata.access_token.chars().take(10).collect::<String>()
			);
			return Ok(Some(metadata.access_token));
		} else {
			crate::log_debug!("⚠️  GET_ACCESS_TOKEN: No valid token found, starting OAuth flow");
		}
	}

	// No valid token - start standard web OAuth flow (PKCE + callback)
	let result = start_oauth_flow(config, server_name).await?;

	match result {
		OAuthCallbackResult::Success { access_token, .. } => Ok(Some(access_token)),
		OAuthCallbackResult::Error { error, description } => Err(anyhow::anyhow!(
			"OAuth failed: {} - {}",
			error,
			description.unwrap_or_default()
		)),
		OAuthCallbackResult::Cancelled => Ok(None),
		OAuthCallbackResult::Timeout => Err(anyhow::anyhow!("OAuth timed out")),
	}
}

/// Checks if a server is authenticated.
pub async fn is_authenticated(server_name: &str, refresh_buffer_seconds: u64) -> bool {
	get_valid_token(server_name, refresh_buffer_seconds)
		.await
		.map(|m| m.is_some())
		.unwrap_or(false)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn valid_config() -> OAuthConfig {
		OAuthConfig::new(
			"client-123".to_string(),
			String::new(),
			"https://auth.example.com/oauth/authorize".to_string(),
			"https://auth.example.com/oauth/token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["mcp:read".to_string(), "mcp:write".to_string()],
		)
	}

	// ------------------------------------------------------------------
	// OAuthConfig::new
	// ------------------------------------------------------------------

	#[test]
	fn new_initializes_fields_with_defaults() {
		let config = valid_config();
		assert_eq!(config.client_id, "client-123");
		assert!(config.client_secret.is_empty());
		assert_eq!(
			config.authorization_url,
			"https://auth.example.com/oauth/authorize"
		);
		assert_eq!(config.token_url, "https://auth.example.com/oauth/token");
		assert_eq!(config.callback_url, "http://localhost:34567/oauth/callback");
		assert_eq!(config.scopes, vec!["mcp:read", "mcp:write"]);
		assert_eq!(config.state, None);
		assert_eq!(config.resource, None);
		assert_eq!(config.refresh_buffer_seconds, 300);
	}

	#[test]
	fn default_refresh_buffer_is_300() {
		assert_eq!(default_refresh_buffer(), 300);
	}

	// ------------------------------------------------------------------
	// validate — acceptance
	// ------------------------------------------------------------------

	#[test]
	fn validate_accepts_https_config() {
		assert!(valid_config().validate().is_ok());
	}

	#[test]
	fn validate_accepts_localhost_http() {
		let mut config = valid_config();
		config.authorization_url = "http://localhost:8123/oauth/authorize".to_string();
		config.token_url = "http://127.0.0.1:8123/oauth/token".to_string();
		assert!(config.validate().is_ok());
	}

	// ------------------------------------------------------------------
	// validate — rejection paths (one mutated field per case)
	// ------------------------------------------------------------------

	#[test]
	fn validate_rejects_empty_client_id() {
		let mut config = valid_config();
		config.client_id = String::new();
		assert!(config.validate().unwrap_err().contains("client_id"));

		// Whitespace-only is still empty (trim check)
		config.client_id = "   ".to_string();
		assert!(config.validate().unwrap_err().contains("client_id"));
	}

	#[test]
	fn validate_rejects_insecure_authorization_url() {
		let mut config = valid_config();
		// Plain HTTP to a remote host is not allowed
		config.authorization_url = "http://auth.example.com/oauth/authorize".to_string();
		let err = config.validate().unwrap_err();
		assert!(err.contains("authorization_url"), "{err}");
		assert!(err.contains("HTTPS"), "{err}");

		// Non-HTTP scheme is equally rejected
		config.authorization_url = "ftp://auth.example.com/oauth/authorize".to_string();
		assert!(config.validate().unwrap_err().contains("authorization_url"));
	}

	#[test]
	fn validate_rejects_invalid_authorization_url() {
		let mut config = valid_config();
		config.authorization_url = "not a valid url".to_string();
		assert!(config.validate().unwrap_err().contains("invalid"));
	}

	#[test]
	fn validate_rejects_insecure_or_invalid_token_url() {
		let mut config = valid_config();
		config.token_url = "http://auth.example.com/oauth/token".to_string();
		assert!(config.validate().unwrap_err().contains("token_url"));

		config.token_url = "not a valid url".to_string();
		assert!(config.validate().unwrap_err().contains("token_url"));
	}

	#[test]
	fn validate_rejects_invalid_callback_url() {
		let mut config = valid_config();
		// Callback allows http/https only
		config.callback_url = "ftp://localhost:34567/oauth/callback".to_string();
		assert!(config.validate().unwrap_err().contains("callback_url"));

		config.callback_url = "not a valid url".to_string();
		assert!(config.validate().unwrap_err().contains("callback_url"));
	}

	#[test]
	fn validate_rejects_empty_scope_strings() {
		let mut config = valid_config();
		config.scopes.push("   ".to_string());
		assert!(config.validate().unwrap_err().contains("empty strings"));
	}

	#[test]
	fn validate_enforces_minimum_refresh_buffer() {
		let mut config = valid_config();
		config.refresh_buffer_seconds = 59;
		assert!(config.validate().unwrap_err().contains("at least 60"));

		// 60 is the inclusive boundary and must pass
		config.refresh_buffer_seconds = 60;
		assert!(config.validate().is_ok());
	}

	// ------------------------------------------------------------------
	// Serde attributes (defaults keep older persisted configs loadable)
	// ------------------------------------------------------------------

	#[test]
	fn serde_fills_defaults_for_missing_optional_fields() {
		let json = r#"{
			"client_id": "client-123",
			"authorization_url": "https://auth.example.com/oauth/authorize",
			"token_url": "https://auth.example.com/oauth/token",
			"callback_url": "http://localhost:34567/oauth/callback"
		}"#;
		let config: OAuthConfig = serde_json::from_str(json).unwrap();
		assert_eq!(config.client_secret, "");
		assert!(config.scopes.is_empty());
		assert_eq!(config.refresh_buffer_seconds, 300);
		assert_eq!(config.state, None);
		assert_eq!(config.resource, None);
	}

	#[test]
	fn serde_roundtrip_preserves_config() {
		let config = valid_config();
		let json = serde_json::to_string(&config).unwrap();
		let parsed: OAuthConfig = serde_json::from_str(&json).unwrap();
		assert_eq!(parsed, config);
	}

	// ------------------------------------------------------------------
	// is_authenticated — unknown server has no stored token
	// (runs against the cfg(test)-isolated keystore, no network)
	// ------------------------------------------------------------------

	#[tokio::test]
	async fn is_authenticated_false_for_unknown_server() {
		let server = format!("never-authenticated-{}", uuid::Uuid::new_v4());
		assert!(!is_authenticated(&server, 300).await);
	}
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod mod_tests;
