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

#[test]
fn archive_roundtrip_preserves_every_message_verbatim() {
	let dir = std::env::temp_dir().join(format!("octomind-archive-test-{}", std::process::id()));
	let messages = vec![
		msg("user", "fix the parser"),
		msg("assistant", "looking at src/parser.rs"),
		msg("tool", "error: unexpected token at line 42"),
	];

	let path = write_archive_to(&dir, "test-id-1", &messages).expect("write succeeds");
	let content = std::fs::read_to_string(&path).expect("archive readable");
	let lines: Vec<&str> = content.lines().collect();
	assert_eq!(lines.len(), 3);

	// Every line deserializes back to the exact original message.
	for (line, original) in lines.iter().zip(messages.iter()) {
		let restored: Message = serde_json::from_str(line).expect("valid JSON line");
		assert_eq!(restored.role, original.role);
		assert_eq!(restored.content, original.content);
	}

	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn decode_jsonl_recovers_verbatim_content_with_escapes() {
	// A quote containing `"`, `\` and newlines is JSON-escaped on disk;
	// evidence matching needs the decoded original, not the raw JSONL.
	let content = "let s = \"a\\nb\";\nsecond line";
	let raw = format!(
		"{}\n",
		serde_json::to_string(&msg("tool", content)).unwrap()
	);
	let decoded = decode_jsonl_contents(&[raw]);
	assert_eq!(decoded, vec![content]);
	// A cap-truncated trailing line is skipped, not an error.
	let decoded = decode_jsonl_contents(&["{\"role\":\"tool\",\"cont".to_string()]);
	assert!(decoded.is_empty());
}

#[test]
fn archive_pointer_names_the_path_and_recall_guidance() {
	let pointer = archive_pointer(Path::new("/tmp/sessions/archive/s1/abc.jsonl"));
	assert!(pointer.contains("path=\"/tmp/sessions/archive/s1/abc.jsonl\""));
	assert!(pointer.contains("do not guess"));
}

#[test]
fn pact_sidecar_roundtrip_recovers_atomic_packet() {
	let dir =
		std::env::temp_dir().join(format!("octomind-pact-archive-test-{}", std::process::id()));
	let messages = vec![msg("assistant", "issued call"), msg("tool", "exact result")];
	let packet_id = "b:test".to_string();
	let packet = super::super::attention::EvidencePacket {
		id: packet_id.clone(),
		kind: super::super::attention::PacketKind::ToolInteraction,
		provenance: super::super::attention::Provenance::ToolObserved,
		message_start: 0,
		message_end: 1,
		depends_on: Vec::new(),
		linkage: super::super::attention::PacketLinkage::StructuredIds,
		tokens: 4,
		lane: super::super::attention::Lane::KeepExact,
		prompt_content: "issued call\nexact result".into(),
		exact_spans: Vec::new(),
		descriptor: "tool interaction".into(),
	};
	let bundle = write_archive_with_index_to(&dir, "pact", &messages, &[packet])
		.expect("archive and sidecar write");
	let recovered = read_blocks(&bundle.index_path, &[packet_id]).expect("stable ID resolves");
	assert_eq!(recovered.len(), 2);
	assert_eq!(recovered[1].content, "exact result");
	let corrupted = vec![
		msg("assistant", "issued call"),
		msg("tool", "changed result"),
	];
	write_archive_to(&dir, "pact", &corrupted).expect("corrupt fixture writes");
	let error = read_blocks(&bundle.index_path, &["b:test".into()]).unwrap_err();
	assert!(error.to_string().contains("content-address verification"));
	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pact_archive_storage_failure_is_returned_to_the_transaction_caller() {
	let blocker = std::env::temp_dir().join(format!(
		"octomind-pact-archive-blocker-{}-{}",
		std::process::id(),
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_nanos()
	));
	std::fs::write(&blocker, "not a directory").expect("fixture blocker writes");
	let error = write_archive_with_index_to(
		&blocker.join("child"),
		"pact-failure",
		&[msg("assistant", "must remain live")],
		&[],
	)
	.unwrap_err();
	assert!(error.to_string().contains("failed to create archive dir"));
	let _ = std::fs::remove_file(blocker);
}
