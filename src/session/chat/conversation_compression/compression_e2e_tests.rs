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

use super::*;
use crate::session::chat::session::ChatSession;
use crate::session::chat::test_support::{
	fake_provider_config, final_response, spawn_stub, ENV_LOCK,
};

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
	config.compression.decision.model = "ollama:fake-model".to_string();

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

#[tokio::test]
async fn test_unparseable_summary_errors_and_keeps_messages() {
	let _guard = ENV_LOCK.lock().await;
	// A garbage decision response must surface an error and leave the
	// session untouched — never a partial drain.
	let url = spawn_stub(vec![final_response("not xml at all")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let mut config = fake_provider_config();
	config.compression.decision.model = "ollama:fake-model".to_string();

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
	config.compression.decision.model = "ollama:fake-model".to_string();
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
	config.compression.decision.model = "ollama:fake-model".to_string();
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
