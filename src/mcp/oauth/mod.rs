// Copyright 2025 Muvon Un Limited
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
//! Supports both:
//! - MCP Authorization Discovery (RFC 9728) - automatic OAuth endpoint discovery
//! - Device Flow (RFC 8628) - for CLI tools without web browser
//! - Manual OAuth configuration - fallback for non-MCP-compliant servers

pub mod callback_server;
pub mod device_flow;
pub mod discovery;
pub mod flow;
pub mod token_store;

// Re-export commonly used types
pub use callback_server::{start_callback_server, OAuthCallbackResult};
pub use device_flow::{
	execute_device_flow, start_device_flow, DeviceCodeResponse, DeviceTokenResponse,
};
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

use crate::config::OAuthConfig;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// Global lock to prevent concurrent OAuth flows for the same client_id
lazy_static::lazy_static! {
	static ref OAUTH_LOCKS: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>> = Arc::new(Mutex::new(HashMap::new()));
}

/// Starts the complete OAuth authorization flow.
pub async fn start_oauth_flow(config: &OAuthConfig) -> Result<OAuthCallbackResult, anyhow::Error> {
	config
		.validate()
		.map_err(|e| anyhow::anyhow!("OAuth config validation failed: {}", e))?;

	let pkce = generate_pkce_pair();
	let state = generate_state();

	// Start callback server using the configured callback_url
	let result = start_callback_server(config, state, pkce.code_verifier).await?;

	Ok(result)
}

/// Gets an access token for the given OAuth configuration and server name.
/// Uses a per-server lock to prevent concurrent OAuth flows.
///
/// # Arguments
/// * `config` - OAuth configuration (either discovered or manual)
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

	// No valid token - start OAuth flow (protected by lock)

	// Use Device Flow for GitHub (RFC 8628) - best for CLI tools
	let issuer = &config.authorization_url;
	if issuer.contains("github.com") {
		crate::log_debug!(
			"🔍 Using GitHub Device Flow for authentication, server_name='{}'",
			server_name
		);
		crate::log_debug!(
			"Using GitHub Device Flow for authentication, server_name='{}'",
			server_name
		);

		crate::log_debug!("🔍 Calling execute_device_flow...");
		let access_token = execute_device_flow(config, server_name)
			.await
			.map_err(|e| anyhow::anyhow!("Device flow failed: {}", e))?;

		crate::log_debug!(
			"✅ Device flow returned! access_token prefix='{}...'",
			access_token.chars().take(10).collect::<String>()
		);
		crate::log_debug!(
			"Device flow completed, access_token prefix='{}...'",
			access_token.chars().take(10).collect::<String>()
		);

		// Save the token
		let metadata = TokenMetadata {
			server_name: server_name.to_string(),
			access_token: access_token.clone(),
			refresh_token: None,
			expires_at: 0, // GitHub tokens don't expire
			scopes: config.scopes.clone(),
		};

		crate::log_debug!("🔍 Saving token for server_name='{}'...", server_name);
		if let Err(e) = save_token(server_name, &metadata).await {
			crate::log_debug!("❌ Failed to save token: {}", e);
			crate::log_error!("Failed to save token: {}", e);
		} else {
			crate::log_debug!("✅ Token save completed");
		}

		// Verify the token was saved
		crate::log_debug!("🔍 Verifying token was saved...");
		match load_token(server_name).await {
			Ok(Some(saved)) => {
				crate::log_debug!(
					"✅ Token verified saved: server_name='{}', token_prefix='{}...'",
					server_name,
					saved.access_token.chars().take(10).collect::<String>()
				);
				crate::log_debug!(
					"Token verified saved: server_name='{}', token_prefix='{}...'",
					server_name,
					saved.access_token.chars().take(10).collect::<String>()
				);
			}
			Ok(None) => {
				crate::log_debug!("❌ Token was NOT found in storage after save attempt!");
				crate::log_error!("Token was NOT found in storage after save attempt!");
			}
			Err(e) => {
				crate::log_debug!("❌ Failed to verify token storage: {}", e);
				crate::log_error!("Failed to verify token storage: {}", e);
			}
		}

		return Ok(Some(access_token));
	}

	// Use regular web flow for other providers
	let result = start_oauth_flow(config).await?;

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
