// Copyright 2025 Muvon Un Limited
//
// OAuth 2.1 + PKCE Flow Implementation

use crate::config::OAuthConfig;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::time::SystemTime;
use url::Url;
use uuid::Uuid;

const PKCE_CODE_VERIFIER_LENGTH: usize = 64;

/// Custom deserializer for OAuth scope field.
/// Handles both comma-separated strings (GitHub) and arrays (standard OAuth).
fn deserialize_scope<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
	D: Deserializer<'de>,
{
	let value = Option::<serde_json::Value>::deserialize(deserializer)?;

	match value {
		Some(serde_json::Value::String(s)) => {
			// GitHub and some OAuth providers return scope as comma-separated string
			let scopes: Vec<String> = s
				.split(',')
				.map(|s| s.trim().to_string())
				.filter(|s| !s.is_empty())
				.collect();
			Ok(Some(scopes))
		}
		Some(serde_json::Value::Array(arr)) => {
			// Standard OAuth returns scope as array
			let mut scopes = Vec::new();
			for v in arr {
				if let Some(s) = v.as_str() {
					scopes.push(s.to_string());
				}
			}
			Ok(Some(scopes))
		}
		_ => Ok(None),
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenResponse {
	pub access_token: String,
	#[serde(default)]
	pub token_type: String,
	#[serde(default)]
	pub expires_in: u64, // GitHub doesn't return this - tokens don't expire
	#[serde(default)]
	pub refresh_token: Option<String>,
	#[serde(default, deserialize_with = "deserialize_scope")]
	pub scope: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct PkcePair {
	pub code_verifier: String,
	pub code_challenge: String,
}

pub fn build_authorization_url(
	config: &OAuthConfig,
	code_challenge: &str,
	state: &str,
	redirect_uri: &str,
) -> String {
	let mut url =
		Url::parse(&config.authorization_url).expect("authorization_url should be validated");
	url.query_pairs_mut()
		.append_pair("client_id", &config.client_id)
		.append_pair("redirect_uri", redirect_uri)
		.append_pair("response_type", "code")
		.append_pair("code_challenge", code_challenge)
		.append_pair("code_challenge_method", "S256")
		.append_pair("state", state)
		.append_pair("scope", &config.scopes.join(" "));

	crate::log_debug!(
		"Building authorization URL - client_id: {}, scopes: {:?}, redirect_uri: {}",
		config.client_id,
		config.scopes,
		redirect_uri
	);

	url.to_string()
}

pub fn generate_pkce_pair() -> PkcePair {
	let bytes = [0u8; PKCE_CODE_VERIFIER_LENGTH];
	let code_verifier = URL_SAFE_NO_PAD.encode(bytes);
	// sha2 0.10 API: use Digest trait's digest method
	let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
	PkcePair {
		code_verifier,
		code_challenge,
	}
}

pub fn generate_state() -> String {
	Uuid::new_v4().to_string()
}

pub async fn exchange_code_for_token(
	config: &OAuthConfig,
	code: &str,
	code_verifier: &str,
	redirect_uri: &str,
) -> Result<OAuthTokenResponse, String> {
	let client = reqwest::Client::new();

	// Build request body - GitHub requires specific format
	// For public clients (PKCE), do NOT include client_secret
	let mut body = serde_json::json!({
		"grant_type": "authorization_code",
		"client_id": config.client_id,
		"code": code,
		"redirect_uri": redirect_uri,
		"code_verifier": code_verifier,
	});

	// Only add client_secret if it's not empty (confidential clients)
	if !config.client_secret.is_empty() {
		body["client_secret"] = serde_json::json!(config.client_secret);
	}

	crate::log_debug!(
		"Exchanging code for token - client_id: {}, redirect_uri: {}, has_secret: {}",
		config.client_id,
		redirect_uri,
		!config.client_secret.is_empty()
	);

	let response = client
		.post(&config.token_url)
		.header(reqwest::header::ACCEPT, "application/json")
		.json(&body)
		.send()
		.await
		.map_err(|e| format!("Network error: {}", e))?;

	let status = response.status();
	let text = response
		.text()
		.await
		.map_err(|e| format!("Read error: {}", e))?;

	crate::log_debug!(
		"Token exchange response - status: {}, body: {}",
		status,
		text
	);

	if !status.is_success() {
		// Try to parse OAuth error
		if let Ok(oauth_err) = serde_json::from_str::<OAuthErrorResponse>(&text) {
			return Err(format!(
				"{} - {}",
				oauth_err.error,
				oauth_err.error_description.unwrap_or_default()
			));
		}
		return Err(format!("Token request failed: {} - {}", status, text));
	}

	// Try to parse as JSON first
	match serde_json::from_str::<OAuthTokenResponse>(&text) {
		Ok(token) => Ok(token),
		Err(e) => {
			// GitHub might return URL-encoded response instead of JSON
			// Try parsing as form data
			crate::log_debug!("Failed to parse as JSON: {}, trying URL-encoded format", e);

			let params: std::collections::HashMap<String, String> =
				serde_urlencoded::from_str(&text)
					.map_err(|parse_err| format!("Invalid response format (not JSON or URL-encoded): JSON error: {}, URL-encoded error: {}", e, parse_err))?;

			// Convert URL-encoded params to OAuthTokenResponse
			let access_token = params
				.get("access_token")
				.ok_or_else(|| format!("Missing access_token in response: {}", text))?
				.clone();

			let token_type = params
				.get("token_type")
				.unwrap_or(&"Bearer".to_string())
				.clone();

			let expires_in = params
				.get("expires_in")
				.and_then(|s| s.parse::<u64>().ok())
				.unwrap_or(0); // Default to 0 if not provided

			let refresh_token = params.get("refresh_token").cloned();

			let scope = params
				.get("scope")
				.map(|s| s.split(',').map(|s| s.trim().to_string()).collect());

			Ok(OAuthTokenResponse {
				access_token,
				token_type,
				expires_in,
				refresh_token,
				scope,
			})
		}
	}
}

pub async fn refresh_access_token(
	config: &OAuthConfig,
	refresh_token: &str,
) -> Result<OAuthTokenResponse, String> {
	let client = reqwest::Client::new();

	// GitHub requires JSON body
	let body = serde_json::json!({
		"grant_type": "refresh_token",
		"client_id": config.client_id,
		"client_secret": config.client_secret,
		"refresh_token": refresh_token,
	});

	let response = client
		.post(&config.token_url)
		.header(reqwest::header::ACCEPT, "application/json")
		.json(&body)
		.send()
		.await
		.map_err(|e| format!("Network error: {}", e))?;

	let status = response.status();
	let text = response
		.text()
		.await
		.map_err(|e| format!("Read error: {}", e))?;

	if !status.is_success() {
		return Err(format!("Token refresh failed: {} - {}", status, text));
	}

	serde_json::from_str(&text).map_err(|e| format!("Invalid response: {}", e))
}

pub fn is_token_expired(expires_at: u64, buffer_seconds: u64) -> bool {
	let now = SystemTime::now()
		.duration_since(SystemTime::UNIX_EPOCH)
		.map(|d| d.as_secs())
		.unwrap_or(0);
	now + buffer_seconds >= expires_at
}

pub fn seconds_until_expiry(expires_at: u64) -> u64 {
	let now = SystemTime::now()
		.duration_since(SystemTime::UNIX_EPOCH)
		.map(|d| d.as_secs())
		.unwrap_or(0);
	if expires_at > now {
		expires_at.saturating_sub(now)
	} else {
		0
	}
}

#[derive(Debug, Serialize, Deserialize)]
struct OAuthErrorResponse {
	error: String,
	#[serde(default)]
	error_description: Option<String>,
}
