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

//! Client registration per the MCP Authorization specification, in its priority order:
//! 1. Client ID Metadata Document (CIMD): when the authorization server advertises
//!    `client_id_metadata_document_supported`, the client_id is the HTTPS URL of Octomind's
//!    published metadata document; the server fetches it and validates the redirect URI
//! 2. Dynamic Client Registration (RFC 7591) through the server's `registration_endpoint`
//! 3. Neither available: discovery fails with a clear error

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use super::discovery::AuthServerMetadata;
use super::OAuthConfig;

/// Octomind's published client metadata document, served by octomind.run. Its URL is the
/// client_id under CIMD, so it must never change: authorization servers key consent and
/// tokens to it. The document lists the loopback callback (`http://localhost:34567/oauth/callback`)
/// as its redirect URI, the public-client auth method `none`, and the
/// `authorization_code` + `refresh_token` grants.
pub const CLIENT_METADATA_URL: &str = "https://octomind.run/oauth/client-metadata.json";

/// Client metadata sent in a DCR registration request (RFC 7591 section 2)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DcrClientMetadata {
	/// Client name displayed to the user during authorization
	pub client_name: String,

	/// Redirect URIs where the auth server can send the user after authorization
	pub redirect_uris: Vec<String>,

	/// OAuth grant types this client uses
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

/// Build the DCR client metadata for this Octomind instance
fn build_client_metadata(callback_url: &str, scopes: &[String]) -> DcrClientMetadata {
	DcrClientMetadata {
		client_name: "Octomind".to_string(),
		redirect_uris: vec![callback_url.to_string()],
		// Stored tokens are refreshed, and RFC 7591 asks clients to register every grant they use.
		grant_types: vec!["authorization_code".to_string(), "refresh_token".to_string()],
		token_endpoint_auth_method: "none".to_string(),
		scope: if scopes.is_empty() {
			None
		} else {
			Some(scopes.join(" "))
		},
	}
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
		.context(format!("Failed to register client at {}", registration_endpoint))?;

	if !response.status().is_success() {
		let status = response.status();
		let body = response.text().await.unwrap_or_default();
		return Err(anyhow!("DCR registration failed with status {}: {}", status, body));
	}

	let dcr_response: DcrRegistrationResponse = response
		.json()
		.await
		.context("Failed to parse DCR registration response")?;

	crate::log_debug!("DCR registration successful: client_id={}", dcr_response.client_id);

	Ok(dcr_response)
}

/// Resolve the client_id for an OAuth configuration using CIMD or DCR.
///
/// # Strategy (MCP Authorization spec priority order)
/// 1. CIMD (`client_id_metadata_document_supported: true`): `CLIENT_METADATA_URL`
/// 2. DCR (RFC 7591) via `registration_endpoint`
/// 3. If neither is available, return an error — no fallback to hardcoded IDs
///
/// # Arguments
/// * `oauth_config` - The OAuth config with empty client_id (will be populated)
/// * `auth_metadata` - Authorization server metadata (drives CIMD/DCR decision)
///
/// # Returns
/// * `Ok(OAuthConfig)` - Config with resolved client_id
/// * `Err` - If neither CIMD nor DCR is available
pub async fn resolve_client_id(oauth_config: OAuthConfig, auth_metadata: &AuthServerMetadata) -> Result<OAuthConfig> {
	// Strategy 1: CIMD — the published metadata document URL is the client_id
	if auth_metadata.client_id_metadata_document_supported.unwrap_or(false) {
		crate::log_debug!("Auth server supports CIMD, using client_id: {}", CLIENT_METADATA_URL);
		return Ok(OAuthConfig {
			client_id: CLIENT_METADATA_URL.to_string(),
			..oauth_config
		});
	}

	// Strategy 2: DCR — register client via registration_endpoint
	if let Some(ref registration_endpoint) = auth_metadata.registration_endpoint {
		crate::log_debug!("Auth server provides DCR endpoint: {}", registration_endpoint);

		match register_via_dcr(registration_endpoint, &oauth_config.callback_url, &oauth_config.scopes).await {
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
