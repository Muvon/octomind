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
	let minimal: OAuthErrorResponse = serde_json::from_str(r#"{"error":"invalid_grant"}"#).unwrap();
	assert_eq!(minimal.error, "invalid_grant");
	assert_eq!(minimal.error_description, None);

	let full: OAuthErrorResponse =
		serde_json::from_str(r#"{"error":"invalid_grant","error_description":"code expired"}"#)
			.unwrap();
	assert_eq!(full.error_description.as_deref(), Some("code expired"));
}
