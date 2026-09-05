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
use std::time::{Duration, Instant};

/// The tap/agent client speaks raw JSON while the octomind ACP server
/// deserializes with the `agent-client-protocol` schema — pin every
/// outgoing params shape against those types so a crate upgrade that
/// changes the wire format fails here instead of at runtime (e.g. the
/// protocolVersion string → u16 break).
#[test]
fn outgoing_params_match_acp_schema() {
	use agent_client_protocol::schema::v1::{InitializeRequest, NewSessionRequest, PromptRequest};

	serde_json::from_value::<InitializeRequest>(acp_initialize_params())
		.expect("initialize params must match ACP v1 schema");
	serde_json::from_value::<NewSessionRequest>(acp_new_session_params(std::path::Path::new(
		"/tmp",
	)))
	.expect("session/new params must match ACP v1 schema");
	serde_json::from_value::<PromptRequest>(acp_prompt_params("sess", "task"))
		.expect("session/prompt params must match ACP v1 schema");
}

/// initialize (id=1) + session/new (id=2) + one streamed message chunk.
const HANDSHAKE: &str = r#"
echo '{"jsonrpc":"2.0","id":1,"result":{}}'
echo '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s"}}'
echo '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}'
"#;

/// Keeps the fake server alive until the client closes stdin. A script
/// that exits the instant its echos finish races the client's pipelined
/// writes: once the process is gone the stdin pipe has no reader, so
/// write_all fails with EPIPE (seen on macOS) before the client reads
/// the id=3 response.
const WAIT_STDIN_EOF: &str = "\ncat >/dev/null";

async fn run_fake_server(script: String, cancel_rx: watch::Receiver<bool>) -> Result<String> {
	run_acp_command(
		"sh",
		&["-c", &script],
		"task",
		&std::env::temp_dir(),
		cancel_rx,
		None,
		false,
	)
	.await
}

#[tokio::test]
async fn authorizer_passes_context_without_vetoing_an_unacknowledged_peer() {
	let sid = "authorizer-acp-parent".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let (tx, mut notifications) = tokio::sync::mpsc::unbounded_channel();
		crate::mcp::process::set_notification_sender(Some(sid.clone()), tx);
		let mut config = crate::session::chat::test_support::fake_provider_config();
		config.supervisor.enabled = true;
		config.supervisor.authorizer.enabled = true;
		let mut session = crate::session::chat::session::ChatSession::for_tests(vec![crate::session::Message {role:"user".into(),content:"Inspect only; never edit files".into(),id:Some("root-user".into()),..Default::default()}]);
		session.session.info.name = sid.clone();
		crate::supervisor::authorizer::capture(&mut session, &config);
		let params = acp_new_session_params(std::path::Path::new("/tmp"));
		assert_eq!(params["_meta"][crate::supervisor::authorizer::META_KEY]["users"][0]["text"], "Inspect only; never edit files");
		let (_tx,rx) = watch::channel(false);
		let result = run_fake_server(format!("{HANDSHAKE}\necho '{{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{{\"stopReason\":\"end_turn\"}}}}'\n{WAIT_STDIN_EOF}"), rx).await.unwrap();
		assert!(result.contains("hello"));
		let acknowledged = HANDSHAKE.replace("\"sessionId\":\"s\"", "\"sessionId\":\"s\",\"_meta\":{\"octomind.authorization\":true}");
		let (_tx,rx) = watch::channel(false);
		let result = run_fake_server(format!("{acknowledged}\necho '{{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{{\"stopReason\":\"end_turn\"}}}}'\n{WAIT_STDIN_EOF}"), rx).await.unwrap();
		assert!(result.contains("hello"));
		assert!(matches!(notifications.try_recv(), Ok(crate::websocket::ServerMessage::Assistant(_))));
		assert!(matches!(notifications.try_recv(), Ok(crate::websocket::ServerMessage::Assistant(_))));
		crate::mcp::process::clear_notification_sender(Some(sid.clone()));
		crate::session::context::cleanup_session(&sid);
	}).await;
}

#[tokio::test]
async fn streams_live_updates_into_tap_registry() {
	use crate::session::tap_runs::{self, TapJob, TapJobStatus, TapLiveState};
	use std::sync::{Arc, RwLock};
	use std::time::SystemTime;

	crate::session::context::with_session_id("tap-live-test-session".to_string(), async {
			let (cancel_tx, _keep_alive) = watch::channel(false);
			tap_runs::register_job(TapJob {
				id: "tap-test-live-000001".to_string(),
				role: "test:live".to_string(),
				workdir: ".".to_string(),
				started_at: SystemTime::now(),
				status: Arc::new(RwLock::new(TapJobStatus::Running)),
				cancel_tx,
				live: Arc::new(RwLock::new(TapLiveState::default())),
			});

			let script = format!(
				"{HANDSHAKE}{}\n{}\n{}{WAIT_STDIN_EOF}",
				r#"echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"shell","rawInput":{"command":"ls -la"}}}}'"#,
				r#"echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"session_info_update"},"_meta":{"octomind.usage":{"session_tokens":100,"session_cost":0.5,"input_tokens":80,"output_tokens":20,"cache_read_tokens":7,"cache_write_tokens":0,"reasoning_tokens":0}}}}'"#,
				r#"echo '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'"#
			);
			// Keep the run's cancel sender alive across both runs: a dropped
			// sender reads as cancellation and aborts at the id=1 handshake.
			let (cancel_tx, cancel_rx) = watch::channel(false);
			run_acp_command(
				"sh",
				&["-c", &script],
				"task",
				&std::env::temp_dir(),
				cancel_rx,
				Some("tap-test-live-000001"),
				false,
			)
			.await
			.expect("run succeeds");

			let job = tap_runs::find_job("tap-test-live-000001").expect("job registered");
			assert_eq!(job.live.last_action.as_deref(), Some("shell ls -la"));
			let usage = job.live.usage.expect("usage recorded from _meta");
			assert_eq!(usage.input_tokens, 80);
			assert_eq!(usage.output_tokens, 20);
			assert_eq!(usage.cache_read_tokens, 7);
			assert!((usage.cost - 0.5).abs() < 1e-9);

			// Resuming the same tap run replays the child's cumulative total, so
			// only the increment may be banked: 0.5 then 0.8 owes 0.8, not 1.3.
			let script = format!(
				"{HANDSHAKE}{}\n{}{WAIT_STDIN_EOF}",
				r#"echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"session_info_update"},"_meta":{"octomind.usage":{"session_tokens":160,"session_cost":0.8,"input_tokens":120,"output_tokens":40,"cache_read_tokens":7,"cache_write_tokens":0,"reasoning_tokens":0}}}}'"#,
				r#"echo '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'"#
			);
			run_acp_command(
				"sh",
				&["-c", &script],
				"task",
				&std::env::temp_dir(),
				cancel_tx.subscribe(),
				Some("tap-test-live-000001"),
				false,
			)
			.await
			.expect("resume succeeds");

			let banked = crate::session::external_spend::take();
			assert!(
				(banked - 0.8).abs() < 1e-9,
				"expected 0.8 banked for the parent, got {banked}"
			);
		})
		.await;
}

#[tokio::test]
async fn collects_output_and_returns_on_clean_exit() {
	let script = format!(
		"{HANDSHAKE}echo '{}'{WAIT_STDIN_EOF}",
		r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#
	);
	let started = Instant::now();
	// A dropped cancel sender reads as cancellation — keep it alive for the run.
	let (cancel_tx, cancel_rx) = watch::channel(false);
	let out = run_fake_server(script, cancel_rx)
		.await
		.expect("clean run succeeds");
	assert_eq!(out, "hello");
	// Child exited on its own — the kill grace period must not be consumed.
	assert!(started.elapsed() < Duration::from_secs(4));
	drop(cancel_tx);
}

#[tokio::test]
async fn collects_background_turn_after_initial_prompt_response() {
	let script = format!(
		"{HANDSHAKE}echo '{}'\ncat >/dev/null\necho '{}'\necho '{}'",
		r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","_meta":{"octomind.pending_work":true}}}"#,
		r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"job finished"}}}}"#,
		r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"final result"}}}}"#,
	);
	// A dropped cancel sender reads as cancellation — keep it alive for the run.
	let (cancel_tx, cancel_rx) = watch::channel(false);
	let out = run_fake_server(script, cancel_rx)
		.await
		.expect("background turn is collected before child EOF");
	assert_eq!(out, "hello\n\nfinal result");
	drop(cancel_tx);
}

#[tokio::test]
async fn kills_child_that_does_not_exit_after_response() {
	// `exec` keeps the same PID so the kill hits the sleeping process.
	let script = format!(
		"{HANDSHAKE}echo '{}'\nexec sleep 1000",
		r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#
	);
	let started = Instant::now();
	// A dropped cancel sender reads as cancellation — keep it alive for the run.
	let (cancel_tx, cancel_rx) = watch::channel(false);
	let out = run_fake_server(script, cancel_rx)
		.await
		.expect("wedged child still yields the response");
	assert_eq!(out, "hello");
	let elapsed = started.elapsed();
	// Returned via the grace-wait + kill path, not by hanging on wait().
	assert!(
		elapsed >= Duration::from_secs(5),
		"kill fired too early: {elapsed:?}"
	);
	assert!(elapsed < Duration::from_secs(15), "run hung: {elapsed:?}");
	drop(cancel_tx);
}

#[tokio::test]
async fn surfaces_prompt_error_instead_of_empty_output() {
	let script = format!(
		"{HANDSHAKE}echo '{}'{WAIT_STDIN_EOF}",
		r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32000,"message":"boom"}}"#
	);
	// A dropped cancel sender reads as cancellation — keep it alive for the run.
	let (cancel_tx, cancel_rx) = watch::channel(false);
	let err = run_fake_server(script, cancel_rx)
		.await
		.expect_err("prompt error must fail the run");
	assert!(err.to_string().contains("boom"), "got: {err:#}");
	drop(cancel_tx);
}

/// Real octomind failures arrive as `-32603` with the fixed JSON-RPC
/// message and the cause in `data` — reporting only `message` would tell
/// the parent nothing but "Internal error".
#[tokio::test]
async fn surfaces_error_data_over_generic_message() {
	let script = format!(
		"{HANDSHAKE}echo '{}'{WAIT_STDIN_EOF}",
		r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32603,"message":"Internal error","data":"API call failed: 429 rate limit"}}"#
	);
	// A dropped cancel sender reads as cancellation — keep it alive for the run.
	let (cancel_tx, cancel_rx) = watch::channel(false);
	let err = run_fake_server(script, cancel_rx)
		.await
		.expect_err("prompt error must fail the run");
	assert!(err.to_string().contains("429 rate limit"), "got: {err:#}");
	drop(cancel_tx);
}

#[tokio::test]
async fn cancellation_kills_child_mid_prompt() {
	// Server never answers the prompt (no id=3) and never exits.
	let script = format!("{HANDSHAKE}exec sleep 1000");
	let (cancel_tx, cancel_rx) = watch::channel(false);
	tokio::spawn(async move {
		tokio::time::sleep(Duration::from_millis(200)).await;
		let _ = cancel_tx.send(true);
	});
	let started = Instant::now();
	let err = run_fake_server(script, cancel_rx)
		.await
		.expect_err("cancellation must fail the run");
	assert!(
		crate::session::cancellation::is_cancelled(&err),
		"got: {err:#}"
	);
	// Cancel must act immediately — not wait out any grace period.
	assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn cancellation_kills_child_during_handshake() {
	let (cancel_tx, cancel_rx) = watch::channel(false);
	tokio::spawn(async move {
		tokio::time::sleep(Duration::from_millis(200)).await;
		let _ = cancel_tx.send(true);
	});
	let started = Instant::now();
	let err = run_fake_server("exec sleep 1000".to_string(), cancel_rx)
		.await
		.expect_err("handshake cancellation must fail the run");
	assert!(crate::session::cancellation::is_cancelled(&err));
	assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn eof_before_prompt_response_is_failure() {
	let script = format!("{HANDSHAKE}IFS= read -r _\nIFS= read -r _\nIFS= read -r _\nexit 0");
	// A dropped cancel sender reads as cancellation — keep it alive for the run.
	let (cancel_tx, cancel_rx) = watch::channel(false);
	let err = run_fake_server(script, cancel_rx)
		.await
		.expect_err("missing id=3 response must fail the run");
	assert!(
		err.to_string()
			.contains("closed before the session/prompt response"),
		"got: {err:#}"
	);
	drop(cancel_tx);
}

#[tokio::test]
async fn handshake_error_response_fails_the_run() {
	let script = format!(
		"echo '{}'\ncat >/dev/null",
		r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"handshake refused"}}"#
	);
	// A dropped cancel sender reads as cancellation — keep it alive for the run.
	let (cancel_tx, cancel_rx) = watch::channel(false);
	let err = run_fake_server(script, cancel_rx)
		.await
		.expect_err("initialize error must fail the run");
	assert!(err.to_string().contains("ACP error"), "got: {err:#}");
	assert!(
		err.to_string().contains("handshake refused"),
		"got: {err:#}"
	);
	drop(cancel_tx);
}

#[tokio::test]
async fn session_new_without_session_id_fails_the_run() {
	let script = concat!(
		"echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n",
		"echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}'\n",
		"cat >/dev/null"
	)
	.to_string();
	// Keep the cancel sender alive for the whole run: the product treats a
	// dropped sender as cancellation (nobody can resume the request), which
	// would abort at the id=1 handshake before session/new is ever read.
	let (cancel_tx, cancel_rx) = watch::channel(false);
	let err = run_fake_server(script, cancel_rx)
		.await
		.expect_err("missing sessionId must fail the run");
	drop(cancel_tx);
	assert!(
		err.to_string()
			.contains("No sessionId in session/new response"),
		"got: {err:#}"
	);
}

#[tokio::test]
async fn malformed_open_handshake_stream_times_out() {
	let (mut writer, reader) = tokio::io::duplex(128);
	writer
		.write_all(b"{malformed json}\n")
		.await
		.expect("write malformed response");
	let mut lines = BufReader::new(reader).lines();
	let (_cancel_tx, mut cancel_rx) = watch::channel(false);

	let error = wait_for_response(&mut lines, 1, &mut cancel_rx, Duration::from_millis(25))
		.await
		.expect_err("an open stream without a valid response must be bounded");
	assert!(
		error
			.to_string()
			.contains("Timed out waiting for ACP response id=1"),
		"got: {error:#}"
	);
}

#[tokio::test]
async fn non_json_and_blank_lines_are_skipped() {
	let script = format!(
		"echo 'not json'\necho ''\n{HANDSHAKE}echo 'garbage'\necho '{}'\ncat >/dev/null",
		r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#
	);
	// A dropped cancel sender reads as cancellation — keep it alive for the run.
	let (cancel_tx, cancel_rx) = watch::channel(false);
	let out = run_fake_server(script, cancel_rx)
		.await
		.expect("noise lines must not break the protocol");
	assert_eq!(out, "hello");
	drop(cancel_tx);
}

/// The handback guard reports exactly one verdict per run: the child's own
/// `_meta["octomind.verified"]` when present, unverified otherwise.
#[tokio::test]
async fn handback_banks_the_childs_verified_verdict() {
	let sid = "__agenttest_handback".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let script = format!(
			"{HANDSHAKE}echo '{}'\necho '{}'\ncat >/dev/null",
			r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"plan"},"_meta":{"octomind.verified":true}}}"#,
			r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#
		);
		// Keep the cancel sender alive across both runs: a dropped sender
		// reads as cancellation and aborts at the id=1 handshake.
		let (cancel_tx, cancel_rx) = watch::channel(false);
		run_acp_command(
			"sh",
			&["-c", &script],
			"task",
			&std::env::temp_dir(),
			cancel_rx,
			None,
			true,
		)
		.await
		.expect("verified run succeeds");
		assert_eq!(crate::supervisor::delegate::take_handback(), (1, 1));

		let script = format!(
			"{HANDSHAKE}echo '{}'\ncat >/dev/null",
			r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#
		);
		run_acp_command(
			"sh",
			&["-c", &script],
			"task",
			&std::env::temp_dir(),
			cancel_tx.subscribe(),
			None,
			true,
		)
		.await
		.expect("unverified run succeeds");
		assert_eq!(crate::supervisor::delegate::take_handback(), (1, 0));
		drop(cancel_tx);

		crate::supervisor::delegate::clear_handback_for_session(&sid);
	})
	.await;
}
