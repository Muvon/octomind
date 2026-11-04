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

use super::super::tests::message;
use super::*;

fn interaction(name: &str, args: Value, result: &str, id: &str) -> Vec<Message> {
	let mut call = message("assistant", "");
	call.tool_calls = Some(serde_json::json!([{"id":id,"name":name,"arguments":args}]));
	let mut result = message("tool", result);
	result.tool_call_id = Some(id.into());
	result.name = Some(name.into());
	vec![call, result]
}

fn read(path: &str, start: usize, lines: &[&str], id: &str) -> Vec<Message> {
	interaction(
		"view",
		serde_json::json!({"path":path}),
		&lines
			.iter()
			.enumerate()
			.map(|(i, s)| format!("{}:ab|{}", start + i, s))
			.collect::<Vec<_>>()
			.join("\n"),
		id,
	)
}

fn selected(messages: &[Message], budget: usize) -> (Vec<EvidencePacket>, String) {
	let mut packets = build_packets("working-set-test", messages, "test");
	link_dependencies(&mut packets);
	allocate_frontier(&mut packets, messages, budget, false, "", None);
	let content = packets
		.iter()
		.filter(|p| p.lane == Lane::KeepExact)
		.map(|p| p.prompt_content.clone())
		.collect::<Vec<_>>()
		.join("\n");
	(packets, content)
}

#[test]
fn keeps_current_regions_and_post_edit_text_with_shifted_coordinates() {
	let mut messages = read("artifact", 1, &["first", "old", "tail"], "read");
	messages.extend(interaction(
		"text_editor",
		serde_json::json!({"path":"artifact","old_text":"old","new_text":"new\nadded"}),
		"-2:ab (1 line)\n+2:ab|new\n+3:ab|added\nshift: lines after 2:ab +1",
		"edit",
	));
	let (packets, frontier) = selected(&messages, 2000);
	assert!(!frontier.contains("old"));
	assert!(frontier.contains("new\nadded"));
	let observed: BTreeMap<_, _> = region_lines(&frontier)
		.into_iter()
		.flat_map(|(h, _, lines)| lines.into_iter().enumerate().map(move |(i, s)| (h.start + i, s)))
		.collect();
	assert_eq!(
		observed,
		BTreeMap::from([
			(1, "first".into()),
			(2, "new".into()),
			(3, "added".into()),
			(4, "tail".into())
		])
	);
	let dir = tempfile::tempdir().unwrap();
	let archive =
		super::super::super::archive::write_archive_with_index_to(dir.path(), "fold", &messages, &packets).unwrap();
	let mut pact = super::super::tests::pact_with(packets[0].clone());
	pact.packets = packets;
	pact.verify_archive(&archive, &messages).unwrap();
}

#[test]
fn unknown_mutation_evicts_affected_artifact_only() {
	let mut messages = read("one", 1, &["stale"], "one");
	messages.extend(read("two", 1, &["current"], "two"));
	messages.extend(interaction(
		"replace",
		serde_json::json!({"path":"one"}),
		"updated",
		"write",
	));
	let (_, frontier) = selected(&messages, 2000);
	assert!(!frontier.contains("stale"));
	assert!(frontier.contains("current"));
}

#[test]
fn overlapping_observations_keep_each_current_line_once() {
	let mut messages = read("artifact", 1, &["one", "two", "three"], "first");
	messages.extend(read("artifact", 2, &["two", "THREE", "four"], "second"));
	let (_, frontier) = selected(&messages, 2000);
	let lines: Vec<_> = region_lines(&frontier)
		.into_iter()
		.flat_map(|(_, _, lines)| lines)
		.collect();
	assert_eq!(lines.len(), 4);
	assert_eq!(lines.iter().filter(|s| s.as_str() == "two").count(), 1);
	assert!(!lines.contains(&"three".to_string()));
}

#[test]
fn selection_respects_budget_and_prioritizes_later_references() {
	let mut messages = read("old", 1, &[&"old ".repeat(100)], "one");
	messages.extend(read("active", 1, &[&"current ".repeat(100)], "two"));
	messages.push(message("assistant", "Continue with active"));
	let (packets, frontier) = selected(&messages, 200);
	assert!(crate::session::estimate_tokens(&frontier) <= 200);
	assert!(packets
		.iter()
		.filter(|p| p.lane == Lane::KeepExact)
		.all(|p| !p.exact_spans.is_empty()));
}

#[test]
fn continuation_is_folded_and_never_nested_in_frontier() {
	let messages = vec![
		message(
			"assistant",
			"<conversation_summary><folded_state>durable</folded_state></conversation_summary>",
		),
		message(
			"user",
			"<continuation>\n<request>original request</request>\n<task>resume action</task>\n</continuation>",
		),
		message("assistant", "next output"),
	];
	let (_, frontier) = selected(&messages, 2000);
	assert!(!frontier.contains("<continuation>"));
	assert!(!frontier.contains("original request"));
	assert!(!frontier.contains("<conversation_summary"));
}

#[test]
fn repeated_folds_keep_material_without_accumulating_wrappers() {
	let mut messages = read("artifact", 1, &["exact current material"], "read");
	let mut sizes = Vec::new();
	for _ in 0..20 {
		let (packets, frontier) = selected(&messages, 2000);
		assert!(frontier.contains("exact current material"));
		assert!(!frontier.contains("<conversation_summary"));
		let mut pact = super::super::tests::pact_with(packets[0].clone());
		pact.packets = packets;
		let summary = super::super::super::knowledge::format_compressed_entry_with_pact(
			"<folded_state>durable fact</folded_state>",
			"",
			"fold".into(),
			None,
			&pact,
			None,
		);
		sizes.push(summary.len());
		messages = vec![
			message("assistant", &summary),
			message("user", "<continuation>\n<task>resume</task>\n</continuation>"),
			message("assistant", "continue"),
		];
	}
	assert!(sizes[2..].iter().all(|n| *n == sizes[2]), "{sizes:?}");
}

#[test]
fn mutation_later_in_same_packet_does_not_resurrect_old_observation() {
	let mut messages = read("artifact", 1, &["before", "unaffected"], "read");
	let edit = interaction(
		"replace",
		serde_json::json!({"path":"artifact","old_text":"before","new_text":"after"}),
		"-1:ab (1 line)\n+1:ab|after\nshift: lines after 1:ab +0",
		"edit",
	);
	let edit_call = edit[0].tool_calls.as_ref().unwrap()[0].clone();
	messages[0]
		.tool_calls
		.as_mut()
		.unwrap()
		.as_array_mut()
		.unwrap()
		.push(edit_call);
	messages.push(edit[1].clone());
	let (_, frontier) = selected(&messages, 2000);
	assert!(!frontier.contains("before"));
	assert!(frontier.contains("after"));
	assert!(frontier.contains("unaffected"));
}

#[test]
fn background_mutation_invalidates_before_fold_commits() {
	let source = read("artifact", 1, &["stale", "unchanged"], "read");
	let (mut packets, _) = selected(&source, 2000);
	let tail = interaction(
		"replace",
		serde_json::json!({"path":"artifact","old_text":"stale","new_text":"fresh"}),
		"-1:ab (1 line)\n+1:ab|fresh\nshift: lines after 1:ab +0",
		"edit",
	);
	revalidate(&mut packets, &source, &tail);
	let text = packets
		.iter()
		.map(|p| p.prompt_content.as_str())
		.collect::<Vec<_>>()
		.join("\n");
	assert!(!text.contains("stale"));
	assert!(!text.contains("fresh"), "new observations already live in the suffix");
	assert!(text.contains("unchanged"));
}

#[test]
fn runtime_expansion_skips_already_retained_regions() {
	let messages = read("artifact", 3, &["three", "four"], "read");
	let (packets, _) = selected(&messages, 2000);
	assert_eq!(unretained_ranges(&packets, "artifact", 1, 6), vec![(1, 2), (5, 6)]);
}

#[test]
fn overwritten_output_is_not_current_even_when_execution_is_a_check() {
	let mut messages = read("result.log", 1, &["old result"], "read");
	messages.extend(interaction(
		"run",
		serde_json::json!({"command":"validator > result.log 2>&1"}),
		"finished",
		"check",
	));
	let (_, frontier) = selected(&messages, 2000);
	assert!(!frontier.contains("old result"));
}

#[test]
fn acknowledged_replacement_inherits_batch_path_and_respects_observed_text() {
	let args = serde_json::json!({"path":"artifact","edits":[{"old_text":"old","new_text":"one\ntwo\nthree\nfour"}]});
	let regions = observations("+1:ab|one\n+2:cd..3:ef (2 lines as sent)\n+4:ab|four", &args);
	assert_eq!(regions[0].lines, vec!["one", "two", "three", "four"]);
	let regions = observations("+1:ab|one\n+2:cd|observed instead\n+4:ab|four", &args);
	assert_eq!(regions[0].lines, vec!["one", "observed instead"]);
	assert!(!regions.iter().any(|r| r.lines.iter().any(|s| s == "three")));
}

#[test]
fn multi_file_search_output_is_never_attributed_to_the_argument_path() {
	let mut messages = interaction(
		"view",
		serde_json::json!({"path":"src","content":"needle"}),
		"src/a.rs:\n12:ab|needle | one\nsrc/b.rs:\n12:cd|needle | two",
		"search",
	);
	messages.push(message("assistant", "Both files match."));
	let (_, frontier) = selected(&messages, 2000);
	assert!(!frontier.contains(REGION_OPEN));
	assert!(frontier.contains("src/a.rs:") && frontier.contains("src/b.rs:"));
}

#[test]
fn single_file_search_hits_keep_their_own_line_numbers() {
	let messages = interaction(
		"view",
		serde_json::json!({"path":"src/a.rs","content":"needle"}),
		"12:ab|needle one\n40:cd|needle two",
		"search",
	);
	let (packets, _) = selected(&messages, 2000);
	assert_eq!(
		unretained_ranges(&packets, "./src/a.rs", 10, 41),
		vec![(10, 11), (13, 39), (41, 41)]
	);
}

#[test]
fn closure_read_larger_than_the_budget_is_trimmed_not_dropped() {
	let big: Vec<String> = (1..=200)
		.map(|n| format!("line {n} of the big artifact body"))
		.collect();
	let big: Vec<&str> = big.iter().map(String::as_str).collect();
	let mut messages = read("small", 1, &["tiny"], "small");
	messages.push(message("assistant", "small small small"));
	messages.extend(read("big", 1, &big, "big"));
	messages.push(message("assistant", "Continue with big"));
	let (packets, frontier) = selected(&messages, 120);
	let read = packets.iter().find(|p| p.message_start == 3).unwrap();
	assert_eq!(read.lane, Lane::KeepExact);
	assert!(!read.exact_spans.is_empty());
	let kept: usize = region_lines(&frontier)
		.iter()
		.filter(|(h, _, _)| h.path == "big")
		.map(|(h, _, _)| h.lines)
		.sum();
	assert!(kept > 0 && kept < 200, "{kept}");
	// One trailing newline per admitted region is outside the accounting.
	let selected_tokens: usize = packets
		.iter()
		.filter(|p| p.lane == Lane::KeepExact)
		.map(|p| crate::session::estimate_tokens(&p.prompt_content))
		.sum();
	assert!(selected_tokens <= 120 + packets.len(), "{selected_tokens}");
	let dir = tempfile::tempdir().unwrap();
	let archive =
		super::super::super::archive::write_archive_with_index_to(dir.path(), "fold", &messages, &packets).unwrap();
	let mut pact = super::super::tests::pact_with(packets[0].clone());
	pact.packets = packets;
	pact.verify_archive(&archive, &messages).unwrap();
}

#[test]
fn tail_mutation_evicting_a_live_dependency_keeps_it_exact_as_an_observation() {
	let mut messages = read("other", 1, &["other text"], "other");
	messages.extend(read("artifact", 1, &["first", "second"], "read"));
	messages.push(message("assistant", "Continue with artifact"));
	let (mut packets, _) = selected(&messages, 2000);
	let other = packets.iter().position(|p| p.message_start == 0).unwrap();
	let read = packets.iter().position(|p| p.message_start == 2).unwrap();
	assert_eq!(packets[other].lane, Lane::KeepExact);
	assert_eq!(packets[read].lane, Lane::KeepExact);
	let mut tail = interaction("replace", serde_json::json!({"path":"artifact"}), "updated", "edit");
	tail.extend(interaction(
		"replace",
		serde_json::json!({"path":"other"}),
		"updated",
		"edit2",
	));
	revalidate(&mut packets, &messages, &tail);
	// Nothing depends on the old read of `other`: a recall pointer now.
	assert_eq!(packets[other].lane, Lane::ArchiveReference);
	assert!(packets[other].prompt_content.is_empty());
	// The checkpoint depends on the evicted read: still exact, as a transcript
	// observation rather than a claim about the live artifact.
	assert_eq!(packets[read].lane, Lane::KeepExact);
	assert!(!packets[read].exact_spans.is_empty());
	assert!(packets[read].prompt_content.contains("[MESSAGE 4 TOOL RESULT"));
	assert_eq!(unretained_ranges(&packets, "artifact", 1, 2), vec![(1, 2)]);
}
