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

//! External unit tests for CIMD/DCR client-id resolution: `resolve_client_id`
//! strategy selection and DCR registration against a loopback stub.
//! Complements the inline `mod tests` (which covers `build_client_metadata` only).

use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn auth_metadata(cimd: Option<bool>, registration_endpoint: Option<String>) -> AuthServerMetadata {
	AuthServerMetadata {
		issuer: "https://auth.example.com".to_string(),
		authorization_endpoint: "https://auth.example.com/oauth/authorize".to_string(),
		token_endpoint: "https://auth.example.com/oauth/token".to_string(),
		scopes_supported: None,
		code_challenge_methods_supported: None,
		registration_endpoint,
		client_id_metadata_document_supported: cimd,
	}
}

fn oauth_config() -> OAuthConfig {
	OAuthConfig::new(
		String::new(),
		String::new(),
		"https://auth.example.com/oauth/authorize".to_string(),
		"https://auth.example.com/oauth/token".to_string(),
		"http://127.0.0.1:34567/oauth/callback".to_string(),
		vec!["mcp:read".to_string()],
	)
}

/// Loopback HTTP stub answering every request with `status` + JSON `body`.
async fn spawn_http_stub(status: &'static str, body: &'static str) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	tokio::spawn(async move {
		loop {
			let Ok((mut sock, _)) = listener.accept().await else {
				break;
			};
			tokio::spawn(async move {
				let mut buf = vec![0u8; 8192];
				let Ok(n) = sock.read(&mut buf).await else {
					return;
				};
				if n == 0 {
					return;
				}
				let response = format!(
					"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
					body.len(),
					body
				);
				let _ = sock.write_all(response.as_bytes()).await;
			});
		}
	});
	format!("http://{addr}/register")
}

/// Bind and drop a listener to obtain a guaranteed-closed local port.
async fn closed_port_url(path: &str) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let port = listener.local_addr().unwrap().port();
	drop(listener);
	format!("http://127.0.0.1:{port}/{path}")
}

// ------------------------------------------------------------------
// Serialization shapes
// ------------------------------------------------------------------

#[test]
fn dcr_client_metadata_omits_scope_when_empty() {
	let metadata = build_client_metadata("http://127.0.0.1:34567/oauth/callback", &[]);
	let json = serde_json::to_string(&metadata).unwrap();
	assert!(!json.contains("\"scope\""), "{json}");
}

#[test]
fn dcr_registration_response_defaults_optionals() {
	let minimal: DcrRegistrationResponse = serde_json::from_str(r#"{"client_id":"c1"}"#).unwrap();
	assert_eq!(minimal.client_id, "c1");
	assert!(minimal.client_secret.is_none());
	assert!(minimal.client_id_issued_at.is_none());
	assert!(minimal.client_secret_expires_at.is_none());

	let full: DcrRegistrationResponse = serde_json::from_str(
		r#"{"client_id":"c2","client_secret":"s","client_id_issued_at":1,"client_secret_expires_at":0}"#,
	)
	.unwrap();
	assert_eq!(full.client_secret.as_deref(), Some("s"));
	assert_eq!(full.client_id_issued_at, Some(1));
	assert_eq!(full.client_secret_expires_at, Some(0));
}

#[test]
fn dcr_registration_response_requires_client_id() {
	assert!(serde_json::from_str::<DcrRegistrationResponse>(r#"{"client_secret":"s"}"#).is_err());
}

// ------------------------------------------------------------------
// resolve_client_id — strategy selection
// ------------------------------------------------------------------

#[tokio::test]
async fn resolve_client_id_uses_published_document_when_cimd_supported() {
	let metadata = auth_metadata(Some(true), None);
	let config = resolve_client_id(oauth_config(), &metadata)
		.await
		.expect("CIMD resolution must succeed");
	assert_eq!(config.client_id, CLIENT_METADATA_URL);
	assert!(config.client_secret.is_empty(), "CIMD clients are public");
}

// MCP priority order: CIMD before DCR. The closed port proves no registration is attempted.
#[tokio::test]
async fn resolve_client_id_prefers_cimd_over_dcr() {
	let metadata = auth_metadata(Some(true), Some(closed_port_url("register").await));
	let config = resolve_client_id(oauth_config(), &metadata)
		.await
		.expect("CIMD resolution must succeed");
	assert_eq!(config.client_id, CLIENT_METADATA_URL);
}

#[test]
fn client_metadata_url_is_a_valid_cimd_client_id() {
	// CIMD client_ids must use https and carry a path
	let url = url::Url::parse(CLIENT_METADATA_URL).expect("valid URL");
	assert_eq!(url.scheme(), "https");
	assert!(url.path().len() > 1, "{CLIENT_METADATA_URL}");
}

#[tokio::test]
async fn resolve_client_id_registers_via_dcr_when_cimd_unsupported() {
	let endpoint = spawn_http_stub("200 OK", r#"{"client_id":"dcr-client-1","client_secret":"dcr-secret"}"#).await;
	let metadata = auth_metadata(Some(false), Some(endpoint));
	let config = resolve_client_id(oauth_config(), &metadata)
		.await
		.expect("DCR resolution must succeed");
	assert_eq!(config.client_id, "dcr-client-1");
	assert_eq!(config.client_secret, "dcr-secret");
}

#[tokio::test]
async fn resolve_client_id_surfaces_dcr_http_failure() {
	let endpoint = spawn_http_stub("400 Bad Request", r#"{"error":"invalid_client_metadata"}"#).await;
	let metadata = auth_metadata(None, Some(endpoint));
	let err = resolve_client_id(oauth_config(), &metadata).await.unwrap_err();
	assert!(
		err.to_string().contains("DCR registration failed with status 400"),
		"{err}"
	);
}

#[tokio::test]
async fn resolve_client_id_surfaces_dcr_connection_failure() {
	let endpoint = closed_port_url("register").await;
	let metadata = auth_metadata(None, Some(endpoint));
	let err = resolve_client_id(oauth_config(), &metadata).await.unwrap_err();
	assert!(err.to_string().contains("DCR registration failed"), "{err}");
}

#[tokio::test]
async fn resolve_client_id_fails_without_cimd_or_dcr() {
	let metadata = auth_metadata(Some(false), None);
	let err = resolve_client_id(oauth_config(), &metadata).await.unwrap_err();
	assert!(err.to_string().contains("Cannot resolve OAuth client_id"), "{err}");
	assert!(err.to_string().contains("auth.example.com"), "{err}");
}
