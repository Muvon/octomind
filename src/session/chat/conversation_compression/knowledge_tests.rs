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
fn compressed_entry_wraps_body_file_context_and_archive_pointer() {
	let dir = tempfile::tempdir().expect("temp dir");
	let archive = dir.path().join("sessions/archive/s1/c.jsonl");
	let rendered = format_compressed_entry_with_context(
		"summary body",
		"src/main.rs:1-10",
		"c-42".to_string(),
		Some(&archive),
	);
	assert!(rendered.starts_with("<conversation_summary id=\"c-42\">\n"));
	assert!(rendered.contains("summary body\n"));
	assert!(rendered.contains("<file_context>\nsrc/main.rs:1-10\n</file_context>\n"));
	assert!(rendered.contains(&format!("<archive path=\"{}\"", archive.display())));
	assert!(rendered.ends_with("</conversation_summary>"));
}

#[test]
fn compressed_entry_with_no_sections_is_a_bare_wrapper() {
	let rendered = format_compressed_entry_with_context("", "", "x".to_string(), None);
	assert_eq!(
		rendered,
		"<conversation_summary id=\"x\">\n</conversation_summary>"
	);
}

#[test]
fn strip_regrown_sections_removes_every_regrown_block() {
	let summary = concat!(
		"kept headline\n",
		"<file_context>\nstale bytes\n</file_context>\n",
		"<analysis_findings>\n<finding>old finding</finding>\n</analysis_findings>\n",
		"<recall_index>\nfirst ids\n</recall_index>\n",
		"<recall_index>\nmore ids\n</recall_index>\n",
		"tail kept"
	);
	let stripped = strip_regrown_sections(summary);
	assert!(stripped.starts_with("kept headline"));
	assert!(stripped.contains("tail kept"));
	for gone in [
		"<file_context>",
		"<analysis_findings>",
		"<recall_index>",
		"stale bytes",
		"old finding",
		"more ids",
	] {
		assert!(!stripped.contains(gone), "`{gone}` must be stripped");
	}
}

#[test]
fn strip_recall_index_removes_every_index_and_keeps_the_rest() {
	let text = concat!(
		"before\n",
		"<recall_index>\na\n</recall_index>\n",
		"middle\n",
		"<recall_index>\nb\n</recall_index>\n",
		"after"
	);
	assert_eq!(strip_recall_index(text), "before\nmiddle\nafter");
}

#[test]
fn strip_block_without_a_close_tag_drops_everything_afterwards() {
	// A malformed summary must strip cleanly instead of re-embedding the
	// half-open block.
	assert_eq!(
		strip_regrown_sections("head\n<file_context>\nnever closed"),
		"head"
	);
}

fn summary_message(findings: &[&str]) -> Message {
	let body = findings
		.iter()
		.map(|f| format!("<finding>{f}</finding>"))
		.collect::<Vec<_>>()
		.join("\n");
	Message {
		role: "assistant".to_string(),
		content: format!(
			"<conversation_summary id=\"s\">\nbody\n<analysis_findings>\n{body}\n</analysis_findings>\n</conversation_summary>"
		),
		..Default::default()
	}
}

#[test]
fn latest_analysis_findings_reads_the_most_recent_summary() {
	let messages = vec![
		Message {
			role: "user".to_string(),
			content: "hello".to_string(),
			..Default::default()
		},
		summary_message(&["stale"]),
		summary_message(&["fresh alpha", "fresh beta"]),
	];
	assert_eq!(
		latest_analysis_findings(&messages),
		vec!["fresh alpha".to_string(), "fresh beta".to_string()]
	);
}

#[test]
fn latest_analysis_findings_is_empty_without_a_summary() {
	let messages = vec![Message {
		role: "assistant".to_string(),
		content: "plain turn, no summary wrapper".to_string(),
		..Default::default()
	}];
	assert!(latest_analysis_findings(&messages).is_empty());
}

#[test]
fn merge_latest_exact_dedupes_on_normalized_wording() {
	let held = vec!["Rust  must  be   installed".to_string()];
	let entries = vec![
		"rust must be installed".to_string(),
		"   ".to_string(),
		"new fact".to_string(),
	];
	assert_eq!(
		merge_latest_exact(&held, &entries),
		vec!["rust must be installed".to_string(), "new fact".to_string()]
	);
}

#[test]
fn analysis_findings_tokens_is_zero_when_empty_and_grows_with_entries() {
	assert_eq!(analysis_findings_tokens(&[]), 0);
	let one = analysis_findings_tokens(&["single finding".to_string()]);
	let two =
		analysis_findings_tokens(&["single finding".to_string(), "second finding".to_string()]);
	assert!(one > 0);
	assert!(two > one);
}

#[test]
fn select_newest_with_budget_keeps_the_newest_entries_that_fit() {
	let findings = vec![
		"old short".to_string(),
		"newer and considerably longer finding that costs many more tokens".to_string(),
	];
	let both = analysis_findings_tokens(&findings);
	let only_newest = analysis_findings_tokens(&findings[1..]);

	let newest_only = select_newest_with_budget(&findings, only_newest);
	assert_eq!(
		newest_only,
		vec!["newer and considerably longer finding that costs many more tokens".to_string()]
	);
	assert_eq!(select_newest_with_budget(&findings, both), findings);
	assert!(select_newest_with_budget(&findings, 0).is_empty());
}

#[test]
fn select_findings_with_vectors_keeps_order_for_orthogonal_findings() {
	let findings = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
	let vectors = vec![
		vec![1.0, 0.0, 0.0],
		vec![0.0, 1.0, 0.0],
		vec![0.0, 0.0, 1.0],
	];
	let budget = analysis_findings_tokens(&findings);
	assert_eq!(
		select_findings_with_vectors(&findings, &vectors, None, budget),
		findings
	);
}

#[test]
fn select_findings_with_vectors_falls_back_to_newest_on_length_mismatch() {
	let findings = vec!["one".to_string(), "two".to_string()];
	assert_eq!(
		select_findings_with_vectors(&findings, &[vec![1.0]], None, usize::MAX),
		findings
	);
}

#[test]
#[serial_test::serial]
fn fold_critical_knowledge_dedupes_and_trims_to_the_retention_limit() {
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let mut config = crate::session::chat::test_support::fake_provider_config();
	config.compression.knowledge_retention = 2;
	let mut session = ChatSession::for_tests(Vec::new());
	session.critical_knowledge = vec!["k0".to_string()];

	fold_critical_knowledge(
		&mut session,
		&config,
		&["k0".to_string(), "k1".to_string(), "k2".to_string()],
	);
	std::env::remove_var("OCTOMIND_DATA_DIR");

	// k0 is a duplicate (not re-added); the retention limit keeps the newest 2.
	assert_eq!(
		session.critical_knowledge,
		vec!["k1".to_string(), "k2".to_string()]
	);
}

#[test]
fn fold_critical_knowledge_ignores_blank_entries() {
	let config = crate::session::chat::test_support::fake_provider_config();
	let mut session = ChatSession::for_tests(Vec::new());
	fold_critical_knowledge(&mut session, &config, &["   ".to_string(), String::new()]);
	assert!(session.critical_knowledge.is_empty());
}
