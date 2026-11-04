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

#[test]
fn live_residency_is_bounded_and_nonincreasing_over_repeated_folds() {
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
	let mut previous = usize::MAX;
	for _ in 0..20 {
		let mut packets = build_packets("bounded", &messages, "test");
		link_dependencies(&mut packets);
		let mut pact = pact_with(packets[0].clone());
		pact.live_tokens = budget(&messages, 32_000);
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
		let summary=render(&pact,"<folded_state><unit kind=\"next_action\" status=\"pending\" refs=\"b:read\">Continue the open step.</unit></folded_state>","","fold",None,Some(&pact.pinned.task.text));
		let size = crate::session::estimate_tokens(&summary);
		assert!(size <= pact.live_tokens, "{size} > {}", pact.live_tokens);
		assert!(size <= previous, "{size} > {previous}");
		assert!(summary.contains("needed material"));
		assert!(summary.contains("Continue the open step."));
		previous = size;
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
	assert!(!summary.contains("id=\"old\""));
	assert!(crate::session::estimate_tokens(&summary) <= 1000);
}
