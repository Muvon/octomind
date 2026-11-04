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
//! Offline harness: selection and live rendering are production functions;
//! only the generative folded state is replaced by its recorded output.
use super::*;

#[test]
#[ignore = "requires FOLD_WS_INPUT and FOLD_WS_OUTPUT saved-trace paths"]
fn replay_saved_folds() {
	let input = std::fs::read_to_string(std::env::var("FOLD_WS_INPUT").unwrap()).unwrap();
	let cases: Vec<serde_json::Value> = serde_json::from_str(&input).unwrap();
	let mut results = Vec::new();
	let mut previous: BTreeMap<String, String> = BTreeMap::new();
	let chained = std::env::var_os("FOLD_WS_CHAIN").is_some();
	for case in cases {
		let mut messages: Vec<Message> = serde_json::from_value(case["messages"].clone()).unwrap();
		let label = case["label"].as_str().unwrap();
		if chained {
			if let Some(prior) = previous.get(label) {
				for message in &mut messages {
					if message.name.as_deref() == Some(super::super::apply::COMPRESSION_MESSAGE_NAME) {
						message.content = prior.clone();
					}
				}
			}
		}
		let budget = case["target_tokens"].as_u64().unwrap() as usize;
		let mut packets = build_packets(
			case["name"].as_str().unwrap(),
			&messages,
			case["model"].as_str().unwrap_or("test"),
		);
		link_dependencies(&mut packets);
		allocate_frontier(
			&mut packets,
			&messages,
			budget,
			false,
			case["plan"].as_str().unwrap_or(""),
		);
		let mut pact = super::tests::pact_with(packets[0].clone());
		pact.packets = packets;
		pact.target_tokens = budget;
		let dir = tempfile::tempdir().unwrap();
		let archive =
			super::super::archive::write_archive_with_index_to(dir.path(), "replay", &messages, &pact.packets).unwrap();
		pact.verify_archive(&archive, &messages).unwrap();
		let (_, bands) = pact.render_live_bands(Some(&archive), None, case["recorded_summary"].as_str().unwrap());
		let frontier = bands
			.split("<active_frontier>\n")
			.nth(1)
			.and_then(|s| s.split("</active_frontier>").next())
			.unwrap_or("");
		let recorded = case["recorded_summary"].as_str().unwrap();
		// Hold the recorded generative output fixed, replacing runtime bands only.
		let mut summary = recorded.to_string();
		for tag in ["active_frontier", "recall_index"] {
			let open = format!("<{tag}>");
			let close = format!("</{tag}>");
			let replacement = bands
				.find(&open)
				.map(|start| {
					let end = bands[start..].find(&close).unwrap() + start + close.len();
					&bands[start..end]
				})
				.unwrap_or("");
			if let Some(start) = summary.find(&open) {
				let end = summary[start..].find(&close).unwrap() + start + close.len();
				summary.replace_range(start..end, replacement);
			} else {
				summary.push_str(replacement);
			}
		}
		previous.insert(label.to_string(), summary.clone());

		results.push(serde_json::json!({
			"label": case["label"], "fold": case["fold"], "budget": budget,
			"summary_chars": summary.chars().count(), "summary_tokens": crate::session::estimate_tokens(&summary),
			"frontier_chars": frontier.chars().count(), "frontier_tokens": crate::session::estimate_tokens(frontier),
			"frontier": frontier, "summary": summary,
		}));
	}
	std::fs::write(
		std::env::var("FOLD_WS_OUTPUT").unwrap(),
		serde_json::to_vec_pretty(&results).unwrap(),
	)
	.unwrap();
}
