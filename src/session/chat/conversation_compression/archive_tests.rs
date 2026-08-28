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

fn msg(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

fn packet(id: &str, start: usize, end: usize) -> super::super::attention::EvidencePacket {
	super::super::attention::EvidencePacket {
		id: id.to_string(),
		kind: super::super::attention::PacketKind::ToolInteraction,
		provenance: super::super::attention::Provenance::ToolObserved,
		message_start: start,
		message_end: end,
		depends_on: Vec::new(),
		linkage: super::super::attention::PacketLinkage::StructuredIds,
		tokens: 4,
		lane: super::super::attention::Lane::KeepExact,
		prompt_content: "issued call\nexact result".into(),
		exact_spans: Vec::new(),
		descriptor: "tool interaction".into(),
	}
}

#[test]
#[serial_test::serial]
fn archive_messages_writes_under_the_sessions_archive_directory() {
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());
	let messages = vec![
		msg("user", "fix the parser"),
		msg("tool", "error at line 42"),
	];
	let written = archive_messages("archive-env-test", "cid-1", &messages);
	std::env::remove_var("OCTOMIND_DATA_DIR");

	let path = written.expect("archive writes");
	let expected = data_dir
		.path()
		.join("sessions")
		.join("archive")
		.join("archive-env-test")
		.join("cid-1.jsonl");
	assert_eq!(path, expected);
	let content = std::fs::read_to_string(&path).expect("archive readable");
	assert_eq!(content.lines().count(), 2);
}

#[test]
#[serial_test::serial]
fn archive_messages_handles_an_empty_range() {
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());
	let written = archive_messages("archive-env-empty", "cid-empty", &[]);
	std::env::remove_var("OCTOMIND_DATA_DIR");

	let path = written.expect("an empty archive still writes a file");
	assert!(path.is_file());
	let content = std::fs::read_to_string(&path).expect("archive readable");
	assert_eq!(content.lines().count(), 0);
}

#[test]
fn read_blocks_rejects_unknown_ids() {
	let dir = tempfile::tempdir().expect("temp dir");
	let messages = vec![msg("assistant", "issued call"), msg("tool", "exact result")];
	let bundle =
		write_archive_with_index_to(dir.path(), "pact", &messages, &[packet("b:one", 0, 1)])
			.expect("bundle writes");
	let error = read_blocks(&bundle.index_path, &["b:missing".to_string()])
		.expect_err("an unknown stable ID must not resolve");
	assert!(error.to_string().contains("not found"));
}

#[test]
fn read_blocks_with_no_requested_ids_returns_no_messages() {
	let dir = tempfile::tempdir().expect("temp dir");
	let messages = vec![msg("assistant", "issued call"), msg("tool", "exact result")];
	let bundle =
		write_archive_with_index_to(dir.path(), "pact", &messages, &[packet("b:one", 0, 1)])
			.expect("bundle writes");
	let recovered = read_blocks(&bundle.index_path, &[]).expect("empty request is not an error");
	assert!(recovered.is_empty());
}

#[test]
fn bundle_entry_resolves_stable_ids() {
	let dir = tempfile::tempdir().expect("temp dir");
	let messages = vec![msg("assistant", "issued call"), msg("tool", "exact result")];
	let bundle =
		write_archive_with_index_to(dir.path(), "pact", &messages, &[packet("b:one", 0, 1)])
			.expect("bundle writes");
	assert!(bundle.entry("b:one").is_some());
	assert!(bundle.entry("b:other").is_none());
}
