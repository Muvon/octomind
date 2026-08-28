// Copyright 2026 Muvon Un Limited
//
// OAuth 2.1 + PKCE Flow Implementation

use super::OAuthConfig;
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

	// RFC 9728 §2.1: include the resource parameter so the authorization
	// server can issue audience-scoped tokens.
	if let Some(resource) = &config.resource {
		url.query_pairs_mut().append_pair("resource", resource);
	}

	crate::log_debug!(
		"Building authorization URL - client_id: {}, scopes: {:?}, redirect_uri: {}, resource: {:?}",
		config.client_id,
		config.scopes,
		redirect_uri,
		config.resource
	);

	url.to_string()
}

pub fn generate_pkce_pair() -> PkcePair {
	// RFC 7636 requires a cryptographically random verifier — a constant one
	// defeats PKCE entirely (anyone intercepting the code can complete the
	// exchange). Fill from UUIDv4s: CSPRNG-backed, already a dependency.
	let mut bytes = [0u8; PKCE_CODE_VERIFIER_LENGTH];
	for chunk in bytes.chunks_mut(16) {
		let id = Uuid::new_v4();
		chunk.copy_from_slice(&id.as_bytes()[..chunk.len()]);
	}
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

	// RFC 9728 §2.1: include the resource parameter in the token exchange.
	if let Some(resource) = &config.resource {
		body["resource"] = serde_json::json!(resource);
	}

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

#[cfg(test)]
mod tests {
	use super::*;

	fn test_config() -> OAuthConfig {
		OAuthConfig::new(
			"flow-test-client".to_string(),
			String::new(),
			"https://auth.example.com/oauth/authorize".to_string(),
			"https://auth.example.com/oauth/token".to_string(),
			"http://localhost:34567/oauth/callback".to_string(),
			vec!["read".to_string(), "write".to_string()],
		)
	}

	fn now_secs() -> u64 {
		SystemTime::now()
			.duration_since(SystemTime::UNIX_EPOCH)
			.unwrap()
			.as_secs()
	}

	fn query_params(url: &str) -> std::collections::HashMap<String, String> {
		Url::parse(url)
			.unwrap()
			.query_pairs()
			.map(|(k, v)| (k.to_string(), v.to_string()))
			.collect()
	}

	// ------------------------------------------------------------------
	// PKCE + state generation
	// ------------------------------------------------------------------

	#[test]
	fn pkce_pair_has_cryptographic_verifier() {
		let pair = generate_pkce_pair();
		// 64 random bytes, base64url without padding → 86 chars
		assert_eq!(pair.code_verifier.len(), 86);
		let decoded = URL_SAFE_NO_PAD
			.decode(pair.code_verifier.as_bytes())
			.unwrap();
		assert_eq!(decoded.len(), PKCE_CODE_VERIFIER_LENGTH);
		// Verifier must be URL-safe (RFC 7636 unreserved charset)
		assert!(pair
			.code_verifier
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
	}

	#[test]
	fn pkce_challenge_is_s256_of_verifier() {
		let pair = generate_pkce_pair();
		// SHA-256 digest is 32 bytes → 43 base64url chars
		assert_eq!(pair.code_challenge.len(), 43);
		let decoded = URL_SAFE_NO_PAD
			.decode(pair.code_challenge.as_bytes())
			.unwrap();
		assert_eq!(decoded.len(), 32);
		assert_eq!(
			pair.code_challenge,
			URL_SAFE_NO_PAD.encode(Sha256::digest(pair.code_verifier.as_bytes()))
		);
	}

	#[test]
	fn pkce_pairs_are_unique() {
		let a = generate_pkce_pair();
		let b = generate_pkce_pair();
		assert_ne!(a.code_verifier, b.code_verifier);
		assert_ne!(a.code_challenge, b.code_challenge);
	}

	#[test]
	fn generate_state_is_unique_uuid() {
		let a = generate_state();
		let b = generate_state();
		assert!(!a.is_empty());
		assert_ne!(a, b);
		assert!(Uuid::parse_str(&a).is_ok(), "state should be a UUID: {a}");
	}

	// ------------------------------------------------------------------
	// build_authorization_url
	// ------------------------------------------------------------------

	#[test]
	fn build_authorization_url_contains_required_params() {
		let url = build_authorization_url(
			&test_config(),
			"test-challenge",
			"test-state",
			"http://localhost:34567/oauth/callback",
		);
		let parsed = Url::parse(&url).unwrap();
		assert_eq!(parsed.scheme(), "https");
		assert_eq!(parsed.host_str(), Some("auth.example.com"));
		assert_eq!(parsed.path(), "/oauth/authorize");

		let params = query_params(&url);
		assert_eq!(params.get("client_id").unwrap(), "flow-test-client");
		assert_eq!(
			params.get("redirect_uri").unwrap(),
			"http://localhost:34567/oauth/callback"
		);
		assert_eq!(params.get("response_type").unwrap(), "code");
		assert_eq!(params.get("code_challenge").unwrap(), "test-challenge");
		assert_eq!(params.get("code_challenge_method").unwrap(), "S256");
		assert_eq!(params.get("state").unwrap(), "test-state");
		// Scopes are joined with spaces
		assert_eq!(params.get("scope").unwrap(), "read write");
		assert!(
			!params.contains_key("resource"),
			"resource omitted when unset"
		);
	}

	#[test]
	fn build_authorization_url_appends_resource_when_set() {
		let mut config = test_config();
		config.resource = Some("https://api.example.com".to_string());
		let url = build_authorization_url(&config, "c", "s", "http://localhost:1/cb");
		let params = query_params(&url);
		assert_eq!(params.get("resource").unwrap(), "https://api.example.com");
	}

	// ------------------------------------------------------------------
	// Token expiry helpers
	// ------------------------------------------------------------------

	#[test]
	fn is_token_expired_past_and_future() {
		let now = now_secs();
		assert!(is_token_expired(now - 3600, 0), "past expiry is expired");
		assert!(
			is_token_expired(now, 0),
			"expiring now counts as expired (>=)"
		);
		assert!(
			!is_token_expired(now + 3600, 60),
			"future expiry with small buffer is valid"
		);
	}

	#[test]
	fn is_token_expired_respects_buffer() {
		let now = now_secs();
		// The buffer pulls the effective expiry earlier: now+1000 with a
		// 2000s buffer is already inside the refresh window.
		assert!(is_token_expired(now + 1000, 2000));
		// Far-future expiry stays valid under the same buffer
		assert!(!is_token_expired(now + 10_000, 2000));
	}

	#[test]
	fn seconds_until_expiry_future_and_elapsed() {
		let now = now_secs();
		let remaining = seconds_until_expiry(now + 1000);
		// A second may tick between now_secs() and the call
		assert!((990..=1000).contains(&remaining), "remaining: {remaining}");
		assert_eq!(seconds_until_expiry(now - 1000), 0);
		assert_eq!(seconds_until_expiry(now), 0);
	}

	// ------------------------------------------------------------------
	// OAuthTokenResponse deserialization
	// ------------------------------------------------------------------

	#[test]
	fn token_response_parses_standard_json() {
		let response: OAuthTokenResponse = serde_json::from_str(
			r#"{"access_token":"at","token_type":"Bearer","expires_in":3600,"refresh_token":"rt","scope":["read","write"]}"#,
		)
		.unwrap();
		assert_eq!(response.access_token, "at");
		assert_eq!(response.token_type, "Bearer");
		assert_eq!(response.expires_in, 3600);
		assert_eq!(response.refresh_token.as_deref(), Some("rt"));
		assert_eq!(
			response.scope,
			Some(vec!["read".to_string(), "write".to_string()])
		);
	}

	#[test]
	fn token_response_parses_github_style_defaults() {
		// GitHub omits token_type/expires_in and returns a comma-separated
		// scope string.
		let response: OAuthTokenResponse =
			serde_json::from_str(r#"{"access_token":"gho_at","scope":"repo, user"}"#).unwrap();
		assert_eq!(response.token_type, "");
		assert_eq!(response.expires_in, 0);
		assert_eq!(response.refresh_token, None);
		assert_eq!(
			response.scope,
			Some(vec!["repo".to_string(), "user".to_string()])
		);
	}

	#[test]
	fn token_response_scope_edge_cases() {
		// Absent scope → None
		let absent: OAuthTokenResponse = serde_json::from_str(r#"{"access_token":"at"}"#).unwrap();
		assert_eq!(absent.scope, None);

		// Explicit null → None
		let null: OAuthTokenResponse =
			serde_json::from_str(r#"{"access_token":"at","scope":null}"#).unwrap();
		assert_eq!(null.scope, None);

		// Comma string: whitespace trimmed, empty segments dropped
		let messy: OAuthTokenResponse =
			serde_json::from_str(r#"{"access_token":"at","scope":" a , ,b "}"#).unwrap();
		assert_eq!(messy.scope, Some(vec!["a".to_string(), "b".to_string()]));

		// Array with non-string entries: only strings kept
		let mixed: OAuthTokenResponse =
			serde_json::from_str(r#"{"access_token":"at","scope":["read", 42]}"#).unwrap();
		assert_eq!(mixed.scope, Some(vec!["read".to_string()]));
	}

	#[test]
	fn token_response_requires_access_token() {
		assert!(serde_json::from_str::<OAuthTokenResponse>(r#"{"token_type":"Bearer"}"#).is_err());
	}

	#[test]
	fn oauth_error_response_deserializes() {
		let minimal: OAuthErrorResponse =
			serde_json::from_str(r#"{"error":"invalid_grant"}"#).unwrap();
		assert_eq!(minimal.error, "invalid_grant");
		assert_eq!(minimal.error_description, None);

		let full: OAuthErrorResponse =
			serde_json::from_str(r#"{"error":"invalid_grant","error_description":"code expired"}"#)
				.unwrap();
		assert_eq!(full.error_description.as_deref(), Some("code expired"));
	}
}
