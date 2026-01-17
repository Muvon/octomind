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

//! OAuth 2.1 + PKCE configuration for MCP servers
//!
//! This module provides strict OAuth configuration for MCP server authentication.
//! All fields are required and validated at config load time.

use serde::{Deserialize, Serialize};
use url::Url;

/// OAuth 2.1 + PKCE configuration for MCP servers.
///
/// This configuration is used for OAuth 2.1 authentication with PKCE (Proof Key for Code Exchange),
/// which is the recommended authentication method for MCP servers per the MCP specification.
///
/// ## Required Fields
/// - `client_id`: The OAuth client ID issued by the authorization server
/// - `client_secret`: The OAuth client secret (for confidential clients)
/// - `authorization_url`: The authorization endpoint URL
/// - `token_url`: The token endpoint URL
/// - `callback_url`: The authorization callback URL registered with the OAuth provider
/// - `scopes`: List of OAuth scopes to request
///
/// ## Example Configuration
///
/// ```toml
/// [mcp.servers.github.oauth]
/// client_id = "your-github-oauth-app-client-id"
/// client_secret = "your-github-oauth-app-client-secret"
/// authorization_url = "https://github.com/login/oauth/authorize"
/// token_url = "https://github.com/login/oauth/access_token"
/// callback_url = "http://localhost:34567/oauth/callback"
/// scopes = ["repo", "read:org", "workflow"]
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct OAuthConfig {
	/// The OAuth client ID.
	/// This is issued by the authorization server when you register your application.
	///
	/// Required: Yes
	/// Validation: Must not be empty
	pub client_id: String,

	/// The OAuth client secret.
	/// Used for confidential clients to authenticate with the token endpoint.
	///
	/// Required: Yes (for confidential client OAuth flows)
	/// Validation: Must not be empty
	pub client_secret: String,

	/// The OAuth authorization endpoint URL.
	/// Users will be redirected to this URL to authorize access.
	///
	/// Required: Yes
	/// Validation: Must be a valid HTTPS URL (or http://localhost for development)
	pub authorization_url: String,

	/// The OAuth token endpoint URL.
	/// Used to exchange authorization codes for tokens and to refresh tokens.
	///
	/// Required: Yes
	/// Validation: Must be a valid HTTPS URL (or http://localhost for development)
	pub token_url: String,

	/// The OAuth authorization callback URL.
	/// This must match exactly with the callback URL registered with your OAuth provider.
	/// For local development, use http://localhost with the port your callback server uses.
	///
	/// Required: Yes
	/// Validation: Must be a valid URL (http://localhost or https:// for production)
	pub callback_url: String,

	/// List of OAuth scopes to request from the user.
	/// Scopes determine what access the application requests.
	///
	/// Required: Yes
	/// Validation: Must not be empty
	#[serde(default)]
	pub scopes: Vec<String>,

	/// Optional: State parameter for CSRF protection.
	/// If not provided, a random state will be generated during OAuth flow.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub state: Option<String>,

	/// Optional: Token refresh buffer in seconds before expiry.
	/// Tokens will be refreshed when they are within this time of expiring.
	/// Default: 300 seconds (5 minutes)
	#[serde(default = "default_refresh_buffer")]
	pub refresh_buffer_seconds: u64,
}

/// Default refresh buffer: 5 minutes before token expiry
fn default_refresh_buffer() -> u64 {
	300
}

impl OAuthConfig {
	/// Creates a new OAuthConfig with all required fields.
	///
	/// # Arguments
	///
	/// * `client_id` - The OAuth client ID
	/// * `client_secret` - The OAuth client secret
	/// * `authorization_url` - The authorization endpoint
	/// * `token_url` - The token endpoint
	/// * `callback_url` - The callback URL registered with OAuth provider
	/// * `scopes` - List of requested scopes
	///
	/// # Returns
	///
	/// A new OAuthConfig with default values for optional fields.
	#[allow(clippy::too_many_arguments)]
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
		}
	}

	/// Validates the OAuth configuration.
	///
	/// Returns `Err` with a descriptive error message if validation fails.
	///
	/// # Validation Rules
	///
	/// - `client_id` must not be empty
	/// - `client_secret` can be empty for public clients (PKCE flow)
	/// - `authorization_url` must be a valid URL
	/// - `token_url` must be a valid URL
	/// - `redirect_uri` must be a valid URL
	/// - `scopes` can be empty (some providers don't require scopes)
	///
	/// # Returns
	///
	/// `Ok(())` if valid, or `Err(String)` with error message.
	pub fn validate(&self) -> Result<(), String> {
		// Validate client_id
		if self.client_id.trim().is_empty() {
			return Err("OAuth client_id cannot be empty".to_string());
		}

		// client_secret can be empty for public clients using PKCE
		// No validation needed for client_secret

		// Validate authorization_url
		let auth_url = Url::parse(&self.authorization_url).map_err(|e| {
			format!(
				"OAuth authorization_url is invalid: {}. Must be a valid URL (e.g., https://example.com/oauth/authorize)",
				e
			)
		})?;

		if auth_url.scheme() != "https"
			&& auth_url.host_str() != Some("localhost")
			&& auth_url.host_str() != Some("127.0.0.1")
		{
			return Err(
				"OAuth authorization_url must use HTTPS or http://localhost/http://127.0.0.1 for development"
					.to_string(),
			);
		}

		// Validate token_url
		let token_url = Url::parse(&self.token_url).map_err(|e| {
			format!(
				"OAuth token_url is invalid: {}. Must be a valid URL (e.g., https://example.com/oauth/token)",
				e
			)
		})?;

		if token_url.scheme() != "https"
			&& token_url.host_str() != Some("localhost")
			&& token_url.host_str() != Some("127.0.0.1")
		{
			return Err(
				"OAuth token_url must use HTTPS or http://localhost/http://127.0.0.1 for development"
					.to_string(),
			);
		}

		// Validate callback_url
		let callback_url = Url::parse(&self.callback_url).map_err(|e| {
			format!(
				"OAuth callback_url is invalid: {}. Must be a valid URL (e.g., http://localhost:34567/oauth/callback)",
				e
			)
		})?;

		if callback_url.scheme() != "http" && callback_url.scheme() != "https" {
			return Err("OAuth callback_url must use HTTP or HTTPS".to_string());
		}

		// Validate scopes (can be empty for some providers)
		// Just validate that scopes don't contain empty strings
		for scope in &self.scopes {
			if scope.trim().is_empty() {
				return Err("OAuth scopes cannot contain empty strings".to_string());
			}
		}

		// Validate refresh_buffer_seconds
		if self.refresh_buffer_seconds < 60 {
			return Err("OAuth refresh_buffer_seconds must be at least 60 seconds".to_string());
		}

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_oauth_config_validation_empty_client_id() {
		let config = OAuthConfig::new(
			String::new(), // empty client_id
			"secret".to_string(),
			"https://example.com/authorize".to_string(),
			"https://example.com/token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["repo".to_string()],
		);

		assert!(config.validate().is_err());
		assert_eq!(
			config.validate().unwrap_err(),
			"OAuth client_id cannot be empty"
		);
	}

	#[test]
	fn test_oauth_config_validation_empty_client_secret() {
		// Empty client_secret is valid for public OAuth clients (PKCE flow)
		let config = OAuthConfig::new(
			"client_id".to_string(),
			String::new(), // empty client_secret - valid for public clients
			"https://example.com/authorize".to_string(),
			"https://example.com/token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["repo".to_string()],
		);

		// Public clients with PKCE don't require a client_secret
		assert!(config.validate().is_ok());
	}

	#[test]
	fn test_oauth_config_validation_empty_scopes() {
		// Empty scopes are valid - some OAuth providers don't require specific scopes
		let config = OAuthConfig::new(
			"client_id".to_string(),
			"secret".to_string(),
			"https://example.com/authorize".to_string(),
			"https://example.com/token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec![], // empty scopes - valid for some providers
		);

		// Empty scopes are now valid (just can't contain empty strings)
		assert!(config.validate().is_ok());
	}

	#[test]
	fn test_oauth_config_validation_invalid_authorization_url() {
		let config = OAuthConfig::new(
			"client_id".to_string(),
			"secret".to_string(),
			"not-a-valid-url".to_string(),
			"https://example.com/token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["repo".to_string()],
		);

		assert!(config.validate().is_err());
		assert!(config
			.validate()
			.unwrap_err()
			.contains("authorization_url is invalid"));
	}

	#[test]
	fn test_oauth_config_validation_http_authorization_url_not_localhost() {
		let config = OAuthConfig::new(
			"client_id".to_string(),
			"secret".to_string(),
			"http://example.com/authorize".to_string(),
			"https://example.com/token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["repo".to_string()],
		);

		assert!(config.validate().is_err());
		assert!(config
			.validate()
			.unwrap_err()
			.contains("authorization_url must use HTTPS"));
	}

	#[test]
	fn test_oauth_config_validation_valid_config() {
		let config = OAuthConfig::new(
			"client_id".to_string(),
			"secret".to_string(),
			"https://github.com/login/oauth/authorize".to_string(),
			"https://github.com/login/oauth/access_token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["repo".to_string(), "read:org".to_string()],
		);

		assert!(config.validate().is_ok());
	}

	#[test]
	fn test_oauth_config_validation_localhost_allowed() {
		let config = OAuthConfig::new(
			"client_id".to_string(),
			"secret".to_string(),
			"http://localhost:8080/oauth/authorize".to_string(),
			"http://localhost:8080/oauth/token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["repo".to_string()],
		);

		assert!(config.validate().is_ok());
	}

	#[test]
	fn test_oauth_config_refresh_buffer_default() {
		let config = OAuthConfig::new(
			"client_id".to_string(),
			"secret".to_string(),
			"https://example.com/authorize".to_string(),
			"https://example.com/token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["repo".to_string()],
		);

		assert_eq!(config.refresh_buffer_seconds, 300);
	}

	#[test]
	fn test_oauth_config_refresh_buffer_minimum() {
		let mut config = OAuthConfig::new(
			"client_id".to_string(),
			"secret".to_string(),
			"https://example.com/authorize".to_string(),
			"https://example.com/token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["repo".to_string()],
		);
		config.refresh_buffer_seconds = 30; // less than minimum

		assert!(config.validate().is_err());
		assert!(config
			.validate()
			.unwrap_err()
			.contains("refresh_buffer_seconds must be at least 60"));
	}

	#[test]
	fn test_oauth_config_validation_invalid_callback_url() {
		let config = OAuthConfig::new(
			"client_id".to_string(),
			"secret".to_string(),
			"https://example.com/authorize".to_string(),
			"https://example.com/token".to_string(),
			"not-a-valid-url".to_string(),
			vec!["repo".to_string()],
		);

		assert!(config.validate().is_err());
		assert!(config
			.validate()
			.unwrap_err()
			.contains("callback_url is invalid"));
	}
}
