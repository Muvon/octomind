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
pub use callback_server::{start_callback_server, OAuthCallbackResult, SignInChannel};
pub use cimd::resolve_client_id;
pub use discovery::{clear_all_discovered_oauth_cache, clear_discovered_oauth_cache, discover_oauth_from_mcp_server};
pub use flow::{
	build_authorization_url, exchange_code_for_token, generate_pkce_pair, generate_state, is_token_expired,
	refresh_access_token, token_expires_at, OAuthTokenResponse, PkcePair,
};
pub use token_store::{clear_token, get_valid_token, load_token, save_token, TokenMetadata, TokenResult};

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

		let auth_url = Url::parse(&self.authorization_url)
			.map_err(|e| format!("OAuth authorization_url is invalid: {}. Must be a valid URL", e))?;

		if auth_url.scheme() != "https"
			&& auth_url.host_str() != Some("localhost")
			&& auth_url.host_str() != Some("127.0.0.1")
		{
			return Err("OAuth authorization_url must use HTTPS or http://localhost/http://127.0.0.1".to_string());
		}

		let token_url = Url::parse(&self.token_url)
			.map_err(|e| format!("OAuth token_url is invalid: {}. Must be a valid URL", e))?;

		if token_url.scheme() != "https"
			&& token_url.host_str() != Some("localhost")
			&& token_url.host_str() != Some("127.0.0.1")
		{
			return Err("OAuth token_url must use HTTPS or http://localhost/http://127.0.0.1".to_string());
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
pub async fn start_oauth_flow(config: &OAuthConfig, server_name: &str) -> Result<OAuthCallbackResult, anyhow::Error> {
	config
		.validate()
		.map_err(|e| anyhow::anyhow!("OAuth config validation failed: {}", e))?;
	let channel = sign_in_channel(server_name)?;

	let pkce = generate_pkce_pair();
	let state = generate_state();

	// Start callback server using the configured callback_url
	let result = start_callback_server(config, server_name, state, pkce.code_verifier, channel).await?;

	Ok(result)
}

/// A browser sign-in needs a person, so it goes to whoever answers the calling
/// session's elicitations. With nobody there (piped runs, sessionless startup or
/// health checks, clients without elicitation) it fails fast instead of opening
/// a browser no one watches and blocking on the callback.
fn sign_in_channel(server_name: &str) -> Result<SignInChannel, anyhow::Error> {
	let nobody = || {
		anyhow::anyhow!(
			"MCP server '{}' needs an OAuth sign-in, but nobody in this session can complete it — sign in once from an interactive octomind session",
			server_name
		)
	};
	let session_id = match crate::session::context::current_session_id() {
		Some(session_id) if !crate::agent::inputs::in_non_interactive_scope() => session_id,
		_ => return Err(nobody()),
	};
	match crate::mcp::elicitation::answerer(&session_id) {
		Some(crate::mcp::elicitation::Answerer::Terminal) => Ok(SignInChannel::Terminal),
		Some(crate::mcp::elicitation::Answerer::Client) => Ok(SignInChannel::Client { session_id }),
		None => Err(nobody()),
	}
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
	// One OAuth flow or refresh per server at a time
	let lock = server_lock(server_name).await;
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
			crate::log_debug!("⚠️  GET_ACCESS_TOKEN: No valid token found, trying refresh before OAuth flow");
		}
	}

	// A stored refresh token renews the access token without the user; only a
	// missing or rejected refresh token needs the browser sign-in.
	if let Some(access_token) = refresh_stored_token(config, server_name).await? {
		return Ok(Some(access_token));
	}

	// No usable token - start standard web OAuth flow (PKCE + callback)
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

/// Access token for `server_name` without user interaction: the stored token
/// while valid, else a refresh-token renewal. Never opens a browser — `Ok(None)`
/// means only an interactive sign-in can produce a token.
pub async fn get_access_token_non_interactive(
	config: &OAuthConfig,
	server_name: &str,
) -> Result<Option<String>, anyhow::Error> {
	let lock = server_lock(server_name).await;
	let _guard = lock.lock().await;

	let valid_token = get_valid_token(server_name, config.refresh_buffer_seconds)
		.await
		.map_err(|e| anyhow::anyhow!("Failed to check token: {}", e))?;
	if let Some(metadata) = valid_token {
		return Ok(Some(metadata.access_token));
	}
	refresh_stored_token(config, server_name).await
}

/// Per-server lock serializing sign-in and refresh, so a rotating refresh
/// token is never spent twice.
async fn server_lock(server_name: &str) -> Arc<Mutex<()>> {
	OAUTH_LOCKS
		.lock()
		.await
		.entry(server_name.to_string())
		.or_insert_with(|| Arc::new(Mutex::new(())))
		.clone()
}

/// Renew the stored access token with its refresh token (RFC 6749 §6).
/// `Ok(None)` when no refresh token is stored or the authorization server
/// rejects it; the rejection is logged because it is why the user is asked to
/// sign in again.
async fn refresh_stored_token(config: &OAuthConfig, server_name: &str) -> Result<Option<String>, anyhow::Error> {
	let stored = load_token(server_name)
		.await
		.map_err(|e| anyhow::anyhow!("Failed to load token: {}", e))?;
	let Some(stored) = stored else {
		return Ok(None);
	};
	let Some(refresh_token) = stored.refresh_token.clone() else {
		return Ok(None);
	};

	let response = match refresh_access_token(config, &refresh_token).await {
		Ok(response) => response,
		Err(e) => {
			crate::log_error!(
				"OAuth token refresh for '{}' rejected, sign-in required: {}",
				server_name,
				e
			);
			return Ok(None);
		}
	};

	let metadata = TokenMetadata {
		server_name: server_name.to_string(),
		access_token: response.access_token.clone(),
		// A server that does not rotate refresh tokens omits the field: the
		// stored one stays valid.
		refresh_token: response.refresh_token.or(Some(refresh_token)),
		expires_at: token_expires_at(response.expires_in),
		scopes: response.scope.unwrap_or(stored.scopes),
	};
	// Same contract as the sign-in callback: the token works for this process
	// either way, a failed save only means renewing again next time.
	if let Err(e) = save_token(server_name, &metadata).await {
		crate::log_error!("Failed to persist refreshed OAuth token for '{}': {}", server_name, e);
	}
	Ok(Some(response.access_token))
}

/// Checks if a server is authenticated.
pub async fn is_authenticated(server_name: &str, refresh_buffer_seconds: u64) -> bool {
	get_valid_token(server_name, refresh_buffer_seconds)
		.await
		.map(|m| m.is_some())
		.unwrap_or(false)
}

#[cfg(test)]
#[path = "mod_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod mod_tests;
