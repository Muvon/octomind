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

//! Dispatch tests for the interactive `/command` surface, driven through the
//! real `process_command` entry with an in-memory session. Only commands
//! without external side effects (network, clipboard, session-file writes)
//! are exercised; the rest are covered by their own module tests.

use super::*;

#[test]
fn status_is_the_only_activity_slash_command() {
	assert!(crate::session::chat::COMMANDS.contains(&"/status"));
	assert!(!crate::session::chat::COMMANDS.contains(&"/agents"));
	assert!(!crate::session::chat::COMMANDS.contains(&"/monitor"));
}

fn test_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

fn cancel_rx() -> tokio::sync::watch::Receiver<bool> {
	tokio::sync::watch::channel(false).1
}

async fn dispatch(
	session: &mut ChatSession,
	config: &mut Config,
	input: &str,
) -> Result<CommandResult> {
	process_command(session, input, config, "assistant", cancel_rx()).await
}

/// Dispatch and, when the command produced typed output, render it through
/// the real CLI display path (what the main loop does with it).
async fn dispatch_rendered(
	session: &mut ChatSession,
	config: &mut Config,
	input: &str,
) -> Result<CommandResult> {
	let result = dispatch(session, config, input).await?;
	if let CommandResult::HandledWithOutput(ref output) = result {
		let mut output = output.clone();
		output.display_cli(session, config).await;
	}
	Ok(result)
}

#[tokio::test]
async fn test_non_commands_are_treated_as_user_input() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();

	for input in ["", "   ", "/definitely-not-a-command", "hello there"] {
		let result = dispatch(&mut session, &mut config, input)
			.await
			.expect("dispatch never errors on unknown input");
		assert!(
			matches!(result, CommandResult::TreatAsUserInput),
			"input {input:?} must fall through to user input"
		);
	}
}

#[tokio::test]
async fn test_exit_command() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();
	let result = dispatch(&mut session, &mut config, "/exit")
		.await
		.expect("exit dispatches");
	assert!(matches!(result, CommandResult::Exit));
}

#[tokio::test]
async fn test_help_alias_dispatches_as_help() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();
	let result = dispatch(&mut session, &mut config, "/?")
		.await
		.expect("help alias dispatches");
	assert!(matches!(
		result,
		CommandResult::HandledWithOutput(ref output) if matches!(**output, CommandOutput::Help { .. })
	));
}

#[tokio::test]
async fn test_loglevel_mutates_config() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();

	let result = dispatch(&mut session, &mut config, "/loglevel debug")
		.await
		.expect("loglevel dispatches");
	assert!(!matches!(result, CommandResult::TreatAsUserInput));
	assert!(config.get_log_level().is_debug_enabled());

	dispatch(&mut session, &mut config, "/loglevel none")
		.await
		.expect("loglevel resets");
	assert!(!config.get_log_level().is_info_enabled());

	// Invalid level is handled (reported), not treated as chat
	let result = dispatch_rendered(&mut session, &mut config, "/loglevel bogus")
		.await
		.expect("invalid level handled");
	assert!(!matches!(result, CommandResult::TreatAsUserInput));
}

#[tokio::test]
async fn test_model_show_and_set() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();

	// Show current model
	dispatch(&mut session, &mut config, "/model")
		.await
		.expect("model show dispatches");

	// Set a new model on the live session
	dispatch(&mut session, &mut config, "/model ollama:other-model")
		.await
		.expect("model set dispatches");
	assert_eq!(session.model, "ollama:other-model");
}

#[tokio::test]
async fn test_model_rejects_invalid_values() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();
	let before = session.model.clone();

	// Each value fails config validation: no provider prefix, an empty model,
	// an empty provider, an unknown provider. Reject it and leave the session
	// untouched. Without the guard the value was saved and the next request
	// failed with no way back.
	for value in [
		"gpt5",
		"openrouter:",
		":gpt-4o",
		"nosuchprovider:some-model",
	] {
		let result = dispatch_rendered(&mut session, &mut config, &format!("/model {value}"))
			.await
			.expect("invalid model is handled, not an error");
		let CommandResult::HandledWithOutput(output) = result else {
			panic!("expected typed output for invalid model {value}");
		};
		assert!(
			matches!(*output, CommandOutput::Error { .. }),
			"{value}: invalid model must report CommandOutput::Error, got {output:?}"
		);
		assert_eq!(
			session.model, before,
			"{value}: rejected model must not be saved"
		);
		assert_eq!(session.session.info.model, before);
	}
}

#[tokio::test]
async fn test_model_accepts_provider_qualified_value() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();

	let result = dispatch(&mut session, &mut config, "/model openrouter:some/model")
		.await
		.expect("valid model dispatches");
	assert!(!matches!(result, CommandResult::TreatAsUserInput));
	assert_eq!(session.model, "openrouter:some/model");
	assert_eq!(session.session.info.model, "openrouter:some/model");
}

#[tokio::test]
async fn test_effort_set() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();

	dispatch(&mut session, &mut config, "/effort high")
		.await
		.expect("effort dispatches");
	assert_eq!(
		session.reasoning_effort,
		Some(crate::config::ReasoningEffortConfig::High)
	);
}

#[tokio::test]
async fn test_display_commands_run_without_panicking() {
	// Populate the session so info/context/report have real content to render
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();
	session.add_user_message("first question").expect("user");
	session
		.add_assistant_message(
			"an answer with some length to it",
			None,
			&config,
			"assistant",
		)
		.expect("assistant message");
	// Non-zero compression stats: /info renders its compression section only
	// when compressions actually happened.
	{
		let cs = &mut session.session.info.compression_stats;
		cs.task_compressions = 1;
		cs.phase_compressions = 1;
		cs.project_compressions = 1;
		cs.conversation_compressions = 2;
		cs.total_messages_removed = 14;
		cs.total_tokens_saved = 9_000;
		cs.input_tokens = 1_200;
		cs.output_tokens = 300;
	}
	// Timing rows and layer sections render only when their totals are set
	session.session.info.total_api_time_ms = 1_500;
	session.session.info.total_tool_time_ms = 300;
	session.session.info.total_layer_time_ms = 200;
	session
		.session
		.add_layer_stats("command:reduce", "ollama:fake-model", 1_000, 200, 0.01);
	session
		.session
		.add_layer_stats("command:reduce", "ollama:fake-model", 900, 150, 0.008);
	session
		.session
		.add_layer_stats("refine", "ollama:fake-model", 500, 100, 0.004);

	for input in [
		"/help",
		"/info",
		"/report",
		"/context",
		"/context all",
		"/context user",
		"/context tool",
		"/context large",
		"/clear",
		"/status",
		"/plan",
		"/list",
		"/model",
		"/effort",
		"/loglevel",
		// Bare invocations render list/usage output without side effects
		"/role",
		"/prompt",
		"/skill",
		"/image",
		"/video",
	] {
		let result = dispatch(&mut session, &mut config, input)
			.await
			.unwrap_or_else(|e| panic!("{input} errored: {e}"));
		match result {
			CommandResult::TreatAsUserInput => {
				panic!("{input} must dispatch as a command")
			}
			// Render the typed output through the real CLI display path —
			// this is what the main loop does with it.
			CommandResult::HandledWithOutput(mut output) => {
				output.display_cli(&mut session, &config).await;
			}
			_ => {}
		}
	}
}

#[tokio::test]
async fn test_reply_dispatches_and_renders() {
	let mut session = ChatSession::for_tests(vec![crate::session::Message {
		role: "assistant".to_string(),
		content: "Tests pass.".to_string(),
		..Default::default()
	}]);
	let mut config = test_config();
	let result = dispatch_rendered(&mut session, &mut config, "/reply")
		.await
		.expect("reply dispatches");
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	assert!(matches!(
		*output,
		CommandOutput::Reply {
			number: 1,
			lines: 1,
			..
		}
	));
}

#[tokio::test]
async fn test_role_switch_without_session_file_fails_gracefully() {
	let mut session = ChatSession::for_tests(Vec::new());
	// Start from a role the template actually defines — for_tests defaults
	// to "core", which only exists in richer configs.
	session.role = "task_refiner".to_string();
	let mut config = test_config();
	// A file-less (in-memory) session cannot persist a role change: the
	// switch must fail GRACEFULLY — error output, role untouched — never
	// a half-switched session. (Successful switches are covered by the
	// binary/ACP e2e tests, which run with real session files.)
	let result = dispatch_rendered(&mut session, &mut config, "/role assistant")
		.await
		.expect("role switch dispatches");
	assert!(!matches!(result, CommandResult::TreatAsUserInput));
	assert_eq!(session.role, "task_refiner", "role must stay unchanged");
}

#[tokio::test]
async fn test_done_on_empty_session_has_nothing_to_compress() {
	// Empty session: /done short-circuits before any model call
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();
	let result = dispatch_rendered(&mut session, &mut config, "/done")
		.await
		.expect("done dispatches");
	assert!(!matches!(result, CommandResult::TreatAsUserInput));
	assert!(session.session.messages.is_empty());
}

#[tokio::test]
async fn test_mcp_subcommands_read_config() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = test_config();
	// The handlers enumerate tools through the global tool map; build it from
	// this config so list/info/full/dump/validate see the real builtin tools.
	crate::mcp::tool_map::initialize_tool_map(&config.get_merged_config_for_role("assistant"))
		.await
		.expect("init tool map");
	// All read-only subcommands. health is safe here: the template config
	// carries only builtin servers, so the forced check never probes an
	// external process.
	for input in [
		"/mcp list",
		"/mcp",
		"/mcp info",
		"/mcp full",
		"/mcp health",
		"/mcp dump",
		"/mcp validate",
		"/mcp bogus-subcommand",
	] {
		let result = dispatch_rendered(&mut session, &mut config, input)
			.await
			.unwrap_or_else(|e| panic!("{input} errored: {e}"));
		assert!(
			!matches!(result, CommandResult::TreatAsUserInput),
			"{input} must dispatch"
		);
	}
}

/// Direct handler check: with the template's builtin servers the /mcp data
/// payloads must actually carry servers and tools — an empty listing here
/// means function enumeration silently broke.
#[tokio::test]
async fn test_mcp_handlers_enumerate_builtin_tools() {
	let config = test_config();

	let result =
		crate::session::chat::session::commands::mcp::handle_mcp(&config, "assistant", &["list"])
			.await
			.expect("mcp list");
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected output");
	};
	let CommandOutput::Mcp { data, .. } = *output else {
		panic!("expected Mcp output");
	};
	let servers = data
		.get("servers")
		.and_then(|v| v.as_object())
		.expect("servers object");
	assert!(!servers.is_empty(), "no servers enumerated: {data}");
	let total_tools: usize = servers
		.values()
		.filter_map(|v| v.as_array())
		.map(|a| a.len())
		.sum();
	assert!(total_tools > 0, "no tools enumerated: {data}");

	for sub in ["info", "full", "validate", "dump"] {
		let result =
			crate::session::chat::session::commands::mcp::handle_mcp(&config, "assistant", &[sub])
				.await
				.unwrap_or_else(|e| panic!("mcp {sub} errored: {e}"));
		let CommandResult::HandledWithOutput(output) = result else {
			panic!("mcp {sub}: expected output");
		};
		let CommandOutput::Mcp { data, .. } = *output else {
			panic!("mcp {sub}: expected Mcp output");
		};
		let text = data.to_string();
		assert!(
			text.contains("schedule") || text.contains("tools"),
			"mcp {sub} payload looks empty: {text}"
		);
	}
}

/// Inline display_cli arms (copy/rename/error) that no dispatchable command
/// reaches without a clipboard or a session file.
#[tokio::test]
async fn test_inline_output_render_arms() {
	let mut session = ChatSession::for_tests(Vec::new());
	let config = test_config();
	let outputs = [
		CommandOutput::Copy {
			copied: true,
			length: Some(42),
			scope: "last".to_string(),
		},
		CommandOutput::Copy {
			copied: true,
			length: None,
			scope: "all".to_string(),
		},
		CommandOutput::Copy {
			copied: false,
			length: None,
			scope: "user".to_string(),
		},
		CommandOutput::Rename {
			session_name: "s".to_string(),
			title: Some("new title".to_string()),
		},
		CommandOutput::Rename {
			session_name: "s".to_string(),
			title: None,
		},
		CommandOutput::Error {
			error: "something broke".to_string(),
			context: Some(serde_json::json!({"where": "here"})),
		},
	];
	for output in outputs {
		let mut output = output.clone();
		output.display_cli(&mut session, &config).await;
	}
}

#[tokio::test]
async fn info_throughput_counts_reasoning_tokens() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.output_tokens = 406;
	session.session.info.reasoning_tokens = 8_900;
	session.session.info.input_tokens = 100;
	session.session.info.cache_read_tokens = 500;
	session.session.info.cache_write_tokens = 20;
	session.session.info.total_api_time_ms = 63_400;
	session.session.info.total_api_calls = 2;
	session.session.info.turn_timing.completed = 2;
	session.session.info.turn_timing.total_time_ms = 90_000;
	session.session.info.turn_timing.last_time_ms = 40_000;
	session.session.info.supervisor_accounting.turn_time =
		crate::supervisor::accounting::TurnTime {
			completed: 2,
			total_time_ms: 90_000,
			main_api_time_ms: 63_400,
			blocking_supervisor_time_ms: 10_000,
		};
	let mut config = test_config();
	let result = dispatch(&mut session, &mut config, "/info")
		.await
		.expect("info succeeds");
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected rendered info output");
	};
	let json = output.to_json();
	assert_eq!(json["timing"]["model_time_ms"], 63_400);
	assert_eq!(json["timing"]["supervisor_blocking_time_ms"], 10_000);
	assert_eq!(json["timing"]["tools_runtime_time_ms"], 16_600);
	assert_eq!(json["timing"]["avg_request_time_ms"], 31_700);
	assert_eq!(json["timing"]["completed_turns"], 2);
	assert_eq!(json["timing"]["avg_turn_time_ms"], 45_000);

	let CommandOutput::Info {
		tokens_used,
		tokens_input,
		tokens_cached,
		tokens_cache_write,
		tokens_reasoning,
		tokens_per_second,
		timing,
		..
	} = *output
	else {
		panic!("expected info output variant");
	};
	assert_eq!(tokens_used, 9_926);
	assert_eq!(tokens_input, 100);
	assert_eq!(tokens_cached, 500);
	assert_eq!(tokens_cache_write, 20);
	assert_eq!(tokens_reasoning, 8_900);
	// (406 + 8_900) / 63.4 s ≈ 146.8 tok/s; output-only math would show 6.4.
	assert!(
		(tokens_per_second - 146.8).abs() < 0.1,
		"{tokens_per_second}"
	);
	assert_eq!(timing.last_turn_time_ms, 40_000);
}

#[tokio::test]
async fn new_preserves_old_identity_while_extraction_completes_for_its_owner() {
	crate::supervisor::accounting::isolated(async {
		use crate::session::context::{
			cleanup_session, current_session_id, init_session_services, with_session_id,
		};
		use crate::supervisor::{
			accounting,
			stats::{record_call, CallKind},
		};
		let dir = tempfile::tempdir().unwrap();
		let mut old = ChatSession::for_tests(Vec::new());
		let old_id = uuid::Uuid::new_v4().to_string();
		old.session.info.name = old_id.clone();
		old.session.session_file = Some(dir.path().join("old.jsonl.zst"));
		let (started_tx, started_rx) = tokio::sync::oneshot::channel();
		let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
		let extraction = tokio::spawn(accounting::run_background(
			old_id.clone(),
			old.session.session_file.clone(),
			async move {
				record_call(CallKind::Distill, 20, 5, 0, 10, 0.25);
				started_tx.send(()).unwrap();
				finish_rx.await.unwrap();
				Ok(())
			},
		));
		started_rx.await.unwrap();
		let mut config = test_config();
		let CommandResult::SwitchSession(new_id) =
			with_session_id(old_id.clone(), dispatch(&mut old, &mut config, "/new"))
				.await
				.unwrap()
		else {
			panic!("new session switch");
		};
		assert_eq!(old.session.info.name, old_id);
		old.save().unwrap();
		let mut new = crate::session::Session::new(new_id.clone(), "model".into());
		with_session_id(new_id.clone(), async {
			init_session_services("assistant");
			assert_eq!(current_session_id(), Some(new_id.clone()));
			record_call(CallKind::Plan, 7, 2, 0, 3, 0.5);
		})
		.await;
		// Even a caller still in the old scope must fold the receiver's own banks.
		with_session_id(old_id.clone(), async {
			new.fold_external_spend();
		})
		.await;
		assert_eq!(
			new.info.supervisor_accounting.kinds[&CallKind::Plan].calls,
			1
		);
		finish_tx.send(()).unwrap();
		extraction.await.unwrap().unwrap();
		new.fold_external_spend();
		assert!(new.info.supervisor_accounting.background_runs.is_empty());
		old.session.fold_external_spend();
		assert_eq!(
			old.session.info.supervisor_accounting.kinds[&CallKind::Distill].cost,
			0.25
		);
		assert_eq!(
			old.session.info.supervisor_accounting.background_runs.len(),
			1
		);
		assert_eq!(
			crate::session::persistence::load_session(old.session.session_file.as_ref().unwrap())
				.unwrap()
				.info
				.name,
			old_id
		);
		cleanup_session(&new_id);
	})
	.await;
}
