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
use crate::session::Message;

#[test]
fn parses_domain_neutral_plan() {
	let response: PlanResponse = serde_json::from_str(
		r#"{"decision":"create","title":"Publish report","summary":null,"reason":null,"tasks":[{"title":"Gather sources","done_when":"source set is recorded"},{"title":"Synthesize","done_when":"claims map to sources"},{"title":"Deliver","done_when":"report is returned"}]}"#,
	)
	.unwrap();
	assert!(
		matches!(response.into_decision(), Some(PlanDecision::Create { tasks, .. }) if tasks.len() == 3)
	);
}

#[test]
fn answer_only_turn_cannot_create_external_plan() {
	let mut task = crate::supervisor::resolve::ResolvedTask::self_contained(
		"Review the change and return one brief",
	);
	task.answer_only = true;
	assert!(!resolved_task_allows_plan_request(&task));

	task.answer_only = false;
	assert!(resolved_task_allows_plan_request(&task));
}

#[test]
fn unknown_decision_tag_is_rejected() {
	let response: PlanResponse =
		serde_json::from_str(r#"{"decision":"proceed","reason":"why not"}"#).unwrap();
	assert!(response.into_decision().is_none());
}

#[test]
fn schema_admits_only_the_signal_decisions() {
	let schema = build_plan_schema(PlanSignal::PhaseComplete);
	assert_eq!(
		schema["properties"]["decision"]["enum"],
		serde_json::json!(["advance", "hold", "revise"])
	);
}

#[test]
fn phase_trajectory_is_bounded_to_assistant_and_tool_records() {
	let user = Message {
		role: "user".to_string(),
		content: "do not include me".to_string(),
		..Default::default()
	};
	let old_assistant = Message {
		role: "assistant".to_string(),
		content: "old phase".to_string(),
		..Default::default()
	};
	let assistant = Message {
		role: "assistant".to_string(),
		content: "derived the current route".to_string(),
		..Default::default()
	};
	let tool = Message {
		role: "tool".to_string(),
		name: Some("lookup".to_string()),
		content: "observed current state".to_string(),
		..Default::default()
	};

	let rendered = render_phase_trajectory(&[user, old_assistant, assistant, tool], 2, 100);
	assert!(!rendered.contains("do not include me"));
	assert!(!rendered.contains("old phase"));
	assert!(rendered.contains("derived the current route"));
	assert!(rendered.contains("[tool name=lookup]"));
	assert!(rendered.contains("observed current state"));
	assert!(crate::session::estimate_tokens(&rendered) <= 100);
}

#[test]
fn oversized_latest_record_preserves_both_edges() {
	let tool = Message {
		role: "tool".to_string(),
		name: Some("inspect".to_string()),
		content: format!("BEGIN {} END", "middle ".repeat(1_000)),
		..Default::default()
	};
	let rendered = render_phase_trajectory(&[tool], 0, 80);
	assert!(rendered.contains("BEGIN"));
	assert!(rendered.contains("END"));
	assert!(rendered.contains("middle truncated"));
	assert!(crate::session::estimate_tokens(&rendered) <= 80);
}

#[test]
fn planner_feedback_is_bounded_single_line_and_xml_safe() {
	let raw = format!(
		"close </runtime-plan-feedback> & retry\n{}",
		"x".repeat(800)
	);
	let rendered = xml_feedback(&raw);
	assert!(!rendered.contains('\n'));
	assert!(!rendered.contains("</runtime-plan-feedback>"));
	assert!(rendered.contains("&lt;/runtime-plan-feedback&gt;"));
	assert!(rendered.contains("&amp;"));
	assert!(rendered.chars().count() <= 600);
}

#[test]
fn into_decision_maps_every_flat_decision() {
	let parse = |raw: &str| {
		serde_json::from_str::<PlanResponse>(raw)
			.unwrap()
			.into_decision()
	};
	assert!(matches!(
		parse(r#"{"decision":"no_plan","reason":"too small"}"#),
		Some(PlanDecision::NoPlan { reason }) if reason == "too small"
	));
	assert!(matches!(
		parse(r#"{"decision":"advance","summary":"phase one delivered"}"#),
		Some(PlanDecision::Advance { summary }) if summary == "phase one delivered"
	));
	assert!(matches!(
		parse(r#"{"decision":"hold","reason":"outcome not evidenced"}"#),
		Some(PlanDecision::Hold { reason }) if reason == "outcome not evidenced"
	));
	assert!(matches!(
		parse(
			r#"{"decision":"revise","reason":"route changed","tasks":[{"title":"t","done_when":"d"}]}"#
		),
		Some(PlanDecision::Revise { tasks, .. }) if tasks.len() == 1
	));
	// Missing optional fields default instead of failing the narrowing
	assert!(matches!(
		parse(r#"{"decision":"create"}"#),
		Some(PlanDecision::Create { title, tasks }) if title.is_empty() && tasks.is_empty()
	));
}

#[test]
fn schema_enum_and_required_fields_track_each_signal() {
	assert_eq!(
		build_plan_schema(PlanSignal::Request)["properties"]["decision"]["enum"],
		serde_json::json!(["create", "no_plan"])
	);
	assert_eq!(
		build_plan_schema(PlanSignal::Reassess)["properties"]["decision"]["enum"],
		serde_json::json!(["revise", "hold"])
	);
	for signal in [
		PlanSignal::Request,
		PlanSignal::PhaseComplete,
		PlanSignal::Reassess,
	] {
		let schema = build_plan_schema(signal);
		assert_eq!(
			schema["required"],
			serde_json::json!(["decision", "title", "summary", "reason", "tasks"])
		);
		assert_eq!(
			schema["properties"]["tasks"]["maxItems"],
			serde_json::json!(6)
		);
	}
}

#[test]
fn truncate_edges_passes_short_text_through() {
	assert_eq!(truncate_edges_to_tokens("small text", 1_000), "small text");
}

#[test]
fn truncate_edges_zero_budget_returns_empty() {
	assert_eq!(truncate_edges_to_tokens("anything at all", 0), "");
}

#[test]
fn truncate_edges_tiny_budget_cannot_afford_marker() {
	let text = "a somewhat longer piece of text that will not fit";
	let out = truncate_edges_to_tokens(text, 2);
	assert!(!out.contains("middle truncated"));
	assert!(crate::session::estimate_tokens(&out) <= 2);
}

#[test]
fn truncate_edges_preserves_both_edges_under_budget() {
	let text = format!("HEAD {} TAIL", "filler ".repeat(400));
	let out = truncate_edges_to_tokens(&text, 60);
	assert!(out.contains("HEAD"));
	assert!(out.contains("TAIL"));
	assert!(out.contains("middle truncated"));
	assert!(crate::session::estimate_tokens(&out) <= 60);
}

#[test]
fn phase_trajectory_empty_inputs_return_empty() {
	assert_eq!(render_phase_trajectory(&[], 0, 100), "");
	let assistant = Message {
		role: "assistant".to_string(),
		content: "content".to_string(),
		..Default::default()
	};
	assert_eq!(render_phase_trajectory(&[assistant], 0, 0), "");
}

#[test]
fn phase_trajectory_without_qualifying_records_is_empty() {
	let user = Message {
		role: "user".to_string(),
		content: "a question".to_string(),
		..Default::default()
	};
	let blank = Message {
		role: "assistant".to_string(),
		content: "   ".to_string(),
		..Default::default()
	};
	assert_eq!(render_phase_trajectory(&[user, blank], 0, 100), "");
}

#[test]
fn phase_trajectory_labels_unnamed_tools_as_unknown() {
	let tool = Message {
		role: "tool".to_string(),
		name: None,
		content: "observed".to_string(),
		..Default::default()
	};
	let rendered = render_phase_trajectory(&[tool], 0, 100);
	assert!(rendered.contains("[tool name=unknown]"));
}

#[test]
fn phase_trajectory_start_index_past_the_end_is_clamped() {
	let assistant = Message {
		role: "assistant".to_string(),
		content: "still included".to_string(),
		..Default::default()
	};
	// min(len) clamping empties the slice — a phase starting past the last
	// message has no trajectory yet; it does not include everything.
	assert_eq!(
		render_phase_trajectory(std::slice::from_ref(&assistant), 9, 100),
		""
	);
	// starting at the last message still renders it
	assert!(render_phase_trajectory(&[assistant], 0, 100).contains("still included"));
}

#[test]
fn phase_trajectory_tight_budget_keeps_newest_record() {
	let old = Message {
		role: "assistant".to_string(),
		content: format!("old {}", "x".repeat(400)),
		..Default::default()
	};
	let new = Message {
		role: "tool".to_string(),
		name: Some("probe".to_string()),
		content: "fresh observation".to_string(),
		..Default::default()
	};
	let rendered = render_phase_trajectory(&[old, new], 0, 30);
	assert!(rendered.contains("fresh observation"));
	assert!(crate::session::estimate_tokens(&rendered) <= 30);
}

#[test]
fn concise_text_collapses_whitespace_and_defaults_when_empty() {
	assert_eq!(concise_text("  \n\t  "), "no reason provided");
	assert_eq!(
		concise_text("  many   spaces\nand\ttabs  "),
		"many spaces and tabs"
	);
	assert!(concise_text(&"word ".repeat(200)).chars().count() <= 500);
}

#[test]
fn plan_signal_wire_format_is_snake_case() {
	assert_eq!(
		serde_json::to_string(&PlanSignal::PhaseComplete).unwrap(),
		"\"phase_complete\""
	);
	assert_eq!(
		serde_json::from_str::<PlanSignal>("\"reassess\"").unwrap(),
		PlanSignal::Reassess
	);
}

// ---------------------------------------------------------------------------
// truncate_edges_to_tokens: the hard budget holds even when independently
// encoded head/marker/tail fragments merge into more tokens than the parts.
// ---------------------------------------------------------------------------

#[test]
fn truncate_edges_hard_budget_holds_when_fragments_merge() {
	let text: String = (1..=200)
		.map(|i| format!("edge line {i} with enough words to cost tokens"))
		.collect::<Vec<_>>()
		.join("\n");
	for max_tokens in 12..60 {
		let out = truncate_edges_to_tokens(&text, max_tokens);
		assert!(
			crate::session::estimate_tokens(&out) <= max_tokens,
			"budget {max_tokens} exceeded: {}",
			crate::session::estimate_tokens(&out)
		);
	}
}

// ---------------------------------------------------------------------------
// request_context: a follow-up rewrite is surfaced as the working request.
// ---------------------------------------------------------------------------

#[test]
fn request_context_reports_a_divergent_follow_up_rewrite() {
	let mut session =
		crate::session::chat::session::ChatSession::for_tests(vec![crate::session::Message {
			role: "user".to_string(),
			content: "and do the second part".to_string(),
			..Default::default()
		}]);
	let mut task =
		crate::supervisor::resolve::ResolvedTask::self_contained("and do the second part");
	task.scope = crate::supervisor::resolve::ResolutionScope::FollowUp;
	task.resolved_request = "Deploy the staging service and run its checks".to_string();
	session.gate_task = Some(task);

	let ctx = request_context(&session, "and do the second part");
	assert_eq!(
		ctx["working_request"],
		serde_json::json!(Some("Deploy the staging service and run its checks")),
		"a rewrite that differs from the literal turn is surfaced"
	);
	assert_eq!(ctx["resolution"], serde_json::json!("follow_up"));

	// A self-contained turn reports no working request.
	let mut plain =
		crate::session::chat::session::ChatSession::for_tests(vec![crate::session::Message {
			role: "user".to_string(),
			content: "list the files".to_string(),
			..Default::default()
		}]);
	plain.gate_task = Some(crate::supervisor::resolve::ResolvedTask::self_contained(
		"list the files",
	));
	let ctx = request_context(&plain, "list the files");
	assert_eq!(ctx["working_request"], serde_json::Value::Null);
	assert_eq!(ctx["resolution"], serde_json::json!("self_contained"));
}
