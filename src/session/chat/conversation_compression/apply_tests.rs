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

fn cache_message(role: &str, content: &str, cached: bool) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		cached,
		cache_ttl: cached.then(|| "stale".to_string()),
		..Default::default()
	}
}

fn content_marker_indices(messages: &[crate::session::Message]) -> Vec<usize> {
	messages
		.iter()
		.enumerate()
		.filter(|(_, message)| message.role != "system" && message.cached)
		.map(|(index, _)| index)
		.collect()
}

#[test]
fn compression_markers_keep_anchor_and_end_after_skill_and_note_reinjection() {
	let mut messages = vec![
		cache_message("system", "system", true),
		cache_message("assistant", "unchanged welcome anchor", false),
		cache_message("user", "<skill name=\"rust\">rules</skill>", true),
		cache_message("assistant", "compressed summary", true),
		cache_message("user", "<continuation>resume</continuation>", true),
		cache_message("user", "<pay-attention>re-anchor</pay-attention>", false),
	];

	align_compression_cache_markers(&mut messages, 1, 3, true);

	assert_eq!(content_marker_indices(&messages), vec![1, 5]);
	assert_eq!(messages[1].cache_ttl.as_deref(), Some("1h"));
	assert!(
		!messages[2].cached,
		"re-injected skill is between boundaries"
	);
	assert!(
		!messages[3].cached,
		"summary is covered by the final boundary"
	);
	assert!(
		!messages[4].cached,
		"stale pre-reinjection end marker is cleared"
	);
	assert!(messages[5].cached, "final current state gets marker #2");
}

#[test]
fn auto_cache_advance_after_align_keeps_the_anchor_watermark() {
	// The full post-compression sequence: align places [anchor(1h), final],
	// then check_and_apply_auto_cache_threshold runs before the next API
	// request (tool_result_processor / api_executor). The advance must be a
	// no-op — historically it marked the uncached skill behind the frontier
	// and evicted the anchor before its 1h entry was ever written.
	let mut session = crate::session::Session::new(
		"align-advance".to_string(),
		"anthropic:claude-sonnet-4-6".to_string(),
	);
	session.messages = vec![
		cache_message("system", "system", true),
		cache_message("assistant", "unchanged welcome anchor", false),
		cache_message("user", "<skill name=\"rust\">rules</skill>", false),
		cache_message("assistant", "compressed summary", false),
		cache_message("user", "<continuation>resume</continuation>", false),
	];

	align_compression_cache_markers(&mut session.messages, 1, 3, true);
	assert_eq!(content_marker_indices(&session.messages), vec![1, 4]);

	let config = default_config();
	let advanced = crate::session::cache::CacheManager::new()
		.check_and_apply_auto_cache_threshold(&mut session, &config, true, "developer")
		.unwrap();

	assert!(!advanced, "no boundary exists past the cached frontier");
	assert_eq!(
		content_marker_indices(&session.messages),
		vec![1, 4],
		"anchor watermark and final marker must survive the advance"
	);
	assert_eq!(session.messages[1].cache_ttl.as_deref(), Some("1h"));
}

#[test]
fn compression_with_system_anchor_uses_both_content_marker_slots() {
	let mut messages = vec![
		cache_message("system", "system anchor", true),
		cache_message("assistant", "compressed summary", false),
		cache_message("user", "<continuation>resume</continuation>", false),
	];

	align_compression_cache_markers(&mut messages, 0, 1, true);

	assert!(messages[0].cached, "system cache marker remains intact");
	assert_eq!(content_marker_indices(&messages), vec![1, 2]);
	assert_eq!(messages[1].cache_ttl, None, "new summary uses normal TTL");
}

#[test]
fn compression_clears_content_markers_for_non_caching_models() {
	let mut messages = vec![
		cache_message("system", "system", true),
		cache_message("assistant", "anchor", true),
		cache_message("assistant", "summary", true),
		cache_message("user", "continuation", true),
	];

	align_compression_cache_markers(&mut messages, 1, 2, false);

	assert!(content_marker_indices(&messages).is_empty());
	assert!(messages[0].cached, "system marker is managed separately");
}

#[test]
fn continuation_detection_ignores_ordinary_messages() {
	assert!(!is_continuation_message("fix the parser"));
	assert!(!is_continuation_message(""));
	// A mention of the tag mid-message is not a wrapper.
	assert!(!is_continuation_message("talk about <continuation> tags"));

	assert!(is_continuation_message("<continuation>\nbody"));
	// Leading whitespace/newlines still count — the wrapper may be re-indented.
	assert!(is_continuation_message("\n  <continuation>\nbody"));
}

#[test]
fn built_wrapper_round_trips_through_the_extractor() {
	let intent = "add retry logic to the uploader";
	let wrapper = build_continuation_content(None, Some(intent), None, false);
	assert!(is_continuation_message(&wrapper));
	assert_eq!(extract_continuation_task(&wrapper).as_deref(), Some(intent));
	assert!(!wrapper.contains("execution plan is already active"));

	// With an active plan the wrapper gains the continue-the-plan note and
	// the task must still round-trip through the extractor.
	let wrapper = build_continuation_content(None, Some(intent), None, true);
	assert!(wrapper.contains("execution plan is already active"));
	assert_eq!(extract_continuation_task(&wrapper).as_deref(), Some(intent));
}

#[test]
fn pact_continuation_separates_contextual_request_from_validated_frontier() {
	let summary = CompressionSummary {
		folded_units: vec![super::super::schema::FoldedUnit {
				text: "Continue monitoring the 50-case benchmark; monitor mon-debabfb8 is already running."
					.to_string(),
				kind: "next_action".to_string(),
				status: "tentative".to_string(),
				refs: vec!["b:frontier".to_string()],
			}],
		..Default::default()
	};
	let action = select_continuation_action(&summary, true);
	let wrapper =
		build_continuation_content(None, Some("Should work now"), action.as_deref(), false);

	assert_eq!(
		extract_continuation_task(&wrapper).as_deref(),
		Some("Should work now"),
		"runtime task identity must remain the exact user request"
	);
	assert!(wrapper.contains(
			"<task>\nContinue monitoring the 50-case benchmark; monitor mon-debabfb8 is already running.\n</task>"
		));
	assert!(!wrapper.contains("<task>\nShould work now\n</task>"));
}

#[test]
fn pact_continuation_falls_back_to_pending_open_loop_over_completed_request() {
	let summary = CompressionSummary {
		folded_units: vec![
			super::super::schema::FoldedUnit {
				text: "Model swap completed; config verified on box.".to_string(),
				kind: "outcome".to_string(),
				status: "established".to_string(),
				refs: vec!["b:done".to_string()],
			},
			super::super::schema::FoldedUnit {
				text: "Proposed fix pending approval: catch the validation error.".to_string(),
				kind: "open_loop".to_string(),
				status: "pending".to_string(),
				refs: vec!["b:loop".to_string()],
			},
		],
		..Default::default()
	};
	let action = select_continuation_action(&summary, true);
	let wrapper =
		build_continuation_content(None, Some("disable plan also"), action.as_deref(), false);

	assert!(wrapper
		.contains("<task>\nProposed fix pending approval: catch the validation error.\n</task>"));
	assert!(!wrapper.contains("<task>\ndisable plan also\n</task>"));
}

#[test]
fn fallback_wrapper_carries_no_extractable_intent() {
	// Without a real user ask the wrapper holds only the placeholder, which
	// must not propagate as if it were the active task.
	let wrapper = build_continuation_content(None, None, None, false);
	assert!(wrapper.contains(CONTINUATION_FALLBACK_INTENT));
	assert_eq!(extract_continuation_task(&wrapper), None);
}

#[test]
fn extract_returns_none_for_non_wrappers_and_malformed_tags() {
	assert_eq!(extract_continuation_task("plain user message"), None);
	// Wrapper without a task block.
	assert_eq!(extract_continuation_task("<continuation>\nno task"), None);
	// Unclosed task block.
	assert_eq!(
		extract_continuation_task("<continuation>\n<task>\nhalf"),
		None
	);
	// Empty task block.
	assert_eq!(
		extract_continuation_task("<continuation>\n<task></task>"),
		None
	);
}

#[test]
fn extract_trims_and_keeps_multiline_intent() {
	let wrapper = "<continuation>\n<task>\n  first line\n  second line  \n</task>\n</continuation>";
	assert_eq!(
		extract_continuation_task(wrapper).as_deref(),
		Some("first line\n  second line")
	);
}

#[test]
fn extract_handles_multibyte_intent_without_panicking() {
	let intent = "почини парсер 日本語";
	let wrapper = build_continuation_content(None, Some(intent), None, false);
	assert_eq!(extract_continuation_task(&wrapper).as_deref(), Some(intent));
}

#[test]
fn continuation_round_trips_exact_previous_assistant_response() {
	let previous = "  Exact answer\nwith formatting and trailing space ";
	let request = "  exact follow-up\nwith trailing space ";
	let wrapper = build_continuation_content(Some(previous), Some(request), None, false);
	assert_eq!(
		extract_previous_assistant_response(&wrapper).as_deref(),
		Some(previous)
	);
	assert!(wrapper.contains(&format!("<request>{request}</request>")));
}

fn default_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config");
	config.build_role_map();
	config
}

fn plain_message(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

fn drained_session(name: &str) -> ChatSession {
	let mut session = ChatSession::for_tests(vec![
		plain_message("system", "system prompt"),
		plain_message("user", "fix the parser"),
		plain_message("assistant", "on it"),
		plain_message("user", "also add tests"),
		plain_message("assistant", "done with tests"),
	]);
	session.session.info.name = name.to_string();
	session
}

#[tokio::test]
async fn apply_compression_validates_clamps_and_budget_drops_file_contexts() {
	let config = default_config();
	let mut session = drained_session("apply-fc-unit");
	let summary = CompressionSummary {
		should_compress: true,
		original_request: "fix the parser".to_string(),
		current_task: "finish parser tests".to_string(),
		progress: "parser fixed".to_string(),
		file_context: vec![
			super::super::schema::FileContextEntry {
				filepath: "src/parser.rs".to_string(),
				start_line: 1,
				end_line: 10,
			},
			super::super::schema::FileContextEntry {
				filepath: "bad.rs".to_string(),
				start_line: 0,
				end_line: 5,
			},
			super::super::schema::FileContextEntry {
				filepath: "worse.rs".to_string(),
				start_line: 9,
				end_line: 3,
			},
			super::super::schema::FileContextEntry {
				filepath: "big.log".to_string(),
				start_line: 1,
				end_line: 98_765,
			},
			super::super::schema::FileContextEntry {
				filepath: format!("{}.rs", "p".repeat(40_000)),
				start_line: 1,
				end_line: 2,
			},
		],
		..Default::default()
	};
	apply_compression(
		&mut session,
		0,
		4,
		&summary,
		500,
		600,
		vec!["fix the parser".to_string()],
		None,
		None,
		Vec::new(),
		&config,
		None,
		None,
		false,
		false,
	)
	.await
	.expect("apply compression");
	// 5 messages - 4 drained + summary + continuation wrapper.
	assert_eq!(session.session.messages.len(), 3);
	let rendered: String = session
		.session
		.messages
		.iter()
		.map(|m| m.content.as_str())
		.collect::<Vec<_>>()
		.join("\n");
	assert!(rendered.contains("src/parser.rs"));
	assert!(rendered.contains("big.log")); // clamped span survives
	assert!(!rendered.contains("bad.rs")); // start_line 0 rejected
	assert!(!rendered.contains("worse.rs")); // start > end rejected
	assert!(!rendered.contains("ppppp")); // over-budget entry dropped
	assert!(rendered.contains("## EARLIER USER REQUESTS"));
}

#[tokio::test]
async fn apply_compression_pact_live_renders_pact_entry_and_skips_legacy_folds() {
	let mut config = default_config();
	config.compression.attention.enabled = true;
	let mut session = ChatSession::for_tests(vec![
		plain_message("system", "system prompt"),
		plain_message("user", "stabilise the deploy pipeline"),
		plain_message("assistant", "investigating flakiness"),
		plain_message("assistant", "found the race"),
	]);
	session.session.info.name = "apply-pact-unit".to_string();
	let pact = super::super::attention::build(&session, 1, 3, 2.0, true, false)
		.await
		.expect("pact context builds");
	let summary = CompressionSummary {
		should_compress: true,
		current_task: "stabilise the deploy pipeline".to_string(),
		folded_units: vec![super::super::schema::FoldedUnit {
			text: "race identified in runner".to_string(),
			kind: "observation".to_string(),
			status: "established".to_string(),
			refs: vec!["b:nonexistent".to_string()],
		}],
		critical_knowledge: vec!["must never be committed in pact mode".to_string()],
		..Default::default()
	};
	apply_compression(
		&mut session,
		0,
		3,
		&summary,
		800,
		900,
		Vec::new(),
		None,
		None,
		Vec::new(),
		&config,
		Some(&pact),
		None,
		false,
		false,
	)
	.await
	.expect("apply pact compression");
	let summary_message = &session.session.messages[1];
	assert_eq!(
		summary_message.name.as_deref(),
		Some(COMPRESSION_MESSAGE_NAME)
	);
	assert!(summary_message.content.contains("controller=\"pact-v"));
	assert!(session
		.session
		.messages
		.iter()
		.any(|m| m.content.trim_start().starts_with(CONTINUATION_TAG_OPEN)));
	// Legacy narrative fields are wire-compat only in PACT mode — never folded.
	assert!(session.critical_knowledge.is_empty());
	assert!(session.analysis_findings.is_empty());
}

#[tokio::test]
async fn apply_compression_reinserts_preserved_skills_between_anchor_and_summary() {
	let config = default_config();
	let mut session = drained_session("apply-skills-unit");
	let mut skill = plain_message(
		"user",
		"<skill name=\"rust\">follow rust conventions</skill>",
	);
	skill.cached = true;
	skill.cache_ttl = Some("stale".to_string());
	let summary = CompressionSummary {
		should_compress: true,
		current_task: "finish parser tests".to_string(),
		progress: "parser fixed".to_string(),
		..Default::default()
	};
	apply_compression(
		&mut session,
		0,
		4,
		&summary,
		500,
		600,
		Vec::new(),
		None,
		None,
		vec![skill],
		&config,
		None,
		None,
		false,
		false,
	)
	.await
	.expect("apply compression with skills");
	let messages = &session.session.messages;
	assert_eq!(messages.len(), 4); // system + skill + summary + continuation
	assert!(messages[1].content.contains("<skill name=\"rust\">"));
	assert!(!messages[1].cached);
	assert!(messages[1].cache_ttl.is_none());
	assert_eq!(messages[2].name.as_deref(), Some(COMPRESSION_MESSAGE_NAME));
	assert!(messages[3]
		.content
		.trim_start()
		.starts_with(CONTINUATION_TAG_OPEN));
}

#[tokio::test]
async fn apply_compression_seeds_intent_from_anchor_request_or_free_form_fallback() {
	let config = default_config();
	let summary = CompressionSummary {
		should_compress: true,
		progress: "narrative".to_string(),
		..Default::default()
	};

	// A pre-set anchor intent survives untouched.
	let mut session = ChatSession::for_tests(vec![
		plain_message("system", "system"),
		plain_message("assistant", "a"),
		plain_message("assistant", "b"),
	]);
	session.session.info.name = "apply-intent-anchor-unit".to_string();
	session.session.info.anchor.intent = "keep me".to_string();
	apply_compression(
		&mut session,
		0,
		2,
		&summary,
		100,
		200,
		Vec::new(),
		None,
		None,
		Vec::new(),
		&config,
		None,
		None,
		false,
		false,
	)
	.await
	.expect("apply with anchor");
	assert_eq!(session.session.info.anchor.intent, "keep me");

	// Empty anchor falls back to the summary's original_request.
	let mut session = ChatSession::for_tests(vec![
		plain_message("system", "system"),
		plain_message("assistant", "a"),
		plain_message("assistant", "b"),
	]);
	session.session.info.name = "apply-intent-request-unit".to_string();
	let mut summary_with_request = summary.clone();
	summary_with_request.original_request = "orig task".to_string();
	apply_compression(
		&mut session,
		0,
		2,
		&summary_with_request,
		100,
		200,
		Vec::new(),
		None,
		None,
		Vec::new(),
		&config,
		None,
		None,
		false,
		false,
	)
	.await
	.expect("apply with request");
	assert_eq!(session.session.info.anchor.intent, "orig task");

	// Nothing anywhere: the free-form placeholder seeds the anchor.
	let mut session = ChatSession::for_tests(vec![
		plain_message("system", "system"),
		plain_message("assistant", "a"),
		plain_message("assistant", "b"),
	]);
	session.session.info.name = "apply-intent-freeform-unit".to_string();
	apply_compression(
		&mut session,
		0,
		2,
		&summary,
		100,
		200,
		Vec::new(),
		None,
		None,
		Vec::new(),
		&config,
		None,
		None,
		false,
		false,
	)
	.await
	.expect("apply free-form");
	assert_eq!(
		session.session.info.anchor.intent,
		"Free-form conversation session"
	);
}

#[tokio::test]
async fn apply_compression_reports_growth_when_summary_outweighs_drain() {
	let config = default_config();
	let mut session = drained_session("apply-growth-unit");
	let summary = CompressionSummary {
		should_compress: true,
		current_task: "finish parser tests".to_string(),
		progress: "parser fixed".to_string(),
		..Default::default()
	};
	// current_context_tokens = 0 forces post > current: the growth branch fires.
	apply_compression(
		&mut session,
		0,
		4,
		&summary,
		500,
		0,
		Vec::new(),
		None,
		None,
		Vec::new(),
		&config,
		None,
		None,
		false,
		false,
	)
	.await
	.expect("apply compression");
	assert!(session.session.info.context_tokens_after_last_compression > 0);
}

#[tokio::test]
async fn apply_compression_with_tail_bridge_keeps_exchange_without_wrapper() {
	let config = default_config();
	let mut session = drained_session("apply-bridge-unit");
	let summary = CompressionSummary {
		should_compress: true,
		current_task: "finish parser tests".to_string(),
		progress: "parser fixed".to_string(),
		..Default::default()
	};
	apply_compression(
		&mut session,
		0,
		2,
		&summary,
		500,
		600,
		Vec::new(),
		None,
		None,
		Vec::new(),
		&config,
		None,
		None,
		false,
		true,
	)
	.await
	.expect("apply compression with tail bridge");
	// 5 messages - 2 drained + summary; the trailing user/assistant pair stays verbatim.
	assert_eq!(session.session.messages.len(), 4);
	assert!(session
		.session
		.messages
		.iter()
		.all(|m| !m.content.trim_start().starts_with(CONTINUATION_TAG_OPEN)));
	assert_eq!(session.session.messages[3].content, "done with tests");
}

#[test]
fn select_continuation_action_is_disabled_without_pact() {
	let summary = CompressionSummary {
		folded_units: vec![super::super::schema::FoldedUnit {
			text: "run the verifier".to_string(),
			kind: "next_action".to_string(),
			status: "pending".to_string(),
			refs: Vec::new(),
		}],
		..Default::default()
	};
	assert_eq!(select_continuation_action(&summary, false), None);
	assert_eq!(
		select_continuation_action(&summary, true),
		Some("run the verifier".to_string())
	);
}

#[tokio::test]
async fn apply_compression_surfaces_pending_jobs_and_tap_runs_in_wrapper() {
	let config = default_config();
	let session_id = "apply-jobs-unit".to_string();
	crate::session::shell_jobs::register_for_session(
		&session_id,
		"test-mcp",
		"file:///tmp/watched",
		"watch the build",
	);
	let mut session = drained_session(&session_id);
	let summary = CompressionSummary {
		should_compress: true,
		current_task: "finish parser tests".to_string(),
		progress: "parser fixed".to_string(),
		..Default::default()
	};
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::tap_runs::init_for_session();
		let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
		crate::session::tap_runs::register_job(crate::session::tap_runs::TapJob {
			id: "tap-unit-1".to_string(),
			role: "developer:general".to_string(),
			workdir: ".".to_string(),
			started_at: std::time::SystemTime::now(),
			status: std::sync::Arc::new(std::sync::RwLock::new(
				crate::session::tap_runs::TapJobStatus::Running,
			)),
			cancel_tx,
			live: std::sync::Arc::new(std::sync::RwLock::new(
				crate::session::tap_runs::TapLiveState::default(),
			)),
		});
		apply_compression(
			&mut session,
			0,
			4,
			&summary,
			500,
			600,
			Vec::new(),
			None,
			None,
			Vec::new(),
			&config,
			None,
			None,
			false,
			false,
		)
		.await
	})
	.await
	.expect("apply compression inside session context");
	let wrapper = session
		.session
		.messages
		.iter()
		.find(|m| m.content.trim_start().starts_with(CONTINUATION_TAG_OPEN))
		.expect("continuation wrapper");
	assert!(wrapper.content.contains("<background_jobs_running>"));
	assert!(wrapper
		.content
		.contains("watch the build (file:///tmp/watched)"));
	assert!(wrapper.content.contains("<tap_runs_running>"));
	assert!(wrapper.content.contains("developer:general (tap-unit-1)"));
	crate::session::shell_jobs::clear_for_session(&session_id);
	crate::session::tap_runs::clear_for_session(&session_id);
}
