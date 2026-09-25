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

//! End-to-end compression tests against the scripted fake provider: the real
//! pipeline runs — range selection, the XML decision/summary round trip,
//! apply (drain + summary/continuation insertion), and stats bookkeeping.
//! The ollama provider does not enforce response schemas, so the wire mode
//! is always XML here.
//! Archive writes also read `OCTOMIND_DATA_DIR`, so use the default serial
//! lock before `ENV_LOCK` to exclude tests that replace and delete that directory.

use super::*;
use crate::mcp::core::plan::sidecar_start;
use crate::session::chat::session::ChatSession;
use crate::session::chat::test_support::{
	fake_provider_config, final_response, spawn_stub, ENV_LOCK,
};

fn plan_directive(title: &str) -> crate::supervisor::plan::PlanTaskDirective {
	crate::supervisor::plan::PlanTaskDirective {
		title: title.to_string(),
		done_when: format!("{title} is verified"),
	}
}

fn msg(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		timestamp: crate::utils::time::now_secs(),
		..Default::default()
	}
}

/// A compressible in-memory session: system anchor + two full user/assistant
/// turns (force mode needs ≥3 conversation messages after the anchor).
fn compressible_session() -> ChatSession {
	let mut session = ChatSession::for_tests(vec![
		msg("system", "You are a helpful assistant."),
		msg("user", "build the frobnicator widget"),
		msg("assistant", "starting on the widget now"),
		msg("user", "make sure it compiles"),
		msg("assistant", "phase one is done and compiling"),
	]);
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();
	session
}

fn xml_summary_body() -> String {
	concat!(
		"<should_compress>true</should_compress>\n",
		"<original_request>build the frobnicator widget</original_request>\n",
		"<session_context>COMPRESS-E2E-CONTEXT: rust repo, widget work</session_context>\n",
		"<current_task>finish the frobnicator widget</current_task>\n",
		"<progress>phase one complete</progress>\n",
		"<analysis_findings><finding>widget lives in src/widget.rs</finding></analysis_findings>\n",
		"<errors_and_corrections><entry>fixed a compile error</entry></errors_and_corrections>\n",
		"<recent_exchanges><exchange>user asked for compilation, assistant confirmed</exchange></recent_exchanges>\n",
		"<key_entities><files><file>src/widget.rs</file></files>",
		"<names><name>Frobnicator</name></names>",
		"<decisions><decision>keep the widget minimal</decision></decisions></key_entities>\n",
		"<next_steps>wire the widget tests</next_steps>\n",
		"<critical_knowledge><knowledge>widget must stay allocation-free</knowledge></critical_knowledge>\n",
		"<open_loops><open_loop>widget rendering</open_loop></open_loops>\n",
		"<file_states><state>src/widget.rs modified</state></file_states>"
	)
	.to_string()
}

#[serial_test::serial]
#[tokio::test]
async fn test_done_compression_end_to_end() {
	let _guard = ENV_LOCK.lock().await;
	// Scripted twice: only one decision call is expected, but a second
	// identical body beats the queue-exhausted fallback if the flow ever
	// grows another call.
	let url = spawn_stub(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = compressible_session();
	let before = session.session.messages.len();

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Done)
			.await
			.expect("compression pipeline");
	assert!(compressed, "forced /done compression must compress");

	// The drained turns were replaced by summary/continuation plumbing that
	// carries the scripted narrative forward.
	let all_content: String = session
		.session
		.messages
		.iter()
		.map(|m| m.content.as_str())
		.collect::<Vec<_>>()
		.join("\n---\n");
	assert!(
		all_content.contains("COMPRESS-E2E-CONTEXT"),
		"summary narrative missing after compression (before={before}, after={}):\n{all_content}",
		session.session.messages.len()
	);

	// The decision call's spend was recorded on the compression component
	let stats = &session.session.info.compression_stats;
	assert!(stats.input_tokens > 0, "compression stats not recorded");

	std::env::remove_var("OLLAMA_API_URL");
}

#[serial_test::serial]
#[tokio::test]
async fn test_unparseable_summary_errors_and_keeps_messages() {
	let _guard = ENV_LOCK.lock().await;
	// A garbage decision response must surface an error and leave the
	// session untouched — never a partial drain.
	let url = spawn_stub(vec![final_response("not xml at all")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = compressible_session();
	let before = session.session.messages.clone();

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let result =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Done).await;
	assert!(result.is_err(), "unparseable summary must error, not drain");
	let after: Vec<String> = session
		.session
		.messages
		.iter()
		.map(|m| m.content.clone())
		.collect();
	let before: Vec<String> = before.iter().map(|m| m.content.clone()).collect();
	assert_eq!(before, after, "failed compression must not touch messages");

	std::env::remove_var("OLLAMA_API_URL");
}

#[serial_test::serial]
#[tokio::test]
async fn test_compression_cancelled_before_api_call() {
	let _guard = ENV_LOCK.lock().await;
	// Cancellation is checked before the API call — no stub needed, but the
	// env var must be parked somewhere harmless while we hold the lock.
	std::env::set_var("OLLAMA_API_URL", "http://127.0.0.1:1/unreachable");

	let config = fake_provider_config();
	let mut session = compressible_session();
	let (tx, rx) = tokio::sync::watch::channel(false);
	tx.send(true).expect("signal cancellation");

	let result =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Done).await;
	let err = result.expect_err("cancelled compression must error");
	assert!(
		err.downcast_ref::<crate::session::cancellation::Cancelled>()
			.is_some(),
		"expected Cancelled, got: {err}"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

#[serial_test::serial]
#[tokio::test]
async fn test_automatic_below_threshold_is_a_noop() {
	// Tiny session, automatic trigger: should_check_compression says no and
	// the pipeline returns false without any provider round trip.
	let config = fake_provider_config();
	let mut session = compressible_session();
	let before = session.session.messages.len();

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("no-op check");
	assert!(!compressed);
	assert_eq!(session.session.messages.len(), before);
}

// ===== TEMPORARY VERIFICATION TESTS (scratch — not part of the staged change) =====

#[serial_test::serial]
#[tokio::test]
async fn verify_midturn_e2e_mid_task_automatic_compression_keeps_user_request() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());
	// Force the context ceiling so the Automatic trigger fires deterministically
	// on a tiny session (current_tokens >= ceiling -> forced deepest compression).
	config.max_session_tokens_threshold = 1;

	// Mid-task tail: [assistant live step, tool result] — no user role at the tail.
	let mut session = ChatSession::for_tests(vec![
		msg("system", "You are a helpful assistant."),
		msg("user", "build the frobnicator widget"),
		msg("assistant", "starting on the widget now"),
		msg("user", "make sure it compiles"),
		msg("assistant", "phase one is done and compiling"),
		msg("user", "add tests too"),
		msg("assistant", "tests added"),
		msg("assistant", "running the build now"),
		msg("tool", "build output: ok"),
	]);
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(compressed, "mid-task automatic compression must compress");

	let wrapper = session
		.session
		.messages
		.iter()
		.find(|m| m.role == "user" && m.content.contains("<continuation>"))
		.expect("MID-TASK: a user-role continuation wrapper must be inserted after the summary");
	assert!(
		wrapper.content.contains("add tests too"),
		"wrapper must carry the active request verbatim, got:\n{}",
		wrapper.content
	);
	let joined = session
		.session
		.messages
		.iter()
		.map(|m| m.content.as_str())
		.collect::<Vec<_>>()
		.join("\n---\n");
	assert!(
		joined.contains("running the build now") && joined.contains("build output: ok"),
		"live exchange must survive byte-exact, got:\n{joined}"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

#[serial_test::serial]
#[tokio::test]
async fn verify_midturn_e2e_fresh_follow_up_keeps_exact_bridge_without_wrapper() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());
	config.max_session_tokens_threshold = 1;

	// Fresh-follow-up tail: [previous assistant answer, brand-new user request].
	let mut session = ChatSession::for_tests(vec![
		msg("system", "You are a helpful assistant."),
		msg("user", "build the frobnicator widget"),
		msg("assistant", "starting on the widget now"),
		msg("user", "make sure it compiles"),
		msg("assistant", "phase one is done and compiling"),
		msg("user", "add tests too"),
		msg("assistant", "the exact answer being followed up"),
		msg("user", "brand-new follow-up request"),
	]);
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(
		compressed,
		"fresh-follow-up automatic compression must compress"
	);

	let joined = session
		.session
		.messages
		.iter()
		.map(|m| m.content.as_str())
		.collect::<Vec<_>>()
		.join("\n---\n");
	assert!(
		joined.contains("brand-new follow-up request")
			&& joined.contains("the exact answer being followed up"),
		"exact [assistant, new request] bridge must survive verbatim, got:\n{joined}"
	);
	assert!(
		!session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("<continuation>")),
		"no continuation wrapper may be inserted when the tail already carries the real request"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// Replay of a failed workflow step: a turn timed out with its request
/// unanswered, and the retry re-sent the same request into the session. Over
/// the ceiling, force kept the step before the unanswered request as the fresh
/// request's bridge, found nothing else to drain, and the run died with
/// "forced compression has no eligible history (range 0..=0)" at 248k/200k.
#[serial_test::serial]
#[tokio::test]
async fn verify_forced_fold_drains_a_request_the_retry_sent_again() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());
	config.max_session_tokens_threshold = 1;

	let request = "Surgically edit every language in the brief (RESENT-REQUEST)";
	let mut prior_summary = msg(
		"assistant",
		"<conversation_summary id=\"c1\">\nearlier work on the brief\n</conversation_summary>",
	);
	prior_summary.name = Some(apply::COMPRESSION_MESSAGE_NAME.to_string());
	let mut session = ChatSession::for_tests(vec![
		msg("system", "You are a helpful assistant."),
		prior_summary,
		msg("assistant", "reading the Spanish draft"),
		msg("tool", "draft contents"),
		msg("user", request),
		msg(
			"user",
			"<pay-attention>\nRe-anchor on your task.\n</pay-attention>",
		),
		msg("user", request),
	]);
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("forced fold must drain the unanswered request");
	assert!(compressed, "the ceiling fold must land");

	let copies: usize = session
		.session
		.messages
		.iter()
		.map(|m| m.content.matches(request).count())
		.sum();
	assert_eq!(
		copies, 1,
		"only the fresh turn states the request — not an 'earlier request' too"
	);
	assert_eq!(
		session.session.messages.last().map(|m| m.content.as_str()),
		Some(request),
		"the fresh request stays the live turn"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// Regression guard for the server-side chaining leak. Providers that keep
/// conversation state server-side (OpenAI/xAI `previous_response_id`, OctoHub
/// `previous_completion_id`) chain off the id of the last assistant message and
/// then send only the delta. If any assistant id survives compaction, the next
/// request replays the full pre-compaction history from the server and the
/// compacted transcript is never what the model sees. Every assistant id must
/// therefore be gone after a fold — summary and retained live tail alike — so
/// the next request rebases onto the compacted transcript.
#[serial_test::serial]
#[tokio::test]
async fn verify_compaction_drops_provider_chain_ids() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());
	config.max_session_tokens_threshold = 1;

	// Same mid-task shape as the wrapper test above: the trailing
	// [assistant(tool_calls), tool] round is the live tail a fold retains.
	let mut session = ChatSession::for_tests(vec![
		msg("system", "You are a helpful assistant."),
		msg("user", "build the frobnicator widget"),
		msg("assistant", "starting on the widget now"),
		msg("user", "make sure it compiles"),
		msg("assistant", "phase one is done and compiling"),
		msg("user", "add tests too"),
		msg("assistant", "tests added"),
		crate::session::Message {
			tool_calls: Some(serde_json::json!([{
				"id": "call_1", "type": "function",
				"function": {"name": "shell", "arguments": "{\"command\":\"cargo build\"}"}
			}])),
			..msg("assistant", "running the build now")
		},
		crate::session::Message {
			tool_call_id: Some("call_1".to_string()),
			name: Some("shell".to_string()),
			..msg("tool", "build output: ok")
		},
	]);
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();
	for (n, message) in session
		.session
		.messages
		.iter_mut()
		.filter(|m| m.role == "assistant")
		.enumerate()
	{
		message.id = Some(format!("resp_{n:032x}"));
	}

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(compressed, "mid-task automatic compression must compress");

	let messages = &session.session.messages;
	assert!(
		messages
			.iter()
			.any(|m| m.content.contains("COMPRESS-E2E-CONTEXT")),
		"summary missing after compression"
	);
	let live = messages
		.iter()
		.find(|m| m.content == "running the build now")
		.expect("live tail assistant must survive the fold");
	assert!(
		live.id.is_none(),
		"retained live assistant kept its chain id"
	);
	let leaked: Vec<String> = messages
		.iter()
		.filter(|m| m.role == "assistant")
		.filter_map(|m| m.id.clone())
		.collect();
	assert!(
		leaked.is_empty(),
		"assistant ids survived compaction — the next request would chain the pre-compaction history: {leaked:?}"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// The two regimes behind the fire line, through the real pipeline. Same
/// mid-task session shape, threshold set just under the live context so the
/// line is crossed; the only difference is where in the turn the check runs.
async fn regime_session(config: &mut crate::config::Config) -> ChatSession {
	let mut session = ChatSession::for_tests(vec![
		msg("system", "You are a helpful assistant."),
		msg("user", "build the frobnicator widget"),
		msg("assistant", "starting on the widget now"),
		msg("user", "make sure it compiles"),
		msg("assistant", "phase one is done and compiling"),
		msg("user", "add tests too"),
		msg("assistant", "tests added"),
		msg("assistant", "running the build now"),
		msg("tool", "build output: ok"),
	]);
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();
	// No tool schemas: the live context is the messages alone, so the
	// threshold below sits in a known relation to it.
	session.cached_tools = Some(Vec::new());
	session.session.info.total_api_calls = 50;
	let live = session.get_full_context_tokens(config).await;
	config.compression.threshold = (live * 2 / 3).max(1);
	session
}

/// Drive the async fold to a settled state: a spawn returns false with a job
/// parked on the session; keep re-entering until it is collected (or nothing
/// is pending). Mirrors what the next tool-round boundary does in production.
async fn settle_folds(session: &mut ChatSession, config: &crate::config::Config) -> bool {
	for _ in 0..100 {
		let (_tx, rx) = tokio::sync::watch::channel(false);
		let compressed =
			check_and_compress_conversation(session, config, rx, CompressionTrigger::Automatic)
				.await
				.expect("compression pipeline");
		if compressed {
			return true;
		}
		if session.fold_job.is_none() {
			return false;
		}
		tokio::time::sleep(std::time::Duration::from_millis(20)).await;
	}
	panic!("background fold never settled");
}

#[serial_test::serial]
#[tokio::test]
async fn verify_turn_boundary_folds_on_crossing_the_line() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = regime_session(&mut config).await;
	// Between the user message and its first call: no history needed.
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;

	assert!(
		settle_folds(&mut session, &config).await,
		"a genuine turn boundary over the line must fold"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// The core non-blocking guarantee: the trigger call spawns and returns
/// without folding, and a stale summary is only ever discarded — the fold is
/// applied solely to the exact range it was computed from.
#[serial_test::serial]
#[tokio::test]
async fn verify_background_fold_discards_on_range_change() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = regime_session(&mut config).await;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(!compressed, "the trigger call must only spawn, not block");
	assert!(
		session.fold_job.is_some(),
		"a background fold must be parked"
	);
	let before = session.session.messages.len();

	// The drained range changes while the fold is in flight: the stale
	// summary must be discarded, never applied.
	session.session.messages[2].content = "starting on the widget now (amended)".to_string();
	while !session
		.fold_job
		.as_ref()
		.expect("job still parked")
		.handle
		.is_finished()
	{
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
	}
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(!compressed, "a stale fold must be discarded");
	assert_eq!(
		session.session.messages.len(),
		before,
		"discarded fold must leave the transcript untouched"
	);
	assert!(
		!session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("COMPRESS-E2E-CONTEXT")),
		"stale summary must not be spliced in"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

#[serial_test::serial]
#[tokio::test]
async fn verify_mid_turn_waits_until_the_pace_justifies_a_fold() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = regime_session(&mut config).await;
	// Two calls into a turn with no completed-turn history: two calls of
	// savings in evidence, below even the base runway — wait.
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls - 2;

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(
		!compressed && session.fold_job.is_none(),
		"mid-turn with a two-call horizon must not fold on size alone"
	);

	// Same context, but a session that has shown a long pace: fold.
	let mut session = regime_session(&mut config).await;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls - 2;
	session.session.info.turn_call_counts = vec![30, 30, 30];
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(
		!compressed && session.fold_job.is_some(),
		"a demonstrated long pace must schedule a background fold"
	);
	if let Some(job) = session.fold_job.take() {
		job.handle.abort();
		let _ = job.handle.await;
	}

	std::env::remove_var("OLLAMA_API_URL");
}

fn veto_summary_body() -> String {
	xml_summary_body().replace(
		"<should_compress>true</should_compress>",
		"<should_compress>false</should_compress>",
	)
}

async fn wait_until_finished(session: &ChatSession) {
	while !session
		.fold_job
		.as_ref()
		.expect("job still parked")
		.handle
		.is_finished()
	{
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
	}
}

/// Inside the ceiling margin nothing is detached and nothing is vetoable: the
/// fold runs inline on the trigger call and lands even when the decision
/// model declines (measured failure: a turn crawled for three hours, every
/// round blocking on a fresh vetoable background fold 17k under the ceiling).
#[serial_test::serial]
#[tokio::test]
async fn verify_ceiling_margin_folds_inline_and_overrides_the_veto() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&veto_summary_body()),
		final_response(&veto_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = regime_session(&mut config).await;
	// Mid-turn with a two-call horizon: below the margin this shape waits.
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls - 2;
	// The ceiling one token above the live context: inside the runway margin.
	let live = session.get_full_context_tokens(&config).await;
	config.max_session_tokens_threshold = live + 1;

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(
		compressed,
		"inside the ceiling margin the fold must land on the trigger call, veto or not"
	);
	assert!(
		session.fold_job.is_none(),
		"no background job inside the ceiling margin"
	);
	assert!(
		session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("COMPRESS-E2E-CONTEXT")),
		"the forced fold must be applied"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// A background fold that fails is not retried on the next round: unforced
/// attempts wait one runway of calls, then try again.
#[serial_test::serial]
#[tokio::test]
async fn verify_failed_background_fold_backs_off_for_a_runway() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response("not xml at all"),
		final_response("not xml at all"),
		final_response("not xml at all"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = regime_session(&mut config).await;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;
	let before = session.session.messages.len();

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(
		!compressed && session.fold_job.is_some(),
		"the trigger call spawns"
	);
	wait_until_finished(&session).await;

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(!compressed, "a garbage summary must not fold");
	assert_eq!(
		session.session.messages.len(),
		before,
		"transcript untouched"
	);
	assert!(
		session.fold_job.is_none(),
		"a failed attempt must not be re-spawned on the same round"
	);
	let calls = session.session.info.total_api_calls;
	assert!(
		session.fold_cooldown_until_call > calls,
		"a failure must start the cooldown"
	);

	// One call short of the cooldown, still a genuine boundary: held.
	session.session.info.total_api_calls = session.fold_cooldown_until_call - 1;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(
		!compressed && session.fold_job.is_none(),
		"no unforced attempt inside the cooldown"
	);

	// Cooldown over: the fold is attempted again.
	session.session.info.total_api_calls = session.fold_cooldown_until_call;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(
		!compressed && session.fold_job.is_some(),
		"after the cooldown a fresh attempt is spawned"
	);
	session.fold_job.take().expect("parked").handle.abort();

	std::env::remove_var("OLLAMA_API_URL");
}

/// Turn end applies a finished fold and never waits for a running one.
#[serial_test::serial]
#[tokio::test]
async fn verify_turn_end_settle_applies_only_a_finished_fold() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = regime_session(&mut config).await;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;
	let before = session.session.messages.len();

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(
		!compressed && session.fold_job.is_some(),
		"the trigger call spawns"
	);

	// A fold that never finishes: turn end leaves it parked and returns at once.
	let FoldJob {
		handle,
		ctx,
		cancel,
	} = session.fold_job.take().expect("parked");
	handle.abort();
	let never = tokio::spawn(async {
		std::future::pending::<
			Result<(
				schema::CompressionSummary,
				Option<crate::providers::TokenUsage>,
			)>,
		>()
		.await
	});
	session.fold_job = Some(FoldJob {
		handle: never,
		ctx,
		cancel,
	});
	let settled = tokio::time::timeout(
		std::time::Duration::from_secs(2),
		settle_pending_fold(&mut session, &config),
	)
	.await
	.expect("turn end must not block on a running fold")
	.expect("settle");
	assert!(!settled, "nothing to apply while the fold is running");
	assert!(
		session.fold_job.is_some(),
		"a running fold stays parked for the next round"
	);
	assert_eq!(
		session.session.messages.len(),
		before,
		"transcript untouched"
	);
	session.fold_job.take().expect("parked").handle.abort();

	// A finished fold is applied at turn end.
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(
		!compressed && session.fold_job.is_some(),
		"a fresh attempt spawns"
	);
	wait_until_finished(&session).await;
	assert!(
		settle_pending_fold(&mut session, &config)
			.await
			.expect("settle"),
		"a finished fold is applied at turn end"
	);
	assert!(session.fold_job.is_none(), "collected");
	assert!(
		session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("COMPRESS-E2E-CONTEXT")),
		"the settled summary must be spliced in"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// The fold request must not die with the operation that started it. The
/// piped loop and the interactive prompt swap the operation channel at every
/// boundary (dropping the old sender), and octolib reads a dropped sender as a
/// cancel — so a fold spanning the boundary used to come back as
/// "cancelled before compression could be applied". On the current-thread
/// test runtime the spawned task has not run yet when the sender is dropped,
/// so this is exactly the boundary race, made deterministic.
#[serial_test::serial]
#[tokio::test]
async fn verify_background_fold_survives_the_operation_sender_being_dropped() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![final_response(&xml_summary_body())]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = regime_session(&mut config).await;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;

	let (tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(!compressed && session.fold_job.is_some(), "spawned");
	// The turn ends: the operation channel is replaced and its sender dropped
	// before the fold task has sent its request.
	drop(tx);
	wait_until_finished(&session).await;
	assert!(
		settle_pending_fold(&mut session, &config)
			.await
			.expect("settle"),
		"the fold must land although the operation that started it is gone"
	);
	assert!(
		session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("COMPRESS-E2E-CONTEXT")),
		"summary spliced in"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// A one-shot run ending with a fold in flight waits for it — bounded by the
/// fold request's own budget — and applies it; a fold that never returns is
/// abandoned at the budget with the failure cooldown, never held past it.
#[serial_test::serial]
#[tokio::test]
async fn verify_exit_collects_a_running_fold_within_the_request_budget() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![final_response(&xml_summary_body())]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());
	config.compression.model.request_timeout_seconds = Some(1);
	config.compression.model.max_retries = Some(0);

	let mut session = regime_session(&mut config).await;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;
	let before = session.session.messages.len();

	// Nothing parked: a no-op.
	assert!(!collect_fold_before_exit(&mut session, &config)
		.await
		.expect("no fold"));

	// A fold that never finishes is abandoned once the request budget is spent.
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(!compressed && session.fold_job.is_some(), "spawned");
	let FoldJob {
		handle,
		ctx,
		cancel,
	} = session.fold_job.take().expect("parked");
	handle.abort();
	let never = tokio::spawn(async {
		std::future::pending::<
			Result<(
				schema::CompressionSummary,
				Option<crate::providers::TokenUsage>,
			)>,
		>()
		.await
	});
	session.fold_job = Some(FoldJob {
		handle: never,
		ctx,
		cancel,
	});
	let started = std::time::Instant::now();
	let applied = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		collect_fold_before_exit(&mut session, &config),
	)
	.await
	.expect("the exit wait is bounded by the request budget")
	.expect("collect");
	assert!(!applied, "a hung fold is abandoned");
	assert!(
		started.elapsed() >= std::time::Duration::from_secs(1),
		"the wait honours the request budget before giving up"
	);
	assert!(session.fold_job.is_none(), "abandoned, not re-parked");
	assert!(
		session.fold_cooldown_until_call > session.session.info.total_api_calls,
		"abandoning starts the failure cooldown"
	);
	assert_eq!(
		session.session.messages.len(),
		before,
		"transcript untouched"
	);

	// A fold that does finish is applied before exit.
	session.fold_cooldown_until_call = 0;
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(!compressed && session.fold_job.is_some(), "spawned again");
	assert!(
		collect_fold_before_exit(&mut session, &config)
			.await
			.expect("collect"),
		"a finishing fold is applied before exit"
	);
	assert!(session.fold_job.is_none(), "collected");
	assert!(
		session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("COMPRESS-E2E-CONTEXT")),
		"the summary is what gets persisted"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// A fold whose response was cut by the output budget before any content
/// (finish_reason=length — the model reasoned its budget away) is not re-sent
/// as is: the generic empty-completion retry stops on a length cut, and the
/// fold repairs itself once with reasoning off and twice the budget. With
/// `max_retries = 0` the old path fails after the first attempt; the repair is
/// the only way the second scripted response can be reached.
#[serial_test::serial]
#[tokio::test]
async fn verify_length_cut_fold_is_repaired_once_with_reasoning_off() {
	let _guard = ENV_LOCK.lock().await;
	let length_cut = serde_json::json!({
		"choices": [{
			"message": {"role": "assistant", "content": ""},
			"finish_reason": "length"
		}],
		"usage": {"prompt_tokens": 20, "completion_tokens": 10, "total_tokens": 30, "cost": 0.001}
	});
	let url = spawn_stub(vec![length_cut, final_response(&xml_summary_body())]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());
	config.compression.model.max_retries = Some(0);

	let mut session = regime_session(&mut config).await;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(!compressed && session.fold_job.is_some(), "spawned");
	wait_until_finished(&session).await;
	assert!(
		settle_pending_fold(&mut session, &config)
			.await
			.expect("settle"),
		"the repaired fold must land"
	);
	assert!(
		session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("COMPRESS-E2E-CONTEXT")),
		"summary spliced in"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// Two length cuts in a row exhaust the single repair: the fold fails with the
/// typed length-cut error and the cooldown starts, without a third request.
#[serial_test::serial]
#[tokio::test]
async fn verify_second_length_cut_fails_the_fold_without_identical_retries() {
	let _guard = ENV_LOCK.lock().await;
	let length_cut = || {
		serde_json::json!({
			"choices": [{
				"message": {"role": "assistant", "content": ""},
				"finish_reason": "length"
			}],
			"usage": {"prompt_tokens": 20, "completion_tokens": 10, "total_tokens": 30, "cost": 0.001}
		})
	};
	// Only two responses are scripted although max_retries=1 would allow four
	// requests on the old path: any identical retry would hit an empty stub.
	let url = spawn_stub(vec![length_cut(), length_cut()]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());
	config.compression.model.max_retries = Some(1);

	let mut session = regime_session(&mut config).await;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;
	let before = session.session.messages.len();

	let (_tx, rx) = tokio::sync::watch::channel(false);
	check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
		.await
		.expect("compression pipeline");
	wait_until_finished(&session).await;
	let FoldJob {
		handle,
		cancel: _cancel,
		..
	} = session.fold_job.take().expect("parked");
	let error = handle
		.await
		.expect("join")
		.expect_err("two length cuts fail the fold");
	let empty = error
		.downcast_ref::<crate::session::completion::EmptyCompletion>()
		.expect("typed empty-completion error");
	assert!(empty.is_length_cut());
	assert_eq!(empty.attempts, 1, "a length cut is never re-sent unchanged");
	assert_eq!(
		session.session.messages.len(),
		before,
		"transcript untouched"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

/// A corroborated deferral is the one case where the runtime declines to spend:
/// the claim is confirmed by a live plan step and an unblocked agent, so the
/// round is held without a paid fold call. The deferral stays recorded for the
/// next eligible round to re-judge against fresh state.
#[serial_test::serial]
#[tokio::test]
async fn verify_corroborated_deferral_holds_without_a_paid_call() {
	let _guard = ENV_LOCK.lock().await;
	// An empty script: any fold call would hit the exhausted-stub fallback and
	// fail to parse, so the hold has to be real, not a swallowed error.
	let url = spawn_stub(Vec::new()).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());
	// No configured safety limit and an unresolvable model: nothing caps the
	// session, so the ceiling margin cannot be what holds this round.
	config.max_session_tokens_threshold = 0;

	let sid = format!("compress-defer-e2e-{}", std::process::id());
	crate::session::context::with_session_id(sid.clone(), async {
		let mut session = regime_session(&mut config).await;
		session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;
		// The runtime-owned corroboration: a plan whose open step is the very
		// step the decline was about.
		sidecar_start(
			"Ship the widget",
			&[plan_directive("implement"), plan_directive("verify")],
		)
		.expect("plan starts");
		session.fold_deferral = Some(FoldDeferral {
			reason: DeferReason::MidDerivation,
			step: crate::mcp::core::plan::active_step_index(),
		});

		let (_tx, rx) = tokio::sync::watch::channel(false);
		let compressed = check_and_compress_conversation(
			&mut session,
			&config,
			rx,
			CompressionTrigger::Automatic,
		)
		.await
		.expect("compression pipeline");

		assert!(!compressed, "a corroborated deferral must not fold");
		assert!(
			session.fold_job.is_none(),
			"a corroborated deferral must not pay for a fold"
		);
		assert_eq!(
			session.fold_deferral,
			Some(FoldDeferral {
				reason: DeferReason::MidDerivation,
				step: Some(0),
			}),
			"a held deferral stays recorded for the next eligible round"
		);
		assert_eq!(
			session.session.info.compression_stats.input_tokens, 0,
			"a held round spends nothing"
		);
	})
	.await;
	crate::session::context::clear_plan_storage(&sid);

	std::env::remove_var("OLLAMA_API_URL");
}

/// The other half of the judgment: a veto the runtime cannot corroborate is
/// overruled, not obeyed. No reason means no claim, so the fold proceeds on the
/// normal background path with the veto withdrawn — not as an emergency fold —
/// and the spent deferral is consumed rather than re-litigated.
#[serial_test::serial]
#[tokio::test]
async fn verify_uncorroborated_deferral_is_overruled_and_the_fold_lands() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&veto_summary_body()),
		final_response(&veto_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = regime_session(&mut config).await;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;
	// A bare veto: the model declined without naming a reason. There is no
	// in-flight claim for the runtime to confirm, so the premise is refuted.
	session.fold_deferral = Some(FoldDeferral {
		reason: DeferReason::None,
		step: None,
	});

	let (_tx, rx) = tokio::sync::watch::channel(false);
	let compressed =
		check_and_compress_conversation(&mut session, &config, rx, CompressionTrigger::Automatic)
			.await
			.expect("compression pipeline");
	assert!(
		!compressed && session.fold_job.is_some(),
		"an overruled deferral is not an emergency: the fold still runs in the background"
	);
	assert_eq!(
		session.fold_deferral, None,
		"a refuted deferral is consumed, not re-litigated"
	);

	assert!(
		settle_folds(&mut session, &config).await,
		"an uncorroborated deferral must be overruled, not obeyed"
	);
	assert!(
		session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("COMPRESS-E2E-CONTEXT")),
		"the overruled veto must still produce the fold"
	);
	assert_eq!(session.fold_deferral, None);

	std::env::remove_var("OLLAMA_API_URL");
}

/// The original failure: the decision model declined every fold and the session
/// sat unfolded until the ceiling. Without a plan step there is nothing for the
/// runtime to check a claim against, so the veto is not offered at all — the
/// model's `false` is overridden on the first call, not paid for twice.
#[serial_test::serial]
#[tokio::test]
async fn verify_veto_is_not_offered_without_a_plan_step_to_check_it_against() {
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response(&veto_summary_body()),
		final_response(&veto_summary_body()),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());

	let mut session = regime_session(&mut config).await;
	session.session.info.api_calls_at_turn_start = session.session.info.total_api_calls;

	assert!(
		settle_folds(&mut session, &config).await,
		"with no plan the model's decline must not hold the fold"
	);
	assert_eq!(
		session.fold_deferral, None,
		"no veto was offered, so nothing is recorded"
	);
	assert!(
		session.session.info.compression_stats.input_tokens > 0,
		"the single paid call landed the fold"
	);
	assert!(
		session
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("COMPRESS-E2E-CONTEXT")),
		"the fold must be applied"
	);

	std::env::remove_var("OLLAMA_API_URL");
}
