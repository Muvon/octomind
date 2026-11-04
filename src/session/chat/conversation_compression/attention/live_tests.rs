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

use super::super::tests::{message, pact_with};
use super::*;

fn view(summary: &str) -> String {
	project(summary).unwrap_or_else(|| summary.to_string())
}

#[test]
fn live_view_is_bounded_over_repeated_folds_while_every_unit_persists() {
	let mut call = message("assistant", "");
	call.tool_calls = Some(serde_json::json!([{"id":"read","name":"view","arguments":{"path":"artifact"}}]));
	let mut result = message(
		"tool",
		&(1..=80)
			.map(|n| format!("{n}:ab|needed material {n}"))
			.collect::<Vec<_>>()
			.join("\n"),
	);
	result.tool_call_id = Some("read".into());
	let mut messages = vec![call, result];
	let facts: String = (1..=30)
		.map(|n| format!("<unit id=\"f{n}\" kind=\"observation\" status=\"established\" refs=\"b:read\">Established fact number {n} about the artifact.</unit>"))
		.collect();
	let body = format!("<folded_state>{facts}<unit id=\"next\" kind=\"next_action\" status=\"pending\" refs=\"b:read\">Continue the open step.</unit></folded_state>");
	for _ in 0..20 {
		let mut packets = build_packets("bounded", &messages, "test");
		link_dependencies(&mut packets);
		let mut pact = pact_with(packets[0].clone());
		pact.live_tokens = budget(32_000);
		pact.frontier_tokens = frontier_budget(pact.live_tokens, &pact.pinned);
		allocate_frontier(
			&mut packets,
			&messages,
			pact.frontier_tokens,
			false,
			"needed material",
			Some(&pact.pinned),
		);
		pact.packets = packets;
		let summary = render(&pact, &body, "", "fold", None, Some(&pact.pinned.task.text));
		assert_eq!(summary.matches("<unit ").count(), 31, "every unit persists");
		assert!(summary.contains("id=\"f30\""), "persisted units keep their ids");
		let view = view(&summary);
		let size = crate::session::estimate_tokens(&view);
		assert!(size <= pact.live_tokens, "{size} > {}", pact.live_tokens);
		assert!(view.contains("needed material"));
		assert!(view.contains("Continue the open step."));
		assert!(
			view.matches("<unit ").count() < 31,
			"the view evicts, the record does not"
		);
		assert!(!view.contains("<unit id="));
		let mut prior = message("assistant", &summary);
		prior.name = Some(super::super::super::apply::COMPRESSION_MESSAGE_NAME.into());
		messages = vec![prior];
	}
}

#[test]
fn duplicate_facts_and_navigation_do_not_regrow_in_live_state() {
	let mut pact = pact_with(super::super::tests::packet(
		"b:read",
		Provenance::ToolObserved,
		Lane::KeepExact,
	));
	pact.live_tokens = 1000;
	pact.frontier_tokens = 200;
	let unit = "<unit id=\"old\" kind=\"next_action\" status=\"pending\" refs=\"b:read\">Continue once.</unit>";
	let summary = render(
		&pact,
		&format!("<folded_state>{unit}{unit}{unit}</folded_state>"),
		"",
		"fold",
		None,
		Some(&pact.pinned.task.text),
	);
	assert_eq!(summary.matches("Continue once.").count(), 1);
	assert!(!view(&summary).contains("id=\"old\""));
	assert!(crate::session::estimate_tokens(&view(&summary)) <= 1000);
}

#[test]
fn persistence_deduplicates_without_changing_ranked_metadata_or_source_order() {
	let observation =
		"<unit id=\"old\" kind=\"observation\" status=\"established\" refs=\"b:old\">Continue once.</unit>";
	let action = "<unit id=\"next\" kind=\"next_action\" status=\"pending\" refs=\"b:new\">Continue once.</unit>";
	let other_action =
		"<unit id=\"other\" kind=\"next_action\" status=\"pending\" refs=\"b:other\">Verify separately.</unit>";
	let fact = "<unit id=\"fact\" kind=\"observation\" status=\"established\" refs=\"b:fact\">A separate fact.</unit>";
	let body = format!("{observation}{action}{fact}{other_action}{action}");
	let persisted = persisted_units(&body, "");
	assert_eq!(persisted, format!("{fact}\n{other_action}\n{action}\n"));
	assert_eq!(persisted_units(&persisted, ""), persisted);
	assert_eq!(
		projected_units(&persisted, "", usize::MAX),
		projected_units(&body, "", usize::MAX)
	);
	assert_eq!(
		persisted_units(&body, "Continue once."),
		format!("{fact}\n{other_action}\n")
	);
}

#[test]
fn projection_evicts_observations_before_decisions_and_is_stable_across_requests() {
	let mut pact = pact_with(super::super::tests::packet(
		"b:read",
		Provenance::ToolObserved,
		Lane::KeepExact,
	));
	pact.live_tokens = 260;
	pact.frontier_tokens = 0;
	let unit = |id: &str, kind: &str, text: &str| {
		format!("<unit id=\"{id}\" kind=\"{kind}\" status=\"established\" refs=\"b:read\">{text}</unit>")
	};
	let body = format!(
		"<folded_state>{}{}{}</folded_state>",
		unit(
			"o1",
			"observation",
			"The parser module has twelve public functions and three private helpers."
		),
		unit(
			"d1",
			"decision",
			"Use the streaming parser; the batch one was rejected for memory."
		),
		unit(
			"o2",
			"observation",
			"The test suite takes about forty seconds on the development machine."
		)
	);
	let summary = render(&pact, &body, "", "fold", None, Some(&pact.pinned.task.text));
	assert_eq!(summary.matches("<unit ").count(), 3);
	let first = view(&summary);
	assert!(first.contains("streaming parser"), "{first}");
	assert!(first.matches("<unit ").count() < 3, "{first}");
	assert_eq!(view(&summary), first, "byte-identical across requests");
}

#[test]
fn legacy_summaries_without_a_projection_budget_are_sent_as_stored() {
	assert_eq!(
		project("<conversation_summary id=\"old\" controller=\"pact-v1\">\n<folded_state>\n<unit id=\"a\" kind=\"observation\" status=\"established\" refs=\"b:x\">fact</unit>\n</folded_state>\n</conversation_summary>"),
		None
	);
}
