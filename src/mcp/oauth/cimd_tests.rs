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

//! External unit tests for CIMD/DCR client-id resolution: the local CIMD
//! metadata HTTP server (GET/OPTIONS/404), `resolve_client_id` strategy
//! selection, and DCR registration against a loopback stub. Complements the
//! inline `mod tests` (which covers `build_client_metadata` only).

use super::*;
use serial_test::serial;
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
fn client_metadata_document_omits_scope_when_empty() {
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
// CIMD HTTP server (global CIMD_SERVER state → serial)
// ------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn cimd_server_serves_metadata_document_over_http() {
	let client_id_url = start_cimd_server(
		"http://127.0.0.1:34567/oauth/callback",
		&["mcp:read".to_string()],
	)
	.await
	.expect("CIMD server should start");
	assert!(
		client_id_url.starts_with("http://localhost:"),
		"{client_id_url}"
	);
	assert!(client_id_url.ends_with("/.well-known/oauth-client.json"));

	// The listener binds 127.0.0.1; swap the advertised host for the fetch.
	let fetch_url = client_id_url.replacen("localhost", "127.0.0.1", 1);
	let response = reqwest::get(&fetch_url)
		.await
		.expect("fetch metadata document");
	assert_eq!(response.status(), 200);
	assert_eq!(
		response
			.headers()
			.get("access-control-allow-origin")
			.and_then(|v| v.to_str().ok()),
		Some("*"),
		"CIMD responses must carry permissive CORS headers"
	);
	let doc: ClientMetadataDocument = response.json().await.expect("parse metadata document");
	assert_eq!(doc.client_name, "Octomind");
	assert_eq!(
		doc.redirect_uris,
		vec!["http://127.0.0.1:34567/oauth/callback".to_string()]
	);
	assert_eq!(doc.grant_types, vec!["authorization_code".to_string()]);
	assert_eq!(doc.token_endpoint_auth_method, "none");
	assert_eq!(doc.scope.as_deref(), Some("mcp:read"));

	stop_cimd_server().await;
}

#[serial]
#[tokio::test]
async fn cimd_server_answers_preflight_and_rejects_unknown_paths() {
	let client_id_url = start_cimd_server("http://127.0.0.1:34567/oauth/callback", &[])
		.await
		.unwrap();
	let fetch_url = client_id_url.replacen("localhost", "127.0.0.1", 1);
	let parsed = url::Url::parse(&fetch_url).unwrap();
	let addr = format!("{}:{}", parsed.host_str().unwrap(), parsed.port().unwrap());

	// OPTIONS preflight → 204 with CORS headers.
	let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
	stream
		.write_all(b"OPTIONS / HTTP/1.1\r\nHost: localhost\r\n\r\n")
		.await
		.unwrap();
	let mut buf = vec![0u8; 2048];
	let n = stream.read(&mut buf).await.unwrap();
	let response = String::from_utf8_lossy(&buf[..n]).to_string();
	assert!(response.starts_with("HTTP/1.1 204"), "{response}");
	assert!(
		response
			.to_lowercase()
			.contains("access-control-allow-origin: *"),
		"{response}"
	);

	// Unknown path → 404.
	let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
	stream
		.write_all(b"GET /other HTTP/1.1\r\nHost: localhost\r\n\r\n")
		.await
		.unwrap();
	let n = stream.read(&mut buf).await.unwrap();
	let response = String::from_utf8_lossy(&buf[..n]).to_string();
	assert!(response.starts_with("HTTP/1.1 404"), "{response}");

	stop_cimd_server().await;
}

#[tokio::test]
async fn stop_cimd_server_without_running_instance_is_noop() {
	stop_cimd_server().await;
	stop_cimd_server().await;
}

// ------------------------------------------------------------------
// resolve_client_id — strategy selection
// ------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn resolve_client_id_prefers_cimd_when_supported() {
	let metadata = auth_metadata(Some(true), None);
	let config = resolve_client_id(oauth_config(), &metadata)
		.await
		.expect("CIMD resolution must succeed");
	assert!(
		config.client_id.starts_with("http://localhost:"),
		"{}",
		config.client_id
	);
	assert!(config.client_id.ends_with("/.well-known/oauth-client.json"));
	stop_cimd_server().await;
}

#[tokio::test]
async fn resolve_client_id_registers_via_dcr_when_cimd_unsupported() {
	let endpoint = spawn_http_stub(
		"200 OK",
		r#"{"client_id":"dcr-client-1","client_secret":"dcr-secret"}"#,
	)
	.await;
	let metadata = auth_metadata(Some(false), Some(endpoint));
	let config = resolve_client_id(oauth_config(), &metadata)
		.await
		.expect("DCR resolution must succeed");
	assert_eq!(config.client_id, "dcr-client-1");
	assert_eq!(config.client_secret, "dcr-secret");
}

#[tokio::test]
async fn resolve_client_id_surfaces_dcr_http_failure() {
	let endpoint =
		spawn_http_stub("400 Bad Request", r#"{"error":"invalid_client_metadata"}"#).await;
	let metadata = auth_metadata(None, Some(endpoint));
	let err = resolve_client_id(oauth_config(), &metadata)
		.await
		.unwrap_err();
	assert!(
		err.to_string()
			.contains("DCR registration failed with status 400"),
		"{err}"
	);
}

#[tokio::test]
async fn resolve_client_id_surfaces_dcr_connection_failure() {
	let endpoint = closed_port_url("register").await;
	let metadata = auth_metadata(None, Some(endpoint));
	let err = resolve_client_id(oauth_config(), &metadata)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("DCR registration failed"), "{err}");
}

#[tokio::test]
async fn resolve_client_id_fails_without_cimd_or_dcr() {
	let metadata = auth_metadata(Some(false), None);
	let err = resolve_client_id(oauth_config(), &metadata)
		.await
		.unwrap_err();
	assert!(
		err.to_string().contains("Cannot resolve OAuth client_id"),
		"{err}"
	);
	assert!(err.to_string().contains("auth.example.com"), "{err}");
}
