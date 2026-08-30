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
fn advertised_mcp3_capabilities_have_handlers() {
	let info = build_client_info(ProtocolVersion::V_2026_07_28);
	assert!(info.capabilities.supports_tasks());
	assert!(info.capabilities.sampling.is_none());
	let elicitation = info
		.capabilities
		.elicitation
		.expect("elicitation must be advertised");
	assert!(elicitation.form.is_some());
	assert!(elicitation.url.is_some());
	assert!(info.capabilities.roots.is_none());
}

#[test]
fn legacy_handshake_does_not_claim_mcp3_features() {
	let info = build_client_info(ProtocolVersion::V_2025_03_26);
	assert!(!info.capabilities.supports_tasks());
	assert!(info.capabilities.elicitation.is_none());
	assert!(info.capabilities.sampling.is_none());
	assert!(info.capabilities.roots.is_none());
}

#[test]
fn idle_timeout_guidance_is_actionable_and_side_effect_safe() {
	let message = idle_timeout_message("deploy", "operations", 30);
	assert!(message.contains("'deploy'"));
	assert!(message.contains("'operations'"));
	assert!(message.contains("PT30S idle"));
	assert!(message.contains("check for side effects"));
	// An idle timeout means no liveness at all, which a slow-but-healthy
	// command cannot produce — it reports progress while it waits. Steering
	// the model to detach instead is what produced launch-then-poll loops,
	// where each check costs a round-trip and re-sends the conversation.
	assert!(message.contains("hung or wedged"));
	assert!(!message.contains("background task"));
	assert!(!message.contains("timeout_seconds"));
}

#[test]
fn absolute_timeout_guidance_distinguishes_progress_from_completion() {
	let message = absolute_timeout_message("index", "search", 30, 600);
	assert!(message.contains("PT600S total"));
	assert!(message.contains("while reporting progress"));
	assert!(message.contains("idle PT30S"));
	assert!(message.contains("check for side effects"));
	assert!(!message.contains("timeout_seconds"));
	assert!(message.len() < 250);
}

#[test]
fn http_header_placeholders_resolve_from_environment() {
	const KEY: &str = "OCTOMIND_TEST_MCP_STATIC_HEADER";
	std::env::set_var(KEY, "secret-token");
	let mut server = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert(
			"Authorization".to_string(),
			format!("Bearer {{{{ENV:{KEY}}}}}"),
		);
	}

	assert!(missing_env_keys(&server).is_empty());
	let headers = resolve_http_headers(&server).expect("header resolution must succeed");
	assert_eq!(
		headers
			.get(reqwest::header::AUTHORIZATION)
			.expect("Authorization header must exist"),
		"Bearer secret-token"
	);
	std::env::remove_var(KEY);
}

#[test]
fn authorization_header_selects_static_auth_without_discovery() {
	let mut server = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert("aUtHoRiZaTiOn".to_string(), "Bearer static".to_string());
	}
	assert_eq!(http_auth_source(&server), HttpAuthSource::StaticHeader);

	let oauth_server = McpServerConfig::http("oauth", "https://example.com/mcp", 30, vec![]);
	assert_eq!(
		http_auth_source(&oauth_server),
		HttpAuthSource::OAuthDiscovery
	);
}

#[tokio::test]
async fn dropped_cancellation_sender_is_terminal() {
	let (sender, receiver) = tokio::sync::watch::channel(false);
	drop(sender);
	let mut token = Some(receiver);
	tokio::time::timeout(Duration::from_millis(100), wait_cancelled(&mut token))
		.await
		.expect("dropped sender must wake cancellation waiter");
}

#[serial_test::serial]
#[test]
fn progress_token_binding_registers_overwrites_and_releases() {
	let token = ProgressToken(rmcp::model::NumberOrString::Number(7));
	assert!(!PROGRESS_TOOL_IDS.read().unwrap().contains_key(&token));

	let binding = ProgressTokenBinding::new(&token, "call-1");
	assert_eq!(
		PROGRESS_TOOL_IDS.read().unwrap().get(&token).cloned(),
		Some("call-1".to_string())
	);

	// A rebind of the same token replaces the tool id.
	let rebound = ProgressTokenBinding::new(&token, "call-2");
	assert_eq!(
		PROGRESS_TOOL_IDS.read().unwrap().get(&token).cloned(),
		Some("call-2".to_string())
	);

	// Drop removes the mapping unconditionally — the binding assumes a
	// single owner per token for the life of one tool-call round.
	drop(rebound);
	assert!(!PROGRESS_TOOL_IDS.read().unwrap().contains_key(&token));
	drop(binding);
	assert!(!PROGRESS_TOOL_IDS.read().unwrap().contains_key(&token));
}

#[test]
fn client_handler_identifies_octomind_with_modern_protocol() {
	let handler = OctoClientHandler::new("test-server");
	let info = handler.get_info();
	assert_eq!(info.client_info.name, "octomind");
	assert!(!info.client_info.version.is_empty());
	assert_eq!(info.protocol_version, ProtocolVersion::V_2026_07_28);
	let experimental = info
		.capabilities
		.experimental
		.expect("session context must be advertised");
	assert!(experimental.contains_key("session"));
}

#[test]
fn client_handler_legacy_retry_advertises_legacy_protocol() {
	let handler = OctoClientHandler {
		legacy: true,
		..OctoClientHandler::new("legacy-server")
	};
	assert_eq!(
		handler.get_info().protocol_version,
		ProtocolVersion::V_2025_03_26
	);
}

#[test]
fn lifecycle_probes_modern_with_legacy_fallback_version() {
	match lifecycle() {
		ClientLifecycleMode::Auto {
			preferred_versions,
			legacy_version,
		} => {
			assert!(preferred_versions.contains(&ProtocolVersion::V_2026_07_28));
			assert_eq!(legacy_version, Some(ProtocolVersion::V_2025_03_26));
		}
		other => panic!("expected Auto lifecycle, got {other:?}"),
	}
}

#[test]
fn missing_env_keys_empty_without_placeholders() {
	let builtin = McpServerConfig::builtin("core", 30, vec![]);
	assert!(missing_env_keys(&builtin).is_empty());

	let mut http = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	if let McpServerConfig::Http { headers, .. } = &mut http {
		headers.insert("X-Static".to_string(), "plain-value".to_string());
	}
	assert!(missing_env_keys(&http).is_empty());
}

#[test]
fn missing_env_keys_collects_sorted_deduped_keys_from_all_fields() {
	let stdin = McpServerConfig::Stdin {
		name: "local".to_string(),
		command: "{{ENV:OCTOMIND_TEST_MISSING_CMD}}".to_string(),
		args: vec![
			"{{ENV:OCTOMIND_TEST_MISSING_ARG}}".to_string(),
			"{{ENV:OCTOMIND_TEST_MISSING_CMD}}".to_string(),
		],
		timeout_seconds: 30,
		tools: vec![],
		env: [(
			"CHILD_VAR".to_string(),
			"{{ENV:OCTOMIND_TEST_MISSING_ENV}}".to_string(),
		)]
		.into(),
		cwd: None,
		auto_bind: None,
	};
	assert_eq!(
		missing_env_keys(&stdin),
		vec![
			"OCTOMIND_TEST_MISSING_ARG",
			"OCTOMIND_TEST_MISSING_CMD",
			"OCTOMIND_TEST_MISSING_ENV",
		]
	);

	let mut http = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	if let McpServerConfig::Http { headers, .. } = &mut http {
		headers.insert(
			"Authorization".to_string(),
			"Bearer {{ENV:OCTOMIND_TEST_MISSING_HEADER}}".to_string(),
		);
	}
	assert_eq!(
		missing_env_keys(&http),
		vec!["OCTOMIND_TEST_MISSING_HEADER"]
	);
}

#[serial_test::serial]
#[test]
fn missing_env_keys_treats_empty_values_as_missing() {
	const KEY: &str = "OCTOMIND_TEST_EMPTY_ENV_KEY";
	std::env::set_var(KEY, "");
	let mut server = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert(
			"Authorization".to_string(),
			format!("Bearer {{{{ENV:{KEY}}}}}"),
		);
	}
	assert_eq!(missing_env_keys(&server), vec![KEY]);

	std::env::set_var(KEY, "token");
	assert!(missing_env_keys(&server).is_empty());
	std::env::remove_var(KEY);
}

#[serial_test::serial]
#[test]
fn resolve_env_placeholders_substitutes_only_resolved_values() {
	const SET: &str = "OCTOMIND_TEST_PLACEHOLDER_SET";
	const EMPTY: &str = "OCTOMIND_TEST_PLACEHOLDER_EMPTY";
	const UNSET: &str = "OCTOMIND_TEST_PLACEHOLDER_UNSET";
	std::env::set_var(SET, "resolved");
	std::env::set_var(EMPTY, "");

	let raw = format!("pre {{{{ENV:{SET}}}}} mid {{{{ENV:{EMPTY}}}}} post {{{{ENV:{UNSET}}}}}");
	assert_eq!(
		resolve_env_placeholders(&raw),
		format!("pre resolved mid {{{{ENV:{EMPTY}}}}} post {{{{ENV:{UNSET}}}}}")
	);

	std::env::remove_var(SET);
	std::env::remove_var(EMPTY);
}

#[test]
fn resolve_http_headers_passes_static_values_through() {
	let mut server = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert("X-Custom".to_string(), "static-value".to_string());
	}
	let resolved = resolve_http_headers(&server).expect("static headers must resolve");
	assert_eq!(
		resolved.get("x-custom").expect("header must exist"),
		"static-value"
	);
}

#[test]
fn resolve_http_headers_empty_for_servers_without_headers() {
	let stdin = McpServerConfig::Stdin {
		name: "local".to_string(),
		command: "echo".to_string(),
		args: vec![],
		timeout_seconds: 30,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	};
	assert!(resolve_http_headers(&stdin)
		.expect("must succeed")
		.is_empty());
}

#[test]
fn resolve_http_headers_rejects_invalid_header_name() {
	let mut server = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert("bad name".to_string(), "value".to_string());
	}
	let err = resolve_http_headers(&server).expect_err("invalid name must fail");
	assert!(err.to_string().contains("Invalid HTTP header name"));
}

#[test]
fn resolve_http_headers_rejects_invalid_header_value() {
	let mut server = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert("X-Bad".to_string(), "line\nbreak".to_string());
	}
	let err = resolve_http_headers(&server).expect_err("invalid value must fail");
	assert!(err.to_string().contains("Invalid value for HTTP header"));
}

#[test]
fn http_auth_source_defaults_to_oauth_discovery() {
	let no_headers = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	assert_eq!(
		http_auth_source(&no_headers),
		HttpAuthSource::OAuthDiscovery
	);

	let stdin = McpServerConfig::Stdin {
		name: "local".to_string(),
		command: "echo".to_string(),
		args: vec![],
		timeout_seconds: 30,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	};
	assert_eq!(http_auth_source(&stdin), HttpAuthSource::OAuthDiscovery);
}

#[test]
fn http_auth_source_supports_debug_and_equality() {
	assert_eq!(
		format!("{:?}", HttpAuthSource::StaticHeader),
		"StaticHeader"
	);
	assert_eq!(
		format!("{:?}", HttpAuthSource::OAuthDiscovery),
		"OAuthDiscovery"
	);
	assert_ne!(HttpAuthSource::StaticHeader, HttpAuthSource::OAuthDiscovery);
}

#[tokio::test]
async fn wait_cancelled_without_token_never_resolves() {
	let mut token = None;
	let outcome = tokio::time::timeout(Duration::from_millis(50), wait_cancelled(&mut token)).await;
	assert!(outcome.is_err(), "no owner and no token must stay pending");
}

#[tokio::test]
async fn wait_cancelled_returns_on_cancel_signal() {
	let (sender, receiver) = tokio::sync::watch::channel(false);
	let mut token = Some(receiver);
	sender.send(true).expect("send must succeed");
	tokio::time::timeout(Duration::from_millis(100), wait_cancelled(&mut token))
		.await
		.expect("cancel signal must wake the waiter");
}

#[tokio::test]
async fn sleep_or_cancel_completes_after_the_duration() {
	let mut token = None;
	sleep_or_cancel(Duration::from_millis(10), &mut token)
		.await
		.expect("uncancelled sleep must complete");
}

#[tokio::test]
async fn sleep_or_cancel_surfaces_cancellation_error() {
	let (sender, receiver) = tokio::sync::watch::channel(false);
	let mut token = Some(receiver);
	sender.send(true).expect("send must succeed");
	let err = sleep_or_cancel(Duration::from_secs(60), &mut token)
		.await
		.expect_err("cancelled sleep must fail");
	assert!(crate::session::cancellation::is_cancelled(&err));
}

/// `expect_err` for `Result<Arc<McpService>>` — the Ok type is not Debug,
/// and deriving Debug on the production handler is out of scope for tests.
fn expect_client_err(result: Result<Arc<McpService>>, msg: &str) -> String {
	match result {
		Ok(_) => panic!("{msg}"),
		Err(e) => e.to_string(),
	}
}

#[tokio::test]
async fn connect_stdio_refuses_unresolved_env_placeholders() {
	let server = McpServerConfig::Stdin {
		name: "guarded".to_string(),
		command: "{{ENV:OCTOMIND_TEST_NEVER_SET_CMD}}".to_string(),
		args: vec![],
		timeout_seconds: 30,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	};
	let msg = expect_client_err(connect_stdio(&server).await, "missing env must be refused");
	assert!(msg.contains("requires env vars"), "unexpected error: {msg}");
	assert!(msg.contains("OCTOMIND_TEST_NEVER_SET_CMD"));
}

#[tokio::test]
async fn connect_stdio_rejects_non_stdio_configs() {
	let server = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	let msg = expect_client_err(connect_stdio(&server).await, "http config must be rejected");
	assert!(msg.contains("requires a stdio server config"));
}

#[tokio::test]
async fn connect_http_rejects_non_http_configs() {
	let server = McpServerConfig::Stdin {
		name: "local".to_string(),
		command: "echo".to_string(),
		args: vec![],
		timeout_seconds: 30,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	};
	let msg = expect_client_err(connect_http(&server).await, "stdio config must be rejected");
	assert!(msg.contains("requires an http server config"));
}

#[tokio::test]
async fn connect_http_refuses_unresolved_env_placeholders() {
	let mut server = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert(
			"Authorization".to_string(),
			"Bearer {{ENV:OCTOMIND_TEST_NEVER_SET_HEADER}}".to_string(),
		);
	}
	let msg = expect_client_err(connect_http(&server).await, "missing env must be refused");
	assert!(msg.contains("requires env vars"));
}

#[tokio::test]
async fn get_or_connect_rejects_builtin_servers() {
	let server = McpServerConfig::builtin("core", 30, vec![]);
	let msg = expect_client_err(get_or_connect(&server).await, "builtin must be rejected");
	assert!(msg.contains("Builtin servers have no MCP client"));
}

#[tokio::test]
async fn http_auth_token_check_short_circuits_for_static_headers() {
	let mut server = McpServerConfig::http("remote", "https://example.com/mcp", 30, vec![]);
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert("Authorization".to_string(), "Bearer static".to_string());
	}
	// Static auth never consults the token store, so no network is touched.
	assert!(http_auth_token_still_current(&server).await);
}

#[serial_test::serial]
#[test]
fn registry_helpers_tolerate_empty_registry() {
	assert!(get("no-such-server").is_none());
	assert!(!is_connected("no-such-server"));
	assert!(connected_names().is_empty());
	disconnect("no-such-server"); // safe for unknown names
	disconnect_all(); // safe on an empty registry
}

#[test]
fn idle_timeout_message_formats_arbitrary_durations_and_names() {
	let zero = idle_timeout_message("t", "s", 0);
	assert!(zero.contains("PT0S idle"));
	let long = idle_timeout_message("search_index", "remote-mcp", 3600);
	assert!(long.contains("'search_index'"));
	assert!(long.contains("'remote-mcp'"));
	assert!(long.contains("PT3600S idle"));
}

#[test]
fn absolute_timeout_message_stays_bounded_for_any_parameters() {
	let message = absolute_timeout_message("build", "local", 45, 7200);
	assert!(message.contains("PT7200S total"));
	assert!(message.contains("idle PT45S"));
	assert!(message.len() < 250);
}
