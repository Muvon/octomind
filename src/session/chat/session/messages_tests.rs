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
fn message_lifecycle_persists_each_role_and_runtime_summary() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("messages.jsonl.zst");
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.session_file = Some(path.clone());
	session.session.info.name = "message-lifecycle".into();
	let config = crate::session::chat::test_support::fake_provider_config();

	session.add_system_message("system prompt").unwrap();
	session.add_user_message("user request").unwrap();
	session
		.add_system_managed_turn_message("<system-note>event</system-note>")
		.unwrap();
	session
		.add_tool_message("tool result", "call-1", "view", &config, Vec::new(), Vec::new())
		.unwrap();
	session
		.add_assistant_message("assistant answer", None, &config, "assistant")
		.unwrap();
	session.save().unwrap();

	assert!(path.exists());
	assert_eq!(session.session.messages[0].role, "system");
	assert_eq!(session.session.messages[1].role, "user");
	assert_eq!(session.session.messages[2].role, "user");
	assert_eq!(session.session.messages[3].role, "tool");
	assert_eq!(session.session.messages[4].role, "assistant");
	assert_eq!(session.last_response, "assistant answer");
	assert_eq!(session.turn_answers, vec!["assistant answer"]);
}

#[test]
fn disabled_spending_limits_continue_and_request_checkpoint_tracks_cost() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = crate::session::chat::test_support::fake_provider_config();
	config.max_session_spending_threshold = 0.0;
	config.max_request_spending_threshold = -1.0;
	session.session.info.total_cost = 2.5;
	assert!(session.check_spending_threshold(&config).unwrap());
	assert!(session.check_request_spending_threshold(&config).unwrap());
	session.start_request_spending_tracking();
	assert_eq!(session.request_spending_checkpoint, 2.5);
}

#[test]
fn spending_thresholds_stop_execution_only_when_exceeded() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = crate::session::chat::test_support::fake_provider_config();

	// Disabled thresholds never gate
	config.max_session_spending_threshold = 0.0;
	config.max_request_spending_threshold = 0.0;
	assert!(session.check_spending_threshold(&config).unwrap());
	assert!(session.check_request_spending_threshold(&config).unwrap());

	// Under threshold → continue
	config.max_session_spending_threshold = 1.0;
	session.session.info.total_cost = 0.5;
	assert!(session.check_spending_threshold(&config).unwrap());

	// Over threshold with non-interactive stdin → auto-decline
	session.session.info.total_cost = 2.0;
	assert!(!session.check_spending_threshold(&config).unwrap());
	assert_eq!(session.spending_stop, Some(SpendingStop::Session));

	// Request-level threshold stops the request
	config.max_request_spending_threshold = 0.5;
	session.start_request_spending_tracking();
	session.session.info.total_cost += 1.0;
	assert!(!session.check_request_spending_threshold(&config).unwrap());
	assert_eq!(session.spending_stop, Some(SpendingStop::Request));
	session.start_request_spending_tracking();
	assert_eq!(session.spending_stop, None);
}

#[test]
fn user_message_is_added_and_gate_eligible() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.add_user_message("hello").unwrap();
	assert_eq!(session.session.messages.len(), 1);
	assert_eq!(session.session.messages[0].role, "user");
	assert!(session.completion_gate_eligible);
}

#[test]
fn system_managed_turn_message_is_wrapped_and_not_turn_owned() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.completion_gate_eligible = true;
	session.add_system_managed_turn_message("background event").unwrap();
	let message = session.session.messages.last().unwrap();
	assert_eq!(message.role, "user");
	assert!(message.content.contains("background event"));
	assert!(!session.completion_gate_eligible);
}

#[test]
fn deferred_turn_hands_eligibility_to_the_resuming_delivery_once() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.completion_gate_eligible = true;
	session.gate_deferred = true;
	session.add_system_managed_turn_message("job result").unwrap();
	assert!(session.completion_gate_eligible);
	assert!(!session.gate_deferred);
	session.add_system_managed_turn_message("schedule fired").unwrap();
	assert!(!session.completion_gate_eligible);
	session.gate_deferred = true;
	session.add_user_message("new request").unwrap();
	assert!(!session.gate_deferred);
}

#[test]
fn assistant_message_tracks_usage_and_cost_from_exchange() {
	let mut session = ChatSession::for_tests(Vec::new());
	let config = crate::session::chat::test_support::fake_provider_config();
	let usage = crate::session::TokenUsage {
		input_tokens: 100,
		cache_read_tokens: 20,
		cache_write_tokens: 10,
		output_tokens: 50,
		reasoning_tokens: 5,
		total_tokens: 185,
		cost: Some(0.5),
		request_time_ms: Some(250),
	};
	let exchange = crate::session::ProviderExchange::new(
		serde_json::json!({}),
		serde_json::json!({
			"usage": {"cost": 0.25},
			"output_items": [
				{"type": "reasoning", "id": "rs_1", "encrypted_content": "opaque", "summary": []},
				{"type": "message", "role": "assistant", "phase": "final_answer", "content": [{"type": "output_text", "text": "answer"}]}
			]
		}),
		Some(usage),
		"test",
	);
	session
		.add_assistant_message("answer", Some(exchange), &config, "assistant")
		.unwrap();
	assert_eq!(session.session.messages.last().unwrap().role, "assistant");
	assert_eq!(
		session
			.session
			.messages
			.last()
			.unwrap()
			.provider_items
			.as_ref()
			.unwrap()[1]["phase"],
		"final_answer"
	);
	assert_eq!(session.last_response, "answer");
	assert_eq!(session.session.info.total_api_calls, 1);
	// Normalized usage.cost wins over the raw response cost
	assert!((session.session.info.total_cost - 0.5).abs() < 1e-9);
	assert_eq!(session.session.info.total_api_time_ms, 250);
	assert_eq!(session.turn_answers, vec!["answer".to_string()]);
}

#[tokio::test]
async fn inbox_batch_takes_turn_semantics_once_from_its_head() {
	use crate::session::inbox::{InboxMessage, InboxSource};

	let mut session = ChatSession::for_tests(Vec::new());
	session.completion_gate_eligible = true;
	session.gate_deferred = true;
	let batch = vec![
		InboxMessage {
			source: InboxSource::BackgroundJob {
				id: "job-a".to_string(),
			},
			content: "job a finished".to_string(),
		},
		InboxMessage {
			source: InboxSource::BackgroundJob {
				id: "job-b".to_string(),
			},
			content: "job b failed".to_string(),
		},
	];

	let config = crate::session::chat::test_support::fake_provider_config();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	session.add_inbox_batch(batch, &config, rx).await.unwrap();

	assert_eq!(session.session.messages.len(), 2);
	assert!(session.session.messages[0].content.contains("job a finished"));
	assert!(session.session.messages[1].content.contains("job b failed"));
	// The head claims the deferred turn; the tail rides along without blanking it
	// (a second add_system_managed_turn_message would — see the deferred-turn test).
	assert!(session.completion_gate_eligible);
	assert!(!session.gate_deferred);
}

#[tokio::test]
async fn inbox_batch_headed_by_a_user_message_owns_the_turn() {
	use crate::session::inbox::{InboxMessage, InboxSource};

	let mut session = ChatSession::for_tests(Vec::new());
	session.completion_gate_eligible = false;
	let batch = vec![InboxMessage {
		source: InboxSource::Inject,
		content: "a new request".to_string(),
	}];

	let config = crate::session::chat::test_support::fake_provider_config();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	session.add_inbox_batch(batch, &config, rx).await.unwrap();

	assert_eq!(session.session.messages.len(), 1);
	assert_eq!(session.session.messages[0].role, "user");
	assert_eq!(session.session.messages[0].content, "a new request");
	assert!(session.completion_gate_eligible);
}

#[tokio::test]
async fn empty_inbox_batch_changes_nothing() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.completion_gate_eligible = false;
	let config = crate::session::chat::test_support::fake_provider_config();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	session.add_inbox_batch(Vec::new(), &config, rx).await.unwrap();
	assert!(session.session.messages.is_empty());
	assert!(!session.completion_gate_eligible);
}

/// Both inbox sinks shape a background job's result like a tool result before
/// it reaches the transcript.
#[tokio::test]
async fn inbox_sinks_shape_background_job_results() {
	use crate::session::inbox::{InboxMessage, InboxSource};

	let mut config = crate::session::chat::test_support::fake_provider_config();
	config.mcp_response_tokens_threshold = 50;
	let result = || InboxMessage {
		source: InboxSource::BackgroundJob {
			id: "octofs://jobs/1".to_string(),
		},
		content: crate::session::shell_jobs::delivery_message(
			"<background_job resource=\"octofs://jobs/1\">",
			&"line of output\n".repeat(500),
		),
	};
	let mut session = ChatSession::for_tests(Vec::new());

	let (_tx, rx) = tokio::sync::watch::channel(false);
	session
		.add_inbox_batch(vec![result()], &config, rx.clone())
		.await
		.unwrap();
	session.add_mid_turn_inbox(vec![result()], &config, rx).await.unwrap();

	assert_eq!(session.session.messages.len(), 2);
	for message in &session.session.messages {
		assert!(
			message
				.content
				.contains(crate::utils::truncation::TRUNCATION_NOTICE_TAG),
			"{}",
			message.content
		);
	}
}

#[test]
fn user_turn_clears_fold_cooldown_but_system_managed_injection_does_not() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.total_api_calls = 40;
	session.fold_cooldown_until_call = 50;

	session.add_user_message("new request").unwrap();
	assert_eq!(
		session.fold_cooldown_until_call, 0,
		"a genuine user turn must clear the call-based fold cooldown"
	);

	session.fold_cooldown_until_call = 50;
	session.add_system_managed_user_message("control-plane note").unwrap();
	assert_eq!(
		session.fold_cooldown_until_call, 50,
		"a system-managed injection must not clear the cooldown"
	);
}

#[test]
fn user_turn_clears_gate_findings_but_keeps_their_count() {
	use crate::session::accounting::{report::Outcome, CallKind, Usage};
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.supervisor_accounting.kinds.insert(
		CallKind::Gate,
		Usage {
			outcome: std::collections::BTreeMap::from([(Outcome::Reported, 3)]),
			reported_findings: vec!["condition 1 unsettled: log not retained".to_string()],
			..Default::default()
		},
	);

	session.add_user_message("new request").unwrap();
	let gate = &session.session.info.supervisor_accounting.kinds[&CallKind::Gate];
	assert!(
		gate.reported_findings.is_empty(),
		"findings belong to the latest turn only"
	);
	assert_eq!(
		gate.outcome[&Outcome::Reported],
		3,
		"the lifetime count survives the turn"
	);
}

#[test]
fn user_turn_clears_a_pending_fold_deferral_but_system_managed_injection_does_not() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.fold_deferral = Some(crate::session::chat::conversation_compression::FoldDeferral {
		reason: crate::session::chat::conversation_compression::DeferReason::MidDerivation,
		step: Some(0),
	});

	session.add_system_managed_user_message("control-plane note").unwrap();
	assert_eq!(
		session.fold_deferral,
		Some(crate::session::chat::conversation_compression::FoldDeferral {
			reason: crate::session::chat::conversation_compression::DeferReason::MidDerivation,
			step: Some(0),
		}),
		"a system-managed injection must not clear a deferral"
	);

	session.add_user_message("new request").unwrap();
	assert_eq!(
		session.fold_deferral, None,
		"a genuine user turn is a natural seam and must clear a pending deferral"
	);
}

#[test]
fn tool_image_messages_roundtrip_through_session_persistence() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("tool-images.jsonl.zst");
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.session_file = Some(path.clone());
	let config = crate::session::chat::test_support::fake_provider_config();
	session.add_user_message("inspect screenshots").unwrap();
	let parent = crate::session::Message {
		role: "assistant".into(),
		tool_calls: Some(serde_json::json!([
			{"id": "mixed", "type": "function", "function": {"name": "screenshot", "arguments": "{}"}},
			{"id": "image-only", "type": "function", "function": {"name": "screenshot", "arguments": "{}"}},
			{"id": "text-only", "type": "function", "function": {"name": "screenshot", "arguments": "{}"}}
		])),
		..Default::default()
	};
	crate::session::append_to_session_file(&path, &serde_json::to_string(&parent).unwrap()).unwrap();
	session.session.messages.push(parent);
	for (id, text, has_image) in [
		("mixed", "caption", true),
		("image-only", "", true),
		("text-only", "body", false),
	] {
		let images = if has_image {
			vec![crate::session::image::ImageAttachment {
				data: crate::session::image::ImageData::Base64("aW1hZ2U=".into()),
				media_type: "image/png".into(),
				source_type: crate::session::image::SourceType::Url,
				dimensions: None,
				size_bytes: None,
			}]
		} else {
			Vec::new()
		};
		session
			.add_tool_message(text, id, "screenshot", &config, images, Vec::new())
			.unwrap();
	}
	session
		.add_assistant_message("done", None, &config, "assistant")
		.unwrap();
	session.save().unwrap();
	let restored = crate::session::load_session(&path).unwrap();
	let original: Vec<_> = session.session.messages.iter().filter(|m| m.role == "tool").collect();
	let loaded: Vec<_> = restored.messages.iter().filter(|m| m.role == "tool").collect();
	assert_eq!(loaded.len(), 3);
	for (original, loaded) in original.iter().zip(loaded) {
		assert_eq!(
			serde_json::to_value(original).unwrap(),
			serde_json::to_value(loaded).unwrap()
		);
	}
}

#[test]
fn a_real_user_turn_clears_the_learning_leftovers() {
	let mut session = ChatSession::for_tests(Vec::new());
	session
		.recalled_refs
		.push(("M1".into(), "memory".into(), "assistant".into(), "project".into()));
	session.used_memory_ids.insert("M1".into());
	session.add_user_message("Next task").unwrap();
	assert!(session.recalled_refs.is_empty());
	assert!(session.used_memory_ids.is_empty());
}

// ---------------------------------------------------------------------------
// Plugin events at the prompt
// ---------------------------------------------------------------------------

use crate::plugins::test_support::{connect_stub, StubPlugin};
use serde_json::json;

/// A session under its own id: stubs attach to the process-wide plugin host by
/// session id, and tests run in parallel.
async fn plugin_session(id: &str, stub: &StubPlugin) -> ChatSession {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.name = id.to_string();
	connect_stub(id, "assistant", std::path::Path::new("."), stub).await;
	session
}

#[tokio::test]
async fn plugin_rewrites_the_prompt_and_its_note_lands_before_the_user_message() {
	let stub =
		StubPlugin::new(&[event::PROMPT_SUBMIT]).answer(json!({"replace_prompt": "rewritten", "note": ["NOTE-FIRST"]}));
	let mut session = plugin_session("o5-prompt-rewrite", &stub).await;
	session.session.info.anchor.intent = "ship the release".to_string();

	let prompt = session.submit_prompt("piped".to_string(), "typed").await.unwrap();
	session.add_user_message(&prompt).unwrap();

	assert_eq!(prompt, "rewritten");
	let heard = &stub.sent(event::PROMPT_SUBMIT)[0];
	assert_eq!(heard["prompt"], "piped");
	assert_eq!(heard["original"], "typed");
	assert_eq!(heard["goal"], "ship the release");
	assert!(heard["carried_summary"]
		.as_str()
		.unwrap()
		.contains("<intent>ship the release</intent>"));
	assert!(session.session.messages[0].content.contains("NOTE-FIRST"));
	assert_eq!(session.session.messages[1].content, "rewritten");

	crate::plugins::end_session("o5-prompt-rewrite");
}

#[tokio::test]
async fn unchanged_prompt_carries_no_original_and_an_empty_anchor_no_summary() {
	let stub = StubPlugin::new(&[event::PROMPT_SUBMIT]);
	let mut session = plugin_session("o5-prompt-plain", &stub).await;

	let prompt = session.submit_prompt("same".to_string(), "same").await.unwrap();

	assert_eq!(prompt, "same");
	let heard = &stub.sent(event::PROMPT_SUBMIT)[0];
	assert!(heard.get("original").is_none());
	assert!(heard.get("carried_summary").is_none());
	assert!(heard.get("goal").is_none());

	crate::plugins::end_session("o5-prompt-plain");
}

#[tokio::test]
async fn plugin_block_stops_the_prompt_with_its_message() {
	let stub = StubPlugin::new(&[event::PROMPT_SUBMIT]).answer(json!({"block": "not on a Friday"}));
	let mut session = plugin_session("o5-prompt-block", &stub).await;

	let error = session.submit_prompt("deploy".to_string(), "deploy").await.unwrap_err();

	assert_eq!(error.to_string(), "not on a Friday");
	assert!(session.session.messages.is_empty());

	crate::plugins::end_session("o5-prompt-block");
}

#[tokio::test]
async fn prompt_goes_unchanged_to_a_plugin_that_did_not_ask_for_it() {
	let stub = StubPlugin::new(&[event::TURN_END]).answer(json!({"replace_prompt": "never used"}));
	let mut session = plugin_session("o5-prompt-unsubscribed", &stub).await;

	let prompt = session.submit_prompt("as typed".to_string(), "as typed").await.unwrap();

	assert_eq!(prompt, "as typed");
	assert!(stub.sent(event::PROMPT_SUBMIT).is_empty());

	crate::plugins::end_session("o5-prompt-unsubscribed");
}

#[tokio::test]
async fn inbox_head_that_becomes_a_user_turn_is_submitted_to_plugins() {
	use crate::session::inbox::{InboxMessage, InboxSource};

	let stub = StubPlugin::new(&[event::PROMPT_SUBMIT]).answer(json!({"replace_prompt": "rewritten request"}));
	let mut session = plugin_session("o5-prompt-inbox", &stub).await;
	let batch = vec![InboxMessage {
		source: InboxSource::Inject,
		content: "a new request".to_string(),
	}];
	let config = crate::session::chat::test_support::fake_provider_config();
	let (_tx, rx) = tokio::sync::watch::channel(false);

	session.add_inbox_batch(batch, &config, rx).await.unwrap();

	assert_eq!(session.session.messages.len(), 1);
	assert_eq!(session.session.messages[0].content, "rewritten request");
	assert_eq!(stub.sent(event::PROMPT_SUBMIT)[0]["prompt"], "a new request");

	crate::plugins::end_session("o5-prompt-inbox");
}

#[tokio::test]
async fn inbox_item_a_plugin_blocks_is_dropped_and_the_rest_of_the_batch_goes_on() {
	use crate::session::inbox::{InboxMessage, InboxSource};

	let stub = StubPlugin::new(&[event::PROMPT_SUBMIT]).respond(|_, params| {
		Ok(if params["prompt"] == "forbidden" {
			json!({"block": "not on a Friday"})
		} else {
			json!({})
		})
	});
	let mut session = plugin_session("o5-prompt-inbox-block", &stub).await;
	let item = |content: &str| InboxMessage {
		source: InboxSource::Inject,
		content: content.to_string(),
	};
	let config = crate::session::chat::test_support::fake_provider_config();
	let (_tx, rx) = tokio::sync::watch::channel(false);

	session
		.add_inbox_batch(vec![item("forbidden"), item("fine request")], &config, rx)
		.await
		.expect("a refused item does not end the batch");

	assert_eq!(session.session.messages.len(), 1);
	assert_eq!(session.session.messages[0].content, "fine request");

	crate::plugins::end_session("o5-prompt-inbox-block");
}

#[test]
fn a_user_turn_resets_what_plugins_track_per_turn() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.plugin_continuations = 3;
	session.turn_mutated_paths.insert("/work/a.rs".to_string());
	session.plugin_verified = Some(true);

	session
		.add_system_managed_user_message("<system-note>note</system-note>")
		.unwrap();
	assert_eq!(session.plugin_continuations, 3, "a note does not start a turn");

	session.add_user_message("new request").unwrap();
	assert_eq!(session.plugin_continuations, 0);
	assert!(session.turn_mutated_paths.is_empty());
	assert_eq!(session.plugin_verified, None);
}

// A block's text joins the history once; a block whose copy left the history,
// as when a compaction folds it, is added again.
#[tokio::test]
async fn plugin_context_joins_the_history_once_and_again_after_its_copy_is_gone() {
	let stub =
		StubPlugin::new(&[event::PROMPT_SUBMIT]).answer(json!({"context": [{"name": "facts", "text": "FACTS"}]}));
	let mut session = plugin_session("o5-context-delivery", &stub).await;
	session.submit_prompt("go".to_string(), "go").await.unwrap();
	let copies = |session: &ChatSession| {
		session
			.session
			.messages
			.iter()
			.filter(|message| message.content.contains("FACTS"))
			.count()
	};

	session.deliver_plugin_context().unwrap();
	session.deliver_plugin_context().unwrap();
	assert_eq!(copies(&session), 1);
	let name = format!("plugin_context:{}:facts", stub.name());
	assert_eq!(session.session.messages[0].name.as_deref(), Some(name.as_str()));

	session.session.messages.clear();
	session.deliver_plugin_context().unwrap();
	assert_eq!(copies(&session), 1);

	crate::plugins::end_session("o5-context-delivery");
}
