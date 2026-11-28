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
use serial_test::serial;

#[cfg(unix)]
async fn run_script(script: &str) -> anyhow::Result<String> {
	let (_cancel_tx, cancel_rx) = watch::channel(false);
	let output = run_acp_command(
		"sh",
		&["-c", script],
		"task",
		&std::env::temp_dir(),
		cancel_rx,
		None,
		false,
	)
	.await;
	// Keep the sender alive through the await above; dropping it early would
	// read as cancellation.
	output
}

#[cfg(unix)]
#[tokio::test]
async fn acp_session_new_eof_after_initialize_fails_the_run() {
	// Answers initialize (id 1) then exits before the new/session response (id 2).
	let err = run_script("echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\nexit 0")
		.await
		.expect_err("EOF before the session response must fail");
	assert!(
		format!("{err:#}").contains("Subprocess closed before response"),
		"got: {err:#}"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn acp_session_new_error_surfaces_the_agent_explanation() {
	// An agent that cannot start answers initialize, then refuses
	// session/new with its reason in `data` (see `octomind acp` startup).
	let err = run_script(concat!(
		"echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n",
		"echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"error\":{\"code\":-32603,\"message\":\"Internal error\",\"data\":\"Agent x:y cannot start: Missing required values A, B\"}}'\n",
		"cat >/dev/null"
	))
	.await
	.expect_err("a refused session must fail the run");
	assert_eq!(
		err.to_string(),
		"ACP error: Agent x:y cannot start: Missing required values A, B"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn acp_eof_before_prompt_response_skips_blank_and_garbage_lines() {
	// Handshake completes, then only blank/non-JSON noise precedes EOF — the
	// failure must be the no-partial-output variant.
	let err = run_script(concat!(
		"echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n",
		"echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"s\"}}'\n",
		"echo ''\n",
		"echo 'this is not json'\n",
		"exit 0",
	))
	.await
	.expect_err("EOF before the prompt response must fail");
	let message = format!("{err:#}");
	assert!(
		message.contains("ACP subprocess closed before the session/prompt response"),
		"got: {message}"
	);
	assert!(
		!message.contains("Partial output"),
		"no chunks were streamed: {message}"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn acp_prompt_error_without_output_omits_partial_trailer() {
	let err = run_script(concat!(
		"echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n",
		"echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"s\"}}'\n",
		"echo '{\"jsonrpc\":\"2.0\",\"id\":3,\"error\":{\"code\":-32603,\"message\":\"Internal error\",\"data\":\"boom\"}}'\n",
		"exit 0",
	))
	.await
	.expect_err("prompt error must fail the run");
	let message = format!("{err:#}");
	assert!(message.contains("ACP prompt failed: boom"), "got: {message}");
	assert!(
		!message.contains("Partial output"),
		"no chunks were streamed: {message}"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn acp_kills_child_that_survives_stdout_eof() {
	// Completes the prompt response, closes stdout, but keeps running — the
	// run must still finish (bounded wait + process-group kill).
	let started = std::time::Instant::now();
	let output = tokio::time::timeout(
		std::time::Duration::from_secs(20),
		run_script(concat!(
			"echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'\n",
			"echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"s\"}}'\n",
			"echo '{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"hello\"}}}}'\n",
			"echo '{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"stopReason\":\"end_turn\"}}'\n",
			"exec 1>&-\n",
			"sleep 300",
		)),
	)
	.await
	.expect("run must terminate despite the surviving child")
	.expect("run succeeds");
	assert_eq!(output, "hello");
	assert!(
		started.elapsed() >= std::time::Duration::from_secs(4),
		"the bounded post-EOF wait must elapse before the kill: {:?}",
		started.elapsed()
	);
}

// --- wait_for_response low-level branches -----------------------------------

#[tokio::test]
async fn wait_for_response_cancelled_before_read_is_cancellation() {
	use tokio::io::{AsyncWriteExt, BufReader};

	let (mut writer, reader) = tokio::io::duplex(64);
	// Leave the stream open but never write: only the pre-set cancellation
	// can terminate the wait.
	let (_keep_tx, mut cancel_rx) = watch::channel(false);
	_keep_tx.send(true).expect("pre-set cancellation");
	let mut lines = BufReader::new(reader).lines();

	let err = wait_for_response(&mut lines, 1, &mut cancel_rx, std::time::Duration::from_secs(5))
		.await
		.expect_err("pre-set cancellation must fail the wait");
	assert!(crate::session::cancellation::is_cancelled(&err), "got: {err:#}");
	let _ = writer.shutdown().await;
}

#[tokio::test]
async fn wait_for_response_ignores_spurious_false_notifications() {
	use tokio::io::{AsyncWriteExt, BufReader};

	let (mut writer, reader) = tokio::io::duplex(64);
	let (cancel_tx, mut cancel_rx) = watch::channel(false);
	let mut lines = BufReader::new(reader).lines();

	// A `false` notification must not abort the wait…
	cancel_tx.send(false).expect("spurious notification");
	// …give the wait a moment to observe it, then answer normally.
	tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	writer
		.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n")
		.await
		.expect("write response");

	let response = wait_for_response(&mut lines, 7, &mut cancel_rx, std::time::Duration::from_secs(5))
		.await
		.expect("a false notification must not cancel the wait");
	assert_eq!(response.pointer("/result/ok").and_then(|v| v.as_bool()), Some(true));
	let _ = writer.shutdown().await;
}

// --- record_tap_live ignore branches ----------------------------------------

#[tokio::test]
#[serial]
async fn record_tap_live_ignores_messages_without_tool_or_usage_updates() {
	let sid = "__agenttest_taplive".to_string();
	crate::session::context::with_session_id(sid, async {
		crate::session::tap_runs::init_for_session();
		let (cancel_tx, _cancel_rx) = watch::channel(false);
		crate::session::tap_runs::register_job(crate::session::tap_runs::TapJob {
			mode: Default::default(),
			id: "probe-run".to_string(),
			role: "probe:agent".to_string(),
			workdir: ".".to_string(),
			started_at: std::time::SystemTime::now(),
			status: std::sync::Arc::new(std::sync::RwLock::new(crate::session::tap_runs::TapJobStatus::Running)),
			error: None,
			cancel_tx,
			live: std::sync::Arc::new(std::sync::RwLock::new(Default::default())),
		});

		// A tool_call update sets the live action…
		record_tap_live(
			"probe-run",
			&serde_json::json!({
				"params": {"update": {"sessionUpdate": "tool_call",
					"toolCallId": "t1", "title": "running probe"}}
			}),
		);
		let recorded = crate::session::tap_runs::find_job("probe-run")
			.expect("job registered")
			.live
			.last_action
			.clone();
		assert_eq!(recorded.as_deref(), Some("running probe"));

		// …messages without a recognized update kind must not touch it.
		record_tap_live(
			"probe-run",
			&serde_json::json!({
				"params": {"update": {"sessionUpdate": "plan",
					"plan": "some plan text"}}
			}),
		);
		record_tap_live("probe-run", &serde_json::json!({"params": {"_meta": {}}}));
		let after = crate::session::tap_runs::find_job("probe-run")
			.expect("job still registered")
			.live
			.last_action
			.clone();
		assert_eq!(
			after, recorded,
			"unrecognized updates must leave the live action untouched"
		);
	})
	.await;
}
