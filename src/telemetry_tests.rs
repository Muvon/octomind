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
fn builtin_tools_pass_through_and_mcp_tools_collapse_to_a_category() {
	assert_eq!(bucket_tool("shell"), "shell");
	assert_eq!(bucket_tool("text_editor"), "text_editor");
	// The whole point: a user-named MCP tool must not survive as itself.
	assert_eq!(bucket_tool("acme_internal_billing"), "ext:external");
	assert_eq!(bucket_tool("github_create_pr"), "ext:github");
}

#[test]
fn provider_failures_bucket_by_condition_not_by_wording() {
	let kind = |m: &str| api_error_kind(&anyhow::anyhow!("{m}"));
	// Providers word the same condition differently; the bucket must not care.
	assert_eq!(kind("API error 429 <unknown status code>"), "rate_limit");
	assert_eq!(kind("Rate limit exceeded for gpt-5"), "rate_limit");
	assert_eq!(kind("Overloaded: upstream capacity"), "overloaded");
	assert_eq!(
		kind("maximum context length is 200000 tokens"),
		"context_length"
	);
	assert_eq!(kind("API error 401 unauthorized"), "auth");
	assert_eq!(kind("API error 503 <unknown status code>"), "server");
	// Unrecognised text falls through to the transport classification, and in
	// no case does any of the message itself become the bucket.
	assert_eq!(kind("something we have never seen"), "other");
}

#[test]
fn model_splits_into_provider_and_catalogue_id() {
	assert_eq!(
		split_model("openrouter:anthropic/claude-sonnet-4"),
		("openrouter".into(), "anthropic/claude-sonnet-4".into())
	);
	assert_eq!(split_model("gpt-5"), (String::new(), "gpt-5".into()));
}

#[test]
fn events_serialize_without_empty_fields() {
	let json = serde_json::to_string(&Event {
		name: "start",
		ts: 1,
		command: "run".into(),
		..Default::default()
	})
	.unwrap();
	assert_eq!(json, r#"{"name":"start","ts":1,"command":"run"}"#);
}

#[test]
#[serial_test::serial]
fn enabled_recorders_fold_counters_into_session_events() {
	ENABLED.store(true, Ordering::Relaxed);
	FIRST_RUN.store(true, Ordering::Relaxed);
	CANCELS.store(0, Ordering::Relaxed);
	*STATE.lock() = State::default();

	record_start("run", vec!["--format".into()]);
	record_tool("shell");
	record_tool("github_create_pr");
	record_tool_error("github_create_pr");
	record_command("/info");
	record_api_error(&anyhow::anyhow!("429 rate limit"));
	record_cancel();
	record_workflow(WorkflowEnd {
		name: "release",
		steps: 3,
		duration_ms: 500,
		cost_usd: 0.25,
		tokens_in: 100,
		tokens_out: 20,
		tool_calls: 2,
		graph: true,
	});

	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.session.info.model = "openai:gpt-test".into();
	session.session.info.role = "assistant".into();
	session.session.info.total_api_calls = 4;
	session.session.info.tool_calls = 2;
	session.session.info.input_tokens = 100;
	session.session.info.output_tokens = 25;
	session.session.info.cache_read_tokens = 50;
	session.session.info.reasoning_tokens = 5;
	session.session.info.total_cost = 0.5;
	record_session(SessionEnd {
		kind: "interactive",
		outcome: "ok",
		error_kind: "",
		resumed: true,
		sandbox: true,
		mcp_servers: 3,
		info: &session.session.info,
	});
	record_error("config", "parse");

	let state = STATE.lock();
	assert_eq!(state.events.len(), 4);
	let workflow = &state.events[1];
	assert_eq!(workflow.kind, "workflow_graph");
	assert_eq!(workflow.cost_micro, 250_000);
	let session = &state.events[2];
	assert_eq!(session.provider, "openai");
	assert_eq!(session.model, "gpt-test");
	assert_eq!(session.cancels, 1);
	assert_eq!(session.tools.get("shell"), Some(&1));
	assert_eq!(session.tools.get("ext:github"), Some(&1));
	assert_eq!(session.tool_errors.get("ext:github"), Some(&1));
	assert_eq!(session.commands.get("/info"), Some(&1));
	assert_eq!(session.api_errors.get("rate_limit"), Some(&1));
	drop(state);

	ENABLED.store(false, Ordering::Relaxed);
	*STATE.lock() = State::default();
}

#[test]
fn transport_errors_classify_by_downcast() {
	let io = anyhow::Error::new(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
	assert_eq!(error_kind(&io), "io");

	let parse = serde_json::from_str::<Vec<String>>("{").unwrap_err();
	assert_eq!(error_kind(&anyhow::Error::new(parse)), "parse");

	assert_eq!(error_kind(&anyhow::anyhow!("mystery")), "other");
}

#[test]
#[serial_test::serial]
fn install_source_detects_a_cargo_test_binary() {
	// The test harness runs from a target build dir (target/debug, or llvm-cov-target/debug under cargo llvm-cov) → "source"
	assert_eq!(install_source(), "source");
}

struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl EnvGuard {
	fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}
impl Drop for EnvGuard {
	fn drop(&mut self) {
		for (key, saved) in &self.0 {
			match saved {
				Some(v) => std::env::set_var(key, v),
				None => std::env::remove_var(key),
			}
		}
	}
}

fn template_config() -> crate::config::Config {
	toml::from_str(include_str!("../config-templates/default.toml"))
		.expect("parse default config template")
}

#[test]
#[serial_test::serial]
fn init_arms_with_identity_and_honours_every_opt_out() {
	let _env = EnvGuard::new(&["OCTOMIND_DATA_DIR", DNT_ENV, TELEMETRY_ENV]);
	let tmp = tempfile::tempdir().unwrap();
	std::env::set_var("OCTOMIND_DATA_DIR", tmp.path());
	std::env::remove_var(DNT_ENV);
	std::env::remove_var(TELEMETRY_ENV);

	let config = template_config();
	assert!(config.telemetry, "template must default telemetry on");

	// Fresh data dir → armed + flagged as first run
	ENABLED.store(false, Ordering::Relaxed);
	init(&config);
	assert!(enabled());
	assert!(FIRST_RUN.load(Ordering::Relaxed));

	// Machine id now exists → subsequent init is not a first run
	ENABLED.store(false, Ordering::Relaxed);
	init(&config);
	assert!(enabled());
	assert!(!FIRST_RUN.load(Ordering::Relaxed));

	// Config opt-out
	let mut off = config.clone();
	off.telemetry = false;
	ENABLED.store(false, Ordering::Relaxed);
	init(&off);
	assert!(!enabled());

	// DO_NOT_TRACK wins over an opted-in config
	std::env::set_var(DNT_ENV, "1");
	ENABLED.store(false, Ordering::Relaxed);
	init(&config);
	assert!(!enabled());
	std::env::remove_var(DNT_ENV);

	// OCTOMIND_TELEMETRY=0 wins too; a truthy value defers to the config
	std::env::set_var(TELEMETRY_ENV, "0");
	ENABLED.store(false, Ordering::Relaxed);
	init(&config);
	assert!(!enabled());
	std::env::set_var(TELEMETRY_ENV, "1");
	ENABLED.store(false, Ordering::Relaxed);
	init(&config);
	assert!(enabled());

	ENABLED.store(false, Ordering::Relaxed);
	*STATE.lock() = State::default();
}

#[tokio::test]
#[serial_test::serial]
async fn flush_ships_buffered_events_and_disarms() {
	let _env = EnvGuard::new(&[
		"OCTOMIND_DATA_DIR",
		DNT_ENV,
		TELEMETRY_ENV,
		crate::account::API_URL_ENV,
	]);
	let tmp = tempfile::tempdir().unwrap();
	std::env::set_var("OCTOMIND_DATA_DIR", tmp.path());
	std::env::remove_var(DNT_ENV);
	std::env::remove_var(TELEMETRY_ENV);

	// Capture stub for the control-plane endpoint
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind");
	let addr = listener.local_addr().expect("addr");
	let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
	let sink = captured.clone();
	tokio::spawn(async move {
		use tokio::io::{AsyncReadExt, AsyncWriteExt};
		let (mut sock, _) = listener.accept().await.expect("accept");
		let mut buf = Vec::new();
		let mut chunk = [0u8; 8192];
		loop {
			let n = sock.read(&mut chunk).await.unwrap_or(0);
			if n == 0 {
				break;
			}
			buf.extend_from_slice(&chunk[..n]);
			if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
				let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
				let cl: usize = head
					.lines()
					.find_map(|l| l.strip_prefix("content-length:"))
					.and_then(|v| v.trim().parse().ok())
					.unwrap_or(0);
				if buf.len() >= pos + 4 + cl {
					break;
				}
			}
		}
		*sink.lock().unwrap() = buf;
		let resp = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok";
		let _ = sock.write_all(resp.as_bytes()).await;
	});
	std::env::set_var(crate::account::API_URL_ENV, format!("http://{addr}"));

	let config = template_config();
	init(&config);
	record_start("run", vec!["--format".into()]);

	flush().await;
	assert!(!enabled(), "flush must disarm telemetry");

	let raw = captured.lock().unwrap().clone();
	let text = String::from_utf8_lossy(&raw).to_string();
	assert!(text.contains("POST /api/v1/telemetry"), "request: {text}");
	assert!(text.contains("\"command\":\"run\""), "body: {text}");
	assert!(text.contains("\"v\":1"), "body: {text}");

	// A second flush with an empty buffer is a no-op
	flush().await;

	ENABLED.store(false, Ordering::Relaxed);
	*STATE.lock() = State::default();
}
