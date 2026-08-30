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
fn self_contained_classification_never_receives_historical_requirements() {
	for request in [
		"Schedule Cointrapper checks every two hours",
		"Check Cointrapper now and schedule checks every two hours",
		"Write a README",
	] {
		let context = TaskContext {
			current_request: request.to_string(),
			recent_history: "Older request: check immediately".to_string(),
			session_context: "Older session goal".to_string(),
			active_plan: "Older checklist".to_string(),
			role_context: String::new(),
			verification_policy: crate::supervisor::VerificationPolicy::Unspecified,
			recent_user_policy_context: Vec::new(),
		};
		let payload = context.render_classification_payload();
		assert!(payload.contains(request));
		assert!(!payload.contains("Older request"));
		assert!(!payload.contains("Older session goal"));
		assert!(!payload.contains("Older checklist"));
		assert!(!parse_classifier(r#"{"scope":"self_contained"}"#).context_dependent);
	}
}

#[test]
fn scheduling_follow_up_resolves_subject_without_importing_immediate_action() {
	let context = TaskContext {
		current_request: "check periodically like every 2h and report status and how is it going"
			.to_string(),
		recent_history: "Earlier user: Check live Cointrapper now\n".to_string(),
		session_context: String::new(),
		active_plan: String::new(),
		role_context: String::new(),
		verification_policy: crate::supervisor::VerificationPolicy::Unspecified,
		recent_user_policy_context: Vec::new(),
	};
	let resolved = parse_resolution(
		&context,
		r#"{"scope":"follow_up","resolved_request":"Schedule a live Cointrapper check every 2h that reports status and how it is going","evidence":[{"source":"recent_history","excerpt":"live Cointrapper"}],"plan_relevant":false}"#,
	);
	assert_eq!(resolved.scope, ResolutionScope::FollowUp);
	assert!(resolved.resolved_request.contains("every 2h"));
	assert!(!resolved.resolved_request.contains("now"));

	let explicit_now = TaskContext {
		current_request: "Check now and schedule every two hours.".to_string(),
		recent_history: "Earlier user: Monitor live Cointrapper\n".to_string(),
		session_context: String::new(),
		active_plan: String::new(),
		role_context: String::new(),
		verification_policy: crate::supervisor::VerificationPolicy::Unspecified,
		recent_user_policy_context: Vec::new(),
	};
	let resolved_now = parse_resolution(
		&explicit_now,
		r#"{"scope":"follow_up","resolved_request":"Check live Cointrapper now and schedule a live Cointrapper check every two hours","evidence":[{"source":"recent_history","excerpt":"live Cointrapper"}],"plan_relevant":false}"#,
	);
	assert!(resolved_now.resolved_request.contains("now"));
	assert!(resolved_now.resolved_request.contains("every two hours"));
}

#[test]
fn follow_up_uses_minimal_rewrite_and_known_sources() {
	let same = context("Same but hourly");
	let resolved = parse_resolution(
		&same,
		r#"{"scope":"follow_up","resolved_request":"Schedule the status check hourly","evidence":[{"source":"recent_history","excerpt":"Schedule the status check every two hours"},{"source":"invented","excerpt":"unsupported"}]}"#,
	);
	assert_eq!(resolved.scope, ResolutionScope::FollowUp);
	assert_eq!(
		resolved.resolved_request,
		"Schedule the status check hourly"
	);
	assert_eq!(resolved.context_sources, ["recent_history"]);
	assert_eq!(resolved.resolution_evidence.len(), 1);
	assert_eq!(resolved.resolution_evidence[0].source, "recent_history");

	let continued_context = context("Continue");
	let continued = parse_resolution(
		&continued_context,
		r#"{"scope":"follow_up","resolved_request":"Continue implementing the active websocket acknowledgement task","evidence":[{"source":"active_plan","excerpt":"active websocket acknowledgement task"}],"plan_relevant":true}"#,
	);
	assert_eq!(continued.scope, ResolutionScope::FollowUp);
	assert_eq!(continued.context_sources, ["active_plan"]);
	assert!(continued.plan_relevant);
}

#[test]
fn ambiguous_or_malformed_resolution_falls_back_to_literal_request() {
	let do_that = context("Do that");
	let ambiguous = parse_resolution(
		&do_that,
		r#"{"scope":"ambiguous","resolved_request":"Delete it","evidence":[{"source":"recent_history","excerpt":"Schedule the status check"}]}"#,
	);
	assert_eq!(ambiguous.scope, ResolutionScope::Ambiguous);
	assert_eq!(ambiguous.resolved_request, "Do that");
	assert!(ambiguous.context_sources.is_empty());

	let readme = context("Write a README");
	let malformed = parse_resolution(&readme, "not json");
	assert_eq!(malformed.scope, ResolutionScope::Ambiguous);
	assert_eq!(malformed.resolved_request, "Write a README");

	let unknown = parse_resolution(
		&readme,
		r#"{"scope":"related","resolved_request":"Finish old work","plan_relevant":true}"#,
	);
	assert_eq!(unknown.resolved_request, "Write a README");
	assert_eq!(unknown.scope, ResolutionScope::Ambiguous);
	assert!(!unknown.plan_relevant);
}

#[test]
fn follow_up_grounded_in_role_context_is_accepted() {
	// The prompt lists role_context as a legal evidence source; a rewrite
	// grounded solely in it must resolve, not degrade to ambiguous.
	let mut ctx = context("Run the scheduled check");
	ctx.role_context = "You are the monitoring agent for Cointrapper status checks".to_string();
	let resolved = parse_resolution(
		&ctx,
		r#"{"scope":"follow_up","resolved_request":"Run the Cointrapper status check","evidence":[{"source":"role_context","excerpt":"Cointrapper status checks"}]}"#,
	);
	assert_eq!(resolved.scope, ResolutionScope::FollowUp);
	assert_eq!(resolved.context_sources, ["role_context"]);
	assert_eq!(resolved.resolution_evidence.len(), 1);
}

#[test]
fn unsupported_follow_up_rewrite_is_rejected_as_ambiguous() {
	let context = context("Continue");
	let invented = parse_resolution(
		&context,
		r#"{"scope":"follow_up","resolved_request":"Delete the production database","evidence":[{"source":"active_plan","excerpt":"Delete the production database"}],"plan_relevant":true}"#,
	);
	assert_eq!(invented.scope, ResolutionScope::Ambiguous);
	assert_eq!(invented.resolved_request, "Continue");
	assert!(invented.context_sources.is_empty());
	assert!(!invented.plan_relevant);
}

#[test]
fn only_explicit_context_dependency_unlocks_follow_up_resolution() {
	assert!(parse_classifier(r#"{"scope":"context_dependent"}"#).context_dependent);
	for response in [
		r#"{"scope":"self_contained"}"#,
		r#"{"scope":"related"}"#,
		"not json",
	] {
		assert!(!parse_classifier(response).context_dependent);
	}
}

#[test]
fn capture_keeps_recent_real_turns_and_excludes_injections() {
	let messages = vec![
		message("user", "Old task"),
		message("assistant", "Old result"),
		message("user", "<pay-attention>synthetic</pay-attention>"),
		message("user", "Schedule status checks"),
	];
	let captured = TaskContext::capture(
		&messages,
		"durable goal",
		Some("live plan"),
		crate::supervisor::VerificationPolicy::Unspecified,
	)
	.expect("current real turn");
	assert_eq!(captured.current_request, "Schedule status checks");
	assert!(captured.recent_history.contains("Old task"));
	assert!(captured.recent_history.contains("Old result"));
	assert!(!captured.recent_history.contains("synthetic"));
	assert_eq!(captured.session_context, "durable goal");
	assert_eq!(captured.active_plan, "live plan");
	assert_eq!(captured.recent_user_policy_context, ["Old task"]);
}

#[test]
fn legacy_policy_backfill_sees_answer_only_user_rule_not_synthetic_text() {
	let messages = vec![
		message("user", "No build or tests; I will test it myself."),
		message("assistant", "Understood."),
		message("user", "<pay-attention>always run tests</pay-attention>"),
		message("user", "Make Ctrl-D exit the picker."),
	];
	let captured = TaskContext::capture(
		&messages,
		"",
		None,
		crate::supervisor::VerificationPolicy::Unspecified,
	)
	.expect("current real turn");
	assert_eq!(
		captured.recent_user_policy_context,
		["No build or tests; I will test it myself."]
	);
	let payload = captured.render_classification_payload();
	assert!(payload.contains("Make Ctrl-D exit the picker"));
	assert!(payload.contains("I will test it myself"));
	assert!(!payload.contains("always run tests"));

	let mut backfill = parse_classifier(
		r#"{"scope":"self_contained","verification_policy_update":"forbid","verification_policy_evidence":"I will test it myself"}"#,
	);
	backfill.validate_policy_update(&captured);
	assert_eq!(
		backfill.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Forbid
	);
	let mut synthetic = parse_classifier(
		r#"{"scope":"self_contained","verification_policy_update":"allow","verification_policy_evidence":"always run tests"}"#,
	);
	synthetic.validate_policy_update(&captured);
	assert_eq!(
		synthetic.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Unchanged
	);
}

#[test]
fn legacy_policy_backfill_keeps_constraints_at_the_end_of_long_turns() {
	let content = format!("{} DO NOT RUN TESTS", "context ".repeat(300));
	let bounded = truncate_head_tail(&content, POLICY_HISTORY_ITEM_CHARS);
	assert!(bounded.starts_with("context"));
	assert!(bounded.ends_with("DO NOT RUN TESTS"));
	assert!(bounded.chars().count() <= POLICY_HISTORY_ITEM_CHARS + 3);
}

#[test]
fn classifier_parses_verification_policy_delta() {
	let forbid_context = context("Do not run tests; I will test it myself.");
	let mut forbidden = parse_classifier(
		r#"{"scope":"self_contained","forbids_verification":true,"verification_policy_update":"forbid","verification_policy_evidence":"I will test it myself"}"#,
	);
	forbidden.validate_policy_update(&forbid_context);
	assert!(forbidden.forbids_verification);
	assert_eq!(
		forbidden.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Forbid
	);

	let allow_context = context("Go ahead and run the tests now.");
	let mut allowed = parse_classifier(
		r#"{"scope":"self_contained","verification_policy_update":"allow","verification_policy_evidence":"run the tests now"}"#,
	);
	allowed.validate_policy_update(&allow_context);
	assert_eq!(
		allowed.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Allow
	);
	assert_eq!(
		parse_classifier(r#"{"scope":"self_contained"}"#).verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Unchanged
	);

	let mut unsupported = parse_classifier(
		r#"{"scope":"self_contained","verification_policy_update":"forbid","verification_policy_evidence":"invented instruction"}"#,
	);
	unsupported.validate_policy_update(&forbid_context);
	assert_eq!(
		unsupported.verification_policy_update,
		crate::supervisor::VerificationPolicyUpdate::Unchanged
	);
}

#[test]
fn new_unrelated_request_keeps_old_goal_out_of_classification() {
	let messages = vec![
		message("user", "Implement the old websocket goal"),
		message("assistant", "Work remains"),
		message("user", "Write a release note for the new CLI flag"),
	];
	let captured = TaskContext::capture(
		&messages,
		"<intent>Implement the old websocket goal</intent>",
		Some("Old websocket checklist"),
		crate::supervisor::VerificationPolicy::Allowed,
	)
	.expect("current real turn");
	let classification = captured.render_classification_payload();
	assert!(classification.contains("Write a release note"));
	assert!(!classification.contains("websocket"));
}

#[test]
fn unrelated_old_plan_does_not_apply_but_relevant_or_changed_plan_does() {
	let mut task = ResolvedTask::self_contained("Write a README");
	task.plan_at_turn_start = "Old trading plan".to_string();
	assert!(!plan_applies(&task, "Old trading plan"));

	task.plan_relevant = true;
	assert!(plan_applies(&task, "Old trading plan"));

	task.plan_relevant = false;
	assert!(plan_applies(&task, "New README plan"));
	assert!(!plan_applies(&task, ""));
}

#[test]
fn answer_only_turn_ignores_preexisting_plan_but_not_plan_changed_this_turn() {
	// A side question during a long-running plan: the resolver may mark the
	// plan relevant (it supplies referents), but an answer-only turn is
	// complete once answered — the open checklist must not block it.
	let mut task = ResolvedTask::self_contained("Is pricing computed per token?");
	task.plan_at_turn_start = "Benchmark plan (2 open)".to_string();
	task.plan_relevant = true;
	task.answer_only = true;
	assert!(!plan_applies(&task, "Benchmark plan (2 open)"));

	// Deterministic act signal outranks classification: a plan created or
	// changed by the turn itself applies even under an answer-only misread.
	assert!(plan_applies(&task, "Benchmark plan (changed)"));

	// Without the answer-only verdict the relevant plan still applies.
	task.answer_only = false;
	assert!(plan_applies(&task, "Benchmark plan (2 open)"));
}

#[test]
fn classifier_parses_answer_only_and_defaults_to_false() {
	let parsed = parse_classifier(
		r#"{"scope":"context_dependent","forbids_verification":false,"answer_only":true}"#,
	);
	assert!(parsed.context_dependent);
	assert!(parsed.answer_only);

	// Absent field, malformed JSON, and non-JSON all keep every gate armed.
	assert!(!parse_classifier(r#"{"scope":"self_contained"}"#).answer_only);
	assert!(!parse_classifier("not json").answer_only);
}
