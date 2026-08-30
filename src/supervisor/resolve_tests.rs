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

//! Complementary unit tests for the completion-gate resolver: truncation
//! bounds, history rendering, classifier/resolver parsing edge cases, and
//! policy-update validation beyond the inline module tests.

use super::*;

fn message(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

fn context(request: &str) -> TaskContext {
	TaskContext {
		current_request: request.to_string(),
		recent_history: "Earlier user: Schedule the status check every two hours\n".to_string(),
		session_context: "<intent>Implement websocket acknowledgements</intent>".to_string(),
		active_plan: "Implement the active websocket acknowledgement task".to_string(),
		role_context: String::new(),
		verification_policy: crate::supervisor::VerificationPolicy::Unspecified,
		recent_user_policy_context: Vec::new(),
	}
}

#[test]
fn truncate_chars_is_char_based_and_appends_ellipsis() {
	assert_eq!(truncate_chars("abc", 3), "abc");
	assert_eq!(truncate_chars("abcd", 3), "abc…");
	assert_eq!(truncate_chars("", 5), "");
	// Counts chars, not bytes: the accented é must not split the budget.
	assert_eq!(truncate_chars("héllo", 4), "héll…");
}

#[test]
fn truncate_head_tail_keeps_both_ends() {
	assert_eq!(truncate_head_tail("abc", 10), "abc");
	assert_eq!(truncate_head_tail("0123456789AB", 10), "01234\n…\n789AB");
	let long = format!("{}END", "a".repeat(50));
	let cut = truncate_head_tail(&long, 10);
	assert!(cut.starts_with('a') && cut.ends_with("END") && cut.contains('…'));
}

#[test]
fn recent_history_renders_last_three_turns_with_answers() {
	let mut messages = Vec::new();
	for i in 0..5 {
		messages.push(message("user", &format!("task {i}")));
		messages.push(message("assistant", &format!("answer {i}")));
	}
	let history = render_recent_history(&messages);
	assert!(history.contains("Earlier user: task 2"));
	assert!(history.contains("Earlier assistant: answer 2"));
	assert!(history.contains("task 4") && history.contains("answer 4"));
	assert!(!history.contains("task 1"));
	assert!(!history.contains("task 0"));
}

#[test]
fn recent_history_ignores_orphan_assistant_messages() {
	let orphan = render_recent_history(&[
		message("assistant", "orphan"),
		message("user", "u1"),
		message("assistant", "a1"),
	]);
	assert!(!orphan.contains("orphan"));
	assert!(orphan.contains("Earlier user: u1"));
	assert!(orphan.contains("Earlier assistant: a1"));
	assert_eq!(render_recent_history(&[]), "");
}

#[test]
fn recent_real_user_turns_cap_and_order() {
	let messages: Vec<Message> = (0..10).map(|i| message("user", &format!("u{i}"))).collect();
	let turns = recent_real_user_turns(&messages);
	assert_eq!(turns.len(), 8, "capped at POLICY_HISTORY_TURN_CAP");
	assert_eq!(turns[0], "u2");
	assert_eq!(turns[7], "u9");
}

#[test]
fn self_contained_task_defaults_are_conservative() {
	let task = ResolvedTask::self_contained("Write tests");
	assert_eq!(task.original_request, "Write tests");
	assert_eq!(task.resolved_request, "Write tests");
	assert_eq!(task.scope, ResolutionScope::SelfContained);
	assert!(task.context_sources.is_empty());
	assert!(task.resolution_evidence.is_empty());
	assert!(!task.plan_relevant);
	assert!(!task.forbids_verification);
	assert!(!task.answer_only);
	assert_eq!(
		task.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Unchanged
	);
	assert!(task.evidence_conditions.is_empty());
	assert_eq!(task.plan_at_turn_start, "");
}

#[test]
fn resolution_scope_str_is_stable() {
	assert_eq!(ResolutionScope::SelfContained.as_str(), "self_contained");
	assert_eq!(ResolutionScope::FollowUp.as_str(), "follow_up");
	assert_eq!(ResolutionScope::Ambiguous.as_str(), "ambiguous");
}

#[test]
fn capture_requires_a_real_user_turn() {
	assert!(TaskContext::capture(
		&[],
		"",
		None,
		crate::supervisor::VerificationPolicy::Unspecified
	)
	.is_none());
	assert!(TaskContext::capture(
		&[message("assistant", "hi")],
		"",
		None,
		crate::supervisor::VerificationPolicy::Unspecified
	)
	.is_none());
}

#[test]
fn classifier_trims_filters_and_caps_conditions() {
	let parsed = parse_classifier(
		r#"{"scope":"self_contained","conditions":["  run tests  ","","   ","build"]}"#,
	);
	assert_eq!(parsed.conditions, vec!["run tests", "build"]);
	let many: Vec<String> = (0..30).map(|i| format!("c{i}")).collect();
	let wire = format!(
		r#"{{"scope":"self_contained","conditions":{}}}"#,
		serde_json::to_string(&many).expect("serializes")
	);
	let capped = parse_classifier(&wire);
	assert_eq!(capped.conditions.len(), 24);
	assert_eq!(capped.conditions[0], "c0");
}

#[test]
fn classifier_operational_constraints_acceptance_and_rejection() {
	// Well-formed array of excerpts → captured verbatim after trim/filter.
	let parsed = parse_classifier(
		r#"{"scope":"self_contained","operational_constraints":["  we work on the remote server  ","","i will rerun it on the server"]}"#,
	);
	assert_eq!(
		parsed.operational_constraints,
		vec![
			"we work on the remote server",
			"i will rerun it on the server"
		]
	);
	// Runaway list is capped at the recitation slot's entire budget.
	let many: Vec<String> = (0..9).map(|i| format!("fact {i}")).collect();
	let wire = format!(
		r#"{{"scope":"self_contained","operational_constraints":{}}}"#,
		serde_json::to_string(&many).expect("serializes")
	);
	assert_eq!(parse_classifier(&wire).operational_constraints.len(), 4);
	// Neighboring format — the field as a bare string instead of an array —
	// must not ride the serde default into a silent empty list: the payload
	// is rejected whole and the fail-open path runs.
	assert!(parse_classifier_checked(
		r#"{"scope":"self_contained","operational_constraints":"we work on the remote server"}"#
	)
	.is_none());
	// Absent field stays accepted (a turn that states no facts) — the same
	// contract `conditions` already has.
	assert!(parse_classifier(r#"{"scope":"self_contained"}"#)
		.operational_constraints
		.is_empty());
}

#[test]
fn classifier_answer_only_clears_conditions() {
	let parsed =
		parse_classifier(r#"{"scope":"self_contained","answer_only":true,"conditions":["a","b"]}"#);
	assert!(parsed.answer_only);
	assert!(parsed.conditions.is_empty());
}

#[test]
fn classifier_survives_prose_and_truncation_failures() {
	// No closing brace → truncated JSON → conservative fallback.
	assert!(!parse_classifier(r#"{"scope":"context_dependent""#).context_dependent);
	assert!(!parse_classifier("no braces at all").context_dependent);
	// JSON embedded in prose is still located via first/last brace.
	assert!(
		parse_classifier(r#"Sure! {"scope":"context_dependent"} hope this helps"#)
			.context_dependent
	);
}

#[test]
fn classifier_verdicts_are_case_insensitive() {
	assert!(parse_classifier(r#"{"scope":"CONTEXT_DEPENDENT"}"#).context_dependent);
	let ctx = context("please stop running things");
	let mut upper = parse_classifier(
		r#"{"scope":"self_contained","verification_policy_update":"FORBID","verification_policy_evidence":"please stop"}"#,
	);
	upper.validate_policy_update(&ctx);
	assert_eq!(
		upper.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Forbid
	);
}

#[test]
fn policy_update_needs_short_exact_evidence() {
	let long = "x".repeat(RESOLUTION_EVIDENCE_CHARS + 1);
	let ctx = context(&format!("prefix {long} suffix"));
	let mut overlong = parse_classifier(&format!(
		r#"{{"scope":"self_contained","verification_policy_update":"forbid","verification_policy_evidence":"{long}"}}"#
	));
	overlong.validate_policy_update(&ctx);
	assert_eq!(
		overlong.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Unchanged
	);
	let mut absent = parse_classifier(
		r#"{"scope":"self_contained","verification_policy_update":"allow","verification_policy_evidence":"never said this"}"#,
	);
	absent.validate_policy_update(&ctx);
	assert_eq!(
		absent.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Unchanged
	);
}

#[test]
fn policy_update_legacy_backfill_only_when_unspecified() {
	let mut ctx = context("Make it so");
	ctx.recent_user_policy_context = vec!["never run the deployment".to_string()];
	// A persisted policy exists → legacy context must not override it.
	ctx.verification_policy = crate::supervisor::VerificationPolicy::Allowed;
	let mut ignored = parse_classifier(
		r#"{"scope":"self_contained","verification_policy_update":"forbid","verification_policy_evidence":"never run the deployment"}"#,
	);
	ignored.validate_policy_update(&ctx);
	assert_eq!(
		ignored.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Unchanged
	);
	// Unspecified → the legacy context may support the update.
	ctx.verification_policy = crate::supervisor::VerificationPolicy::Unspecified;
	let mut honored = parse_classifier(
		r#"{"scope":"self_contained","verification_policy_update":"forbid","verification_policy_evidence":"never run the deployment"}"#,
	);
	honored.validate_policy_update(&ctx);
	assert_eq!(
		honored.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Forbid
	);
}

#[test]
fn resolution_empty_rewrite_is_ambiguous() {
	let resolved = parse_resolution(
		&context("Continue"),
		r#"{"scope":"follow_up","resolved_request":"   ","evidence":[{"source":"recent_history","excerpt":"Schedule the status check every two hours"}]}"#,
	);
	assert_eq!(resolved.scope, ResolutionScope::Ambiguous);
	assert_eq!(resolved.resolved_request, "Continue");
}

#[test]
fn resolution_evidence_must_be_short_and_grounded() {
	// Excerpt absent from the claimed source → rejected → ambiguous.
	let absent = parse_resolution(
		&context("Same but hourly"),
		r#"{"scope":"follow_up","resolved_request":"Hourly check","evidence":[{"source":"recent_history","excerpt":"not in history"}]}"#,
	);
	assert_eq!(absent.scope, ResolutionScope::Ambiguous);
	// Over-long excerpt → rejected even when it does appear in the source.
	let long = "S".repeat(RESOLUTION_EVIDENCE_CHARS + 1);
	let mut big = context("Same but hourly");
	big.recent_history = format!("Earlier user: {long}");
	let overlong = parse_resolution(
		&big,
		&format!(
			r#"{{"scope":"follow_up","resolved_request":"Hourly","evidence":[{{"source":"recent_history","excerpt":"{long}"}}]}}"#
		),
	);
	assert_eq!(overlong.scope, ResolutionScope::Ambiguous);
}

#[test]
fn resolution_dedupes_evidence_and_sources() {
	let resolved = parse_resolution(
		&context("Same but hourly"),
		r#"{"scope":"follow_up","resolved_request":"Hourly","evidence":[
			{"source":"recent_history","excerpt":"Schedule the status check every two hours"},
			{"source":"recent_history","excerpt":"Schedule the status check every two hours"},
			{"source":"recent_history","excerpt":"every two hours"}]}"#,
	);
	assert_eq!(resolved.context_sources, ["recent_history"]);
	assert_eq!(resolved.resolution_evidence.len(), 2);
}

#[test]
fn resolution_session_context_grounds_follow_up() {
	let mut ctx = context("Continue with that");
	ctx.session_context = "<intent>Ship the websocket ack</intent>".to_string();
	let resolved = parse_resolution(
		&ctx,
		r#"{"scope":"follow_up","resolved_request":"Continue shipping the websocket ack","evidence":[{"source":"session_context","excerpt":"websocket ack"}]}"#,
	);
	assert_eq!(resolved.scope, ResolutionScope::FollowUp);
	assert_eq!(resolved.context_sources, ["session_context"]);
}

#[test]
fn resolution_plan_relevance_requires_plan_evidence() {
	// Model claims plan relevance but cites no active_plan evidence.
	let claimed = parse_resolution(
		&context("Continue"),
		r#"{"scope":"follow_up","resolved_request":"Continue the check","evidence":[{"source":"recent_history","excerpt":"Schedule the status check every two hours"}],"plan_relevant":true}"#,
	);
	assert!(!claimed.plan_relevant);
	// Cites the plan but reports irrelevance.
	let cited_not_relevant = parse_resolution(
		&context("Continue"),
		r#"{"scope":"follow_up","resolved_request":"Continue the check","evidence":[{"source":"active_plan","excerpt":"active websocket acknowledgement task"}],"plan_relevant":false}"#,
	);
	assert!(!cited_not_relevant.plan_relevant);
	// Cites the plan AND claims relevance.
	let relevant = parse_resolution(
		&context("Continue"),
		r#"{"scope":"follow_up","resolved_request":"Continue the check","evidence":[{"source":"active_plan","excerpt":"active websocket acknowledgement task"}],"plan_relevant":true}"#,
	);
	assert!(relevant.plan_relevant);
}

#[test]
fn resolution_resolved_request_is_bounded() {
	let big = "R".repeat(RESOLVED_REQUEST_CHARS + 10);
	let resolved = parse_resolution(
		&context("Continue"),
		&format!(
			r#"{{"scope":"follow_up","resolved_request":"{big}","evidence":[{{"source":"recent_history","excerpt":"Schedule the status check every two hours"}}]}}"#
		),
	);
	assert_eq!(
		resolved.resolved_request.chars().count(),
		RESOLVED_REQUEST_CHARS + 1
	);
	assert!(resolved.resolved_request.ends_with('…'));
}

#[test]
fn render_resolution_payload_carries_every_context_source() {
	let payload = context("Same but hourly").render_resolution_payload();
	for key in [
		"\"current_user_request\"",
		"\"recent_history\"",
		"\"session_context\"",
		"\"active_plan\"",
		"\"role_context\"",
	] {
		assert!(payload.contains(key), "payload missing {key}");
	}
	assert!(payload.contains("Same but hourly"));
}

#[tokio::test]
async fn resolve_empty_request_short_circuits_without_a_model() {
	let config = crate::session::chat::test_support::fake_provider_config();
	let context = TaskContext {
		current_request: "   ".to_string(),
		recent_history: String::new(),
		session_context: String::new(),
		active_plan: String::new(),
		role_context: String::new(),
		verification_policy: crate::supervisor::VerificationPolicy::Unspecified,
		recent_user_policy_context: Vec::new(),
	};
	let (_tx, operation_rx) = tokio::sync::watch::channel(false);
	let resolved = resolve(&config, &context, operation_rx).await;
	assert_eq!(resolved.scope, ResolutionScope::SelfContained);
	assert_eq!(resolved.resolved_request, "   ");
	assert!(!resolved.forbids_verification);
	assert_eq!(
		resolved.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Unchanged
	);
}

// ---------------------------------------------------------------------------
// parse_resolution: malformed JSON falls back to the literal request.
// ---------------------------------------------------------------------------

#[test]
fn resolution_with_unterminated_or_invalid_json_stays_ambiguous() {
	let ctx = context("do the second part");
	let unterminated = r#"{"scope":"follow_up","resolved_request":"do ""#;
	assert_eq!(
		parse_resolution(&ctx, unterminated).scope,
		ResolutionScope::Ambiguous
	);
	let invalid = "{not valid json}";
	assert_eq!(
		parse_resolution(&ctx, invalid).scope,
		ResolutionScope::Ambiguous
	);
}

#[test]
fn policy_update_unchanged_never_requires_evidence() {
	let mut verdict = ClassifierVerdict {
		context_dependent: false,
		forbids_verification: false,
		verification_policy_update: crate::supervisor::VerificationPolicyUpdate::Unchanged,
		verification_policy_evidence: "fabricated quote that appears nowhere".to_string(),
		answer_only: false,
		conditions: Vec::new(),
		operational_constraints: Vec::new(),
	};
	let ctx = context("a plain request");
	verdict.validate_policy_update(&ctx);
	assert_eq!(
		verdict.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Unchanged,
		"an unchanged policy never inspects evidence"
	);
}

// ---------------------------------------------------------------------------
// resolve(): the full classifier → resolver round trip against the fake
// provider. Every failure mode must fall back to a usable literal request.
// ---------------------------------------------------------------------------

fn resolve_context(request: &str) -> TaskContext {
	TaskContext {
		current_request: request.to_string(),
		recent_history: "USER]: earlier: use the staging endpoint".to_string(),
		session_context: String::new(),
		active_plan: String::new(),
		role_context: String::new(),
		verification_policy: crate::supervisor::VerificationPolicy::Unspecified,
		recent_user_policy_context: Vec::new(),
	}
}

fn resolve_config() -> crate::config::Config {
	let mut config = crate::session::chat::test_support::fake_provider_config();
	config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();
	config
}

const SELF_CONTAINED: &str = r#"{"scope":"self_contained","forbids_verification":true,"verification_policy_update":"unchanged","verification_policy_evidence":"","answer_only":false,"conditions":["the staging endpoint is used"],"operational_constraints":[]}"#;

#[tokio::test]
async fn a_self_contained_classification_returns_with_the_full_verdict() {
	use crate::session::chat::test_support::{final_response, spawn_stub, ENV_LOCK};
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![final_response(SELF_CONTAINED)]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let resolved = resolve(
		&resolve_config(),
		&resolve_context("use the staging endpoint"),
		rx,
	)
	.await;
	std::env::remove_var("OLLAMA_API_URL");

	assert_eq!(resolved.scope, ResolutionScope::SelfContained);
	assert!(
		resolved.forbids_verification,
		"the classifier verdict is carried"
	);
	assert_eq!(
		resolved.evidence_conditions,
		vec!["the staging endpoint is used".to_string()]
	);
}

#[tokio::test]
async fn a_context_dependent_turn_resolves_through_the_follow_up_model() {
	use crate::session::chat::test_support::{final_response, spawn_stub, ENV_LOCK};
	let _guard = ENV_LOCK.lock().await;
	let classifier = r#"{"scope":"context_dependent","forbids_verification":false,"verification_policy_update":"unchanged","verification_policy_evidence":"","answer_only":false,"conditions":["the staging endpoint is used"],"operational_constraints":[]}"#;
	let follow_up = r#"{"scope":"follow_up","resolved_request":"Use the staging endpoint for the load test","evidence":[{"source":"recent_history","excerpt":"use the staging endpoint"}],"plan_relevant":false}"#;
	let url = spawn_stub(vec![final_response(classifier), final_response(follow_up)]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let resolved = resolve(&resolve_config(), &resolve_context("and run it again"), rx).await;
	std::env::remove_var("OLLAMA_API_URL");

	assert_eq!(resolved.scope, ResolutionScope::FollowUp);
	assert_eq!(
		resolved.resolved_request,
		"Use the staging endpoint for the load test"
	);
	assert_eq!(
		resolved.evidence_conditions,
		vec!["the staging endpoint is used".to_string()],
		"the checklist compiled from the literal turn survives the rewrite"
	);
}

#[tokio::test]
async fn an_unusable_classifier_answer_gets_one_doubled_budget_retry() {
	use crate::session::chat::test_support::{final_response, spawn_stub, ENV_LOCK};
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response("certainly! no json here"),
		final_response(SELF_CONTAINED),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let resolved = resolve(
		&resolve_config(),
		&resolve_context("use the staging endpoint"),
		rx,
	)
	.await;
	std::env::remove_var("OLLAMA_API_URL");

	assert_eq!(resolved.scope, ResolutionScope::SelfContained);
	assert!(
		resolved.forbids_verification,
		"the retried verdict is applied"
	);
}

#[tokio::test]
async fn a_failed_classifier_retry_fails_open_to_the_literal_request() {
	use crate::session::chat::test_support::{final_response, spawn_stub_with_status, ENV_LOCK};
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub_with_status(vec![
		(200, final_response("no json at all")),
		(500, serde_json::json!({"error": "classifier down"})),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let resolved = resolve(
		&resolve_config(),
		&resolve_context("use the staging endpoint"),
		rx,
	)
	.await;
	std::env::remove_var("OLLAMA_API_URL");

	assert_eq!(resolved.scope, ResolutionScope::SelfContained);
	assert_eq!(resolved.resolved_request, "use the staging endpoint");
	assert!(
		!resolved.forbids_verification,
		"the fallback verdict is neutral"
	);
}

#[tokio::test]
async fn a_failed_classifier_call_uses_the_literal_request() {
	use crate::session::chat::test_support::{spawn_stub_with_status, ENV_LOCK};
	let _guard = ENV_LOCK.lock().await;
	let url =
		spawn_stub_with_status(vec![(500, serde_json::json!({"error": "classifier down"}))]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let resolved = resolve(
		&resolve_config(),
		&resolve_context("use the staging endpoint"),
		rx,
	)
	.await;
	std::env::remove_var("OLLAMA_API_URL");

	assert_eq!(resolved.scope, ResolutionScope::SelfContained);
	assert_eq!(resolved.resolved_request, "use the staging endpoint");
}

#[tokio::test]
async fn a_failed_follow_up_resolver_preserves_ambiguity() {
	use crate::session::chat::test_support::{final_response, spawn_stub_with_status, ENV_LOCK};
	let _guard = ENV_LOCK.lock().await;
	let classifier = r#"{"scope":"context_dependent","forbids_verification":false,"verification_policy_update":"unchanged","verification_policy_evidence":"","answer_only":false,"conditions":[],"operational_constraints":[]}"#;
	let url = spawn_stub_with_status(vec![
		(200, final_response(classifier)),
		(500, serde_json::json!({"error": "resolver down"})),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let resolved = resolve(&resolve_config(), &resolve_context("and run it again"), rx).await;
	std::env::remove_var("OLLAMA_API_URL");

	assert_eq!(resolved.scope, ResolutionScope::Ambiguous);
	assert_eq!(resolved.resolved_request, "and run it again");
}

#[tokio::test]
async fn a_still_unusable_retry_fails_open() {
	use crate::session::chat::test_support::{final_response, spawn_stub, ENV_LOCK};
	let _guard = ENV_LOCK.lock().await;
	let url = spawn_stub(vec![
		final_response("first garbage"),
		final_response("second garbage"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let resolved = resolve(
		&resolve_config(),
		&resolve_context("use the staging endpoint"),
		rx,
	)
	.await;
	std::env::remove_var("OLLAMA_API_URL");

	assert_eq!(resolved.scope, ResolutionScope::SelfContained);
	assert_eq!(resolved.resolved_request, "use the staging endpoint");
}
