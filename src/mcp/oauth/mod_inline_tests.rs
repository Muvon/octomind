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
