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

#[test]
fn test_parse_bearer_challenge() {
	let header = r#"Bearer resource_metadata="https://api.example.com/.well-known/oauth-protected-resource""#;
	let challenge = parse_bearer_challenge(header);
	assert_eq!(
		challenge.resource_metadata.as_deref(),
		Some("https://api.example.com/.well-known/oauth-protected-resource")
	);
	assert!(challenge.scopes.is_empty());
}

#[test]
fn test_parse_bearer_challenge_without_params() {
	assert_eq!(
		parse_bearer_challenge("Bearer realm=\"example\""),
		BearerChallenge::default()
	);
}

#[test]
fn test_parse_bearer_challenge_scopes() {
	let header = r#"Bearer resource_metadata="https://mcp.example.com/prm", scope="files:read files:write""#;
	let challenge = parse_bearer_challenge(header);
	assert_eq!(
		challenge.scopes,
		vec!["files:read".to_string(), "files:write".to_string()]
	);
	// An error code naming a scope is not a scope parameter
	let header = r#"Bearer error="insufficient_scope", resource_metadata="https://mcp.example.com/prm""#;
	assert!(parse_bearer_challenge(header).scopes.is_empty());
}

#[test]
fn test_build_oauth_config() {
	let auth_metadata = AuthServerMetadata {
		issuer: "https://api.example.com".to_string(),
		authorization_endpoint: "https://api.example.com/oauth/authorize".to_string(),
		token_endpoint: "https://api.example.com/oauth/token".to_string(),
		scopes_supported: Some(vec!["read".to_string(), "write".to_string()]),
		code_challenge_methods_supported: Some(vec!["S256".to_string()]),
		registration_endpoint: None,
		client_id_metadata_document_supported: None,
	};

	let resource_metadata = ProtectedResourceMetadata {
		resource: "https://api.example.com".to_string(),
		authorization_servers: vec!["https://api.example.com".to_string()],
		scopes_supported: None,
	};

	let config = build_oauth_config_from_metadata(&auth_metadata, &resource_metadata, &[]);

	// client_id is empty placeholder — resolved by CIMD/DCR
	assert!(config.client_id.is_empty());
	assert_eq!(config.authorization_url, "https://api.example.com/oauth/authorize");
	assert_eq!(config.token_url, "https://api.example.com/oauth/token");
	assert_eq!(config.scopes, vec!["read", "write"]);
	// Public client - no secret
	assert!(config.client_secret.is_empty());
}

// ------------------------------------------------------------------
// WWW-Authenticate header edge cases
// ------------------------------------------------------------------

#[test]
fn test_parse_header_edge_cases() {
	let resource_metadata = |header: &str| parse_bearer_challenge(header).resource_metadata;
	// Empty header
	assert!(resource_metadata("").is_none());
	// Multiple params — resource_metadata still extracted
	let header =
		"Bearer realm=\"example\", resource_metadata=\"https://auth.example.com/.well-known/oauth-protected-resource\"";
	assert_eq!(
		resource_metadata(header).as_deref(),
		Some("https://auth.example.com/.well-known/oauth-protected-resource")
	);
	// Unquoted value does not match the RFC 9728 format
	assert!(resource_metadata("Bearer resource_metadata=https://example.com/prm").is_none());
	// Empty quoted value does not match
	assert!(resource_metadata("Bearer resource_metadata=\"\"").is_none());
	// URL with query string survives intact
	let header = "Bearer resource_metadata=\"https://example.com/prm?foo=bar&baz=1\"";
	assert_eq!(
		resource_metadata(header).as_deref(),
		Some("https://example.com/prm?foo=bar&baz=1")
	);
}

#[test]
fn test_protected_resource_metadata_urls_follow_spec_order() {
	assert_eq!(
		protected_resource_metadata_urls("https://example.com/public/mcp/"),
		vec![
			"https://example.com/.well-known/oauth-protected-resource/public/mcp".to_string(),
			"https://example.com/.well-known/oauth-protected-resource".to_string(),
		]
	);
	assert_eq!(
		protected_resource_metadata_urls("https://example.com"),
		vec!["https://example.com/.well-known/oauth-protected-resource".to_string()]
	);
}

#[test]
fn test_auth_server_metadata_urls_follow_spec_order() {
	assert_eq!(
		auth_server_metadata_urls("https://auth.example.com/tenant1"),
		vec![
			"https://auth.example.com/.well-known/oauth-authorization-server/tenant1".to_string(),
			"https://auth.example.com/.well-known/openid-configuration/tenant1".to_string(),
			"https://auth.example.com/tenant1/.well-known/openid-configuration".to_string(),
		]
	);
	assert_eq!(
		auth_server_metadata_urls("https://auth.example.com/"),
		vec![
			"https://auth.example.com/.well-known/oauth-authorization-server".to_string(),
			"https://auth.example.com/.well-known/openid-configuration".to_string(),
		]
	);
}

#[test]
fn test_ensure_pkce_s256() {
	let with = |methods: Option<Vec<&str>>| AuthServerMetadata {
		code_challenge_methods_supported: methods.map(|m| m.into_iter().map(str::to_string).collect()),
		..auth_metadata_fixture(None, None, None)
	};
	assert!(ensure_pkce_s256(&with(Some(vec!["plain", "S256"]))).is_ok());
	let err = ensure_pkce_s256(&with(None)).unwrap_err();
	assert!(err.to_string().contains("PKCE S256"), "{err}");
	assert!(ensure_pkce_s256(&with(Some(vec!["plain"]))).is_err());
}

// ------------------------------------------------------------------
// Metadata struct deserialization
// ------------------------------------------------------------------

#[test]
fn test_protected_resource_metadata_deserialization() {
	let minimal: ProtectedResourceMetadata = serde_json::from_str(
		r#"{"resource":"https://api.example.com","authorization_servers":["https://auth.example.com"]}"#,
	)
	.unwrap();
	assert_eq!(minimal.resource, "https://api.example.com");
	assert_eq!(
		minimal.authorization_servers,
		vec!["https://auth.example.com".to_string()]
	);
	assert!(minimal.scopes_supported.is_none(), "scopes are optional");

	let full: ProtectedResourceMetadata = serde_json::from_str(
			r#"{"resource":"https://api.example.com","authorization_servers":["https://a.example.com","https://b.example.com"],"scopes_supported":["mcp:read","mcp:write"]}"#,
		)
		.unwrap();
	assert_eq!(
		full.scopes_supported,
		Some(vec!["mcp:read".to_string(), "mcp:write".to_string()])
	);
	assert_eq!(full.authorization_servers.len(), 2);

	// authorization_servers is required
	assert!(serde_json::from_str::<ProtectedResourceMetadata>(r#"{"resource":"https://api.example.com"}"#).is_err());
}

#[test]
fn test_auth_server_metadata_deserialization() {
	let minimal: AuthServerMetadata = serde_json::from_str(
			r#"{"issuer":"https://auth.example.com","authorization_endpoint":"https://auth.example.com/authorize","token_endpoint":"https://auth.example.com/token"}"#,
		)
		.unwrap();
	assert_eq!(minimal.issuer, "https://auth.example.com");
	assert!(minimal.scopes_supported.is_none());
	assert!(minimal.code_challenge_methods_supported.is_none());
	assert!(minimal.registration_endpoint.is_none());
	assert!(minimal.client_id_metadata_document_supported.is_none());

	let full: AuthServerMetadata = serde_json::from_str(
			r#"{"issuer":"https://auth.example.com","authorization_endpoint":"https://auth.example.com/authorize","token_endpoint":"https://auth.example.com/token","scopes_supported":["read"],"code_challenge_methods_supported":["S256"],"registration_endpoint":"https://auth.example.com/register","client_id_metadata_document_supported":true}"#,
		)
		.unwrap();
	assert_eq!(full.scopes_supported, Some(vec!["read".to_string()]));
	assert_eq!(full.code_challenge_methods_supported, Some(vec!["S256".to_string()]));
	assert_eq!(
		full.registration_endpoint.as_deref(),
		Some("https://auth.example.com/register")
	);
	assert_eq!(full.client_id_metadata_document_supported, Some(true));

	// token_endpoint is required
	assert!(serde_json::from_str::<AuthServerMetadata>(
		r#"{"issuer":"https://auth.example.com","authorization_endpoint":"https://auth.example.com/authorize"}"#
	)
	.is_err());
}

// ------------------------------------------------------------------
// build_oauth_config_from_metadata — scope precedence and defaults
// ------------------------------------------------------------------

fn auth_metadata_fixture(
	scopes: Option<Vec<String>>,
	registration_endpoint: Option<String>,
	cimd_supported: Option<bool>,
) -> AuthServerMetadata {
	AuthServerMetadata {
		issuer: "https://auth.example.com".to_string(),
		authorization_endpoint: "https://auth.example.com/oauth/authorize".to_string(),
		token_endpoint: "https://auth.example.com/oauth/token".to_string(),
		scopes_supported: scopes,
		code_challenge_methods_supported: Some(vec!["S256".to_string()]),
		registration_endpoint,
		client_id_metadata_document_supported: cimd_supported,
	}
}

fn resource_metadata_fixture(scopes: Option<Vec<String>>) -> ProtectedResourceMetadata {
	ProtectedResourceMetadata {
		resource: "https://api.example.com".to_string(),
		authorization_servers: vec!["https://auth.example.com".to_string()],
		scopes_supported: scopes,
	}
}

#[test]
fn test_build_oauth_config_scope_precedence() {
	let auth = auth_metadata_fixture(Some(vec!["read".to_string()]), None, None);

	let resource = resource_metadata_fixture(Some(vec!["mcp:read".to_string()]));

	// Challenge scopes are authoritative
	let config = build_oauth_config_from_metadata(&auth, &resource, &["files:write".to_string()]);
	assert_eq!(config.scopes, vec!["files:write".to_string()]);

	// Resource scopes win over auth server scopes
	let config = build_oauth_config_from_metadata(&auth, &resource, &[]);
	assert_eq!(config.scopes, vec!["mcp:read".to_string()]);

	// Falls back to auth server scopes when the resource lists none
	let config = build_oauth_config_from_metadata(&auth, &resource_metadata_fixture(None), &[]);
	assert_eq!(config.scopes, vec!["read".to_string()]);

	// Empty when neither document lists scopes
	let auth_no_scopes = auth_metadata_fixture(None, None, None);
	let config = build_oauth_config_from_metadata(&auth_no_scopes, &resource_metadata_fixture(None), &[]);
	assert!(config.scopes.is_empty());
}

#[test]
fn test_build_oauth_config_defaults() {
	let auth = auth_metadata_fixture(None, Some("https://auth.example.com/register".to_string()), Some(true));
	let config = build_oauth_config_from_metadata(&auth, &resource_metadata_fixture(None), &[]);

	// client_id is a placeholder resolved later by CIMD/DCR
	assert!(config.client_id.is_empty());
	assert!(config.client_secret.is_empty());
	assert_eq!(config.authorization_url, "https://auth.example.com/oauth/authorize");
	assert_eq!(config.token_url, "https://auth.example.com/oauth/token");
	assert_eq!(config.callback_url, "http://localhost:34567/oauth/callback");
	assert_eq!(config.refresh_buffer_seconds, 300);
	assert!(config.state.is_none());
	assert_eq!(config.resource.as_deref(), Some("https://api.example.com"));
}

// ------------------------------------------------------------------
// Cache management
// ------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn test_cache_clear_helpers_never_panic() {
	// Clearing an untouched entry and an empty cache must be safe
	clear_discovered_oauth_cache("never-populated-server");
	clear_all_discovered_oauth_cache();
	assert!(cached_discovery("never-populated-server").is_none());
}

// ------------------------------------------------------------------
// HTTP stub for RFC 9728 / RFC 8414 discovery flows
// ------------------------------------------------------------------

/// Spawn a local HTTP server routing (method, path, host) to a response.
/// Returns the base URL, e.g. "http://127.0.0.1:12345".
async fn spawn_http_stub<F>(handler: F) -> String
where
	F: Fn(&str, &str, &str) -> (u16, Vec<(String, String)>, String) + Send + Sync + 'static,
{
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	let handler = std::sync::Arc::new(handler);
	tokio::spawn(async move {
		loop {
			let Ok((mut sock, _)) = listener.accept().await else {
				break;
			};
			let handler = handler.clone();
			tokio::spawn(async move {
				let mut buf = vec![0u8; 16384];
				let n = match sock.read(&mut buf).await {
					Ok(n) if n > 0 => n,
					_ => return,
				};
				let request = String::from_utf8_lossy(&buf[..n]).to_string();
				let mut parts = request.lines().next().unwrap_or("").split_whitespace();
				let method = parts.next().unwrap_or("").to_string();
				let path = parts.next().unwrap_or("").to_string();
				let host = request
					.lines()
					.find_map(|l| {
						let (key, value) = l.split_once(':')?;
						key.eq_ignore_ascii_case("host").then(|| value.trim().to_string())
					})
					.unwrap_or_default();
				let (status, headers, body) = handler(&method, &path, &host);
				let reason = match status {
					200 => "OK",
					401 => "Unauthorized",
					404 => "Not Found",
					_ => "Error",
				};
				let mut response = format!(
						"HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
						body.len()
					);
				for (name, value) in &headers {
					response.push_str(&format!("{name}: {value}\r\n"));
				}
				response.push_str("\r\n");
				response.push_str(&body);
				let _ = sock.write_all(response.as_bytes()).await;
			});
		}
	});
	format!("http://{addr}")
}

fn unique_server_name(prefix: &str) -> String {
	format!("{prefix}-{}", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn test_fetch_protected_resource_metadata_success_and_errors() {
	let body =
			r#"{"resource":"https://api.example.com","authorization_servers":["https://auth.example.com"],"scopes_supported":["mcp"]}"#
				.to_string();
	let url = spawn_http_stub(move |_m, _p, _h| (200, vec![], body.clone())).await;
	let metadata = fetch_protected_resource_metadata(&url).await.unwrap();
	assert_eq!(metadata.resource, "https://api.example.com");
	assert_eq!(metadata.scopes_supported, Some(vec!["mcp".to_string()]));

	// Non-success status is an error naming the status
	let url = spawn_http_stub(move |_m, _p, _h| (404, vec![], String::new())).await;
	let err = fetch_protected_resource_metadata(&url).await.unwrap_err();
	assert!(err.to_string().contains("status"), "{err}");

	// Invalid JSON body is a parse error
	let url = spawn_http_stub(move |_m, _p, _h| (200, vec![], "not json".to_string())).await;
	let err = fetch_protected_resource_metadata(&url).await.unwrap_err();
	assert!(err.to_string().contains("parse"), "{err}");
}

#[tokio::test]
async fn test_fetch_auth_server_metadata_builds_rfc8414_url() {
	let body =
			r#"{"issuer":"https://auth.example.com","authorization_endpoint":"https://auth.example.com/authorize","token_endpoint":"https://auth.example.com/token"}"#
				.to_string();
	let url = spawn_http_stub(move |method, path, _host| {
		if method == "GET" && path == "/.well-known/oauth-authorization-server" {
			(200, vec![], body.clone())
		} else {
			(404, vec![], String::new())
		}
	})
	.await;

	// Trailing slash on the issuer is trimmed before building the well-known URL
	let metadata = fetch_auth_server_metadata(&format!("{url}/")).await.unwrap();
	assert_eq!(metadata.issuer, "https://auth.example.com");
	assert_eq!(metadata.token_endpoint, "https://auth.example.com/token");
	assert!(metadata.registration_endpoint.is_none());

	// Non-success status surfaces the RFC 8414 discovery URL
	let url = spawn_http_stub(move |_m, _p, _h| (500, vec![], String::new())).await;
	let err = fetch_auth_server_metadata(&url).await.unwrap_err();
	assert!(err.to_string().contains("RFC 8414"), "{err}");
}

#[tokio::test]
#[serial_test::serial]
async fn test_discover_well_known_metadata_with_cimd_caches_result() {
	let base = spawn_http_stub(move |method, path, host| {
			let prm = format!(
				"{{\"resource\":\"http://{host}/mcp\",\"authorization_servers\":[\"http://{host}\"],\"scopes_supported\":[\"mcp:read\"]}}"
			);
			let asm = format!(
				"{{\"issuer\":\"http://{host}\",\"authorization_endpoint\":\"http://{host}/oauth/authorize\",\"token_endpoint\":\"http://{host}/oauth/token\",\"scopes_supported\":[\"read\"],\"code_challenge_methods_supported\":[\"S256\"],\"client_id_metadata_document_supported\":true}}"
			);
			if method == "GET" && path.ends_with("/.well-known/oauth-protected-resource") {
				(200, vec![], prm)
			} else if method == "GET" && path == "/.well-known/oauth-authorization-server" {
				(200, vec![], asm)
			} else {
				(404, vec![], String::new())
			}
		})
		.await;
	let name = unique_server_name("cimd");
	clear_discovered_oauth_cache(&name);

	let config = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
		.await
		.unwrap();

	// client_id is Octomind's published CIMD document URL
	assert_eq!(config.client_id, super::super::cimd::CLIENT_METADATA_URL);
	assert_eq!(config.authorization_url, format!("{base}/oauth/authorize"));
	assert_eq!(config.token_url, format!("{base}/oauth/token"));
	// Resource scopes take precedence over auth server scopes
	assert_eq!(config.scopes, vec!["mcp:read".to_string()]);

	// Result is cached: a second discovery returns the identical config
	assert!(cached_discovery(&name).is_some());
	let again = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
		.await
		.unwrap();
	assert_eq!(again.client_id, config.client_id);

	clear_discovered_oauth_cache(&name);
	assert!(cached_discovery(&name).is_none());
}

#[tokio::test]
#[serial_test::serial]
async fn test_discover_401_flow_with_dcr() {
	let base = spawn_http_stub(move |method, path, host| {
			let prm = format!(
				"{{\"resource\":\"http://{host}/mcp\",\"authorization_servers\":[\"http://{host}\"]}}"
			);
			let asm = format!(
				"{{\"issuer\":\"http://{host}\",\"authorization_endpoint\":\"http://{host}/oauth/authorize\",\"token_endpoint\":\"http://{host}/oauth/token\",\"code_challenge_methods_supported\":[\"S256\"],\"registration_endpoint\":\"http://{host}/register\"}}"
			);
			if method == "POST" && path.starts_with("/mcp") {
				(
					401,
					vec![(
						"WWW-Authenticate".to_string(),
						format!("Bearer resource_metadata=\"http://{host}/prm\""),
					)],
					String::new(),
				)
			} else if method == "GET" && path == "/prm" {
				(200, vec![], prm)
			} else if method == "GET" && path == "/.well-known/oauth-authorization-server" {
				(200, vec![], asm)
			} else if method == "POST" && path == "/register" {
				(
					200,
					vec![],
					"{\"client_id\":\"dcr-client-123\",\"client_secret\":\"dcr-secret\"}".to_string(),
				)
			} else {
				(404, vec![], String::new())
			}
		})
		.await;
	let name = unique_server_name("dcr");
	clear_discovered_oauth_cache(&name);

	let config = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
		.await
		.unwrap();

	// client_id/secret came from the DCR registration response
	assert_eq!(config.client_id, "dcr-client-123");
	assert_eq!(config.client_secret, "dcr-secret");
	assert_eq!(config.token_url, format!("{base}/oauth/token"));
	assert!(cached_discovery(&name).is_some());

	clear_discovered_oauth_cache(&name);
}

#[tokio::test]
#[serial_test::serial]
async fn test_discover_errors_without_401() {
	// No 401 and no Protected Resource Metadata — not an RFC 9728 server
	let base = spawn_http_stub(move |_m, _p, _h| (200, vec![], String::new())).await;
	let name = unique_server_name("no401");
	clear_discovered_oauth_cache(&name);

	let err = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("401"), "{err}");
	assert!(cached_discovery(&name).is_none(), "failures must not be cached");
}

#[tokio::test]
#[serial_test::serial]
async fn test_discover_errors_on_401_without_header() {
	let base = spawn_http_stub(move |method, path, _host| {
		if method == "POST" && path.starts_with("/mcp") {
			(401, vec![], String::new())
		} else {
			(404, vec![], String::new())
		}
	})
	.await;
	let name = unique_server_name("nohdr");
	clear_discovered_oauth_cache(&name);

	let err = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("WWW-Authenticate"), "{err}");
}

#[tokio::test]
#[serial_test::serial]
async fn test_discover_errors_on_empty_authorization_servers() {
	let base = spawn_http_stub(move |method, path, host| {
		if method == "GET" && path.ends_with("/.well-known/oauth-protected-resource") {
			(
				200,
				vec![],
				format!("{{\"resource\":\"http://{host}/mcp\",\"authorization_servers\":[]}}"),
			)
		} else {
			(404, vec![], String::new())
		}
	})
	.await;
	let name = unique_server_name("emptyas");
	clear_discovered_oauth_cache(&name);

	let err = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("no authorization servers"), "{err}");
}

#[tokio::test]
#[serial_test::serial]
async fn test_discover_errors_without_cimd_or_dcr() {
	let base = spawn_http_stub(move |method, path, host| {
			let prm = format!(
				"{{\"resource\":\"http://{host}/mcp\",\"authorization_servers\":[\"http://{host}\"]}}"
			);
			let asm = format!(
				"{{\"issuer\":\"http://{host}\",\"authorization_endpoint\":\"http://{host}/oauth/authorize\",\"token_endpoint\":\"http://{host}/oauth/token\",\"code_challenge_methods_supported\":[\"S256\"]}}"
			);
			if method == "GET" && path.ends_with("/.well-known/oauth-protected-resource") {
				(200, vec![], prm)
			} else if method == "GET" && path == "/.well-known/oauth-authorization-server" {
				(200, vec![], asm)
			} else {
				(404, vec![], String::new())
			}
		})
		.await;
	let name = unique_server_name("nocimd");
	clear_discovered_oauth_cache(&name);

	let err = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("client_id"), "{err}");
	assert!(cached_discovery(&name).is_none(), "failures must not be cached");
}

#[tokio::test]
#[serial_test::serial]
async fn test_discover_uses_challenge_scopes_and_openid_configuration() {
	let base = spawn_http_stub(move |method, path, host| {
			let prm = format!(
				"{{\"resource\":\"http://{host}/mcp\",\"authorization_servers\":[\"http://{host}/tenant\"],\"scopes_supported\":[\"mcp:read\"]}}"
			);
			// Served only through OpenID Connect Discovery with path insertion
			let oidc = format!(
				"{{\"issuer\":\"http://{host}/tenant\",\"authorization_endpoint\":\"http://{host}/authorize\",\"token_endpoint\":\"http://{host}/token\",\"code_challenge_methods_supported\":[\"S256\"],\"client_id_metadata_document_supported\":true}}"
			);
			if method == "POST" && path.starts_with("/mcp") {
				(
					401,
					vec![(
						"WWW-Authenticate".to_string(),
						format!("Bearer resource_metadata=\"http://{host}/prm\", scope=\"files:read\""),
					)],
					String::new(),
				)
			} else if method == "GET" && path == "/prm" {
				(200, vec![], prm)
			} else if method == "GET" && path == "/.well-known/openid-configuration/tenant" {
				(200, vec![], oidc)
			} else {
				(404, vec![], String::new())
			}
		})
		.await;
	let name = unique_server_name("oidc");
	clear_discovered_oauth_cache(&name);

	let config = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
		.await
		.unwrap();
	assert_eq!(config.scopes, vec!["files:read".to_string()]);
	assert_eq!(config.token_url, format!("{base}/token"));
	clear_discovered_oauth_cache(&name);
}

#[tokio::test]
#[serial_test::serial]
async fn test_discover_refuses_server_without_pkce() {
	let base = spawn_http_stub(move |method, path, host| {
			let prm = format!(
				"{{\"resource\":\"http://{host}/mcp\",\"authorization_servers\":[\"http://{host}\"]}}"
			);
			let asm = format!(
				"{{\"issuer\":\"http://{host}\",\"authorization_endpoint\":\"http://{host}/authorize\",\"token_endpoint\":\"http://{host}/token\",\"client_id_metadata_document_supported\":true}}"
			);
			if method == "GET" && path.ends_with("/.well-known/oauth-protected-resource") {
				(200, vec![], prm)
			} else if method == "GET" && path == "/.well-known/oauth-authorization-server" {
				(200, vec![], asm)
			} else {
				(404, vec![], String::new())
			}
		})
		.await;
	let name = unique_server_name("nopkce");
	clear_discovered_oauth_cache(&name);

	let err = discover_oauth_from_mcp_server(&format!("{base}/mcp/"), &name)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("PKCE S256"), "{err}");
	assert!(cached_discovery(&name).is_none(), "failures must not be cached");
}
