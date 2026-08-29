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

//! Recall over hand-written PACT sidecars: the registry scan, the per-index
//! grouping, and the verbatim dereference path, exercised end to end against
//! a temp data dir. The sidecar entries are written directly (with a locally
//! computed content digest) because the packet types behind the real writer
//! are private to the compression module.

use super::*;
use crate::session::Message;
use sha2::{Digest, Sha256};

fn msg(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

/// Mirror of `archive::block_digest` — hand-written sidecar entries must carry
/// the same content-addressed digest the reader verifies on every recall.
fn block_digest(id: &str, messages: &[Message]) -> String {
	let mut hasher = Sha256::new();
	hasher.update(b"octomind-pact-archive-v1\0");
	hasher.update(id.as_bytes());
	for message in messages {
		hasher.update([0u8]);
		hasher.update(serde_json::to_vec(message).expect("message serializes"));
	}
	hasher
		.finalize()
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect()
}

/// Write `<data>/sessions/archive/<session>/<cid>.jsonl` plus its
/// `.blocks.jsonl` sidecar describing the given (id, 1-based line range) blocks.
fn write_sidecar(
	data_dir: &std::path::Path,
	session: &str,
	cid: &str,
	messages: &[Message],
	blocks: &[(&str, usize, usize)],
) {
	let dir = data_dir.join("sessions").join("archive").join(session);
	std::fs::create_dir_all(&dir).expect("create archive dir");

	let mut archive = String::new();
	for m in messages {
		archive.push_str(&serde_json::to_string(m).expect("message serializes"));
		archive.push('\n');
	}
	std::fs::write(dir.join(format!("{cid}.jsonl")), archive).expect("write archive");

	let mut sidecar = String::new();
	for (id, start, end) in blocks {
		let block = &messages[start - 1..*end];
		let entry = json!({
			"id": id,
			"kind": "tool_interaction",
			"provenance": "tool_observed",
			"dependencies": [],
			"linkage": "structured_ids",
			"exact_spans": [],
			"content_digest": block_digest(id, block),
			"archive_line_start": start,
			"archive_line_end": end,
			"descriptor": "tool interaction",
		});
		sidecar.push_str(&entry.to_string());
		sidecar.push('\n');
	}
	std::fs::write(dir.join(format!("{cid}.blocks.jsonl")), sidecar).expect("write sidecar");
}

fn call(ids: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: "recall".to_string(),
		parameters: ids,
		tool_id: "t1".to_string(),
	}
}

#[tokio::test]
async fn recall_requires_an_active_session() {
	let err = execute_recall(&call(json!({"ids": ["b:1"]})))
		.await
		.expect_err("no session scope is active");
	assert!(err.to_string().contains("no active session"), "err: {err}");
}

#[tokio::test]
#[serial_test::serial]
async fn recall_errors_when_nothing_was_archived_yet() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let session = format!("recall-empty-{}", std::process::id());
	let result = crate::session::context::with_session_id(session, async {
		execute_recall(&call(json!({"ids": ["b:1"]}))).await
	})
	.await;
	std::env::remove_var("OCTOMIND_DATA_DIR");

	let err = result.expect_err("registry is empty");
	assert!(
		err.to_string().contains("no compressed blocks archived"),
		"err: {err}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn recall_returns_archived_messages_verbatim() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let session = format!("recall-hit-{}", std::process::id());
	write_sidecar(
		data_dir.path(),
		&session,
		"cid-1",
		&[
			msg("user", "fix the parser"),
			msg("tool", "error at line 42"),
		],
		&[("b:alpha", 1, 2)],
	);

	let result = crate::session::context::with_session_id(session, async {
		execute_recall(&call(json!({"ids": ["b:alpha"]}))).await
	})
	.await;
	std::env::remove_var("OCTOMIND_DATA_DIR");

	let out = result.expect("block resolves");
	assert_eq!(out.tool_id, "t1");
	assert!(!out.is_error());
	let content = out.extract_content();
	assert!(
		content.contains("<recall ids=\"b:alpha\">"),
		"content: {content}"
	);
	assert!(content.contains("[user] fix the parser"));
	assert!(content.contains("[tool] error at line 42"));
	assert!(content.contains("</recall>"));
}

#[tokio::test]
#[serial_test::serial]
async fn recall_rejects_unknown_block_ids() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let session = format!("recall-unknown-{}", std::process::id());
	write_sidecar(
		data_dir.path(),
		&session,
		"cid-1",
		&[msg("user", "fix the parser")],
		&[("b:alpha", 1, 1)],
	);

	let result = crate::session::context::with_session_id(session, async {
		execute_recall(&call(json!({"ids": ["b:alpha", "b:ghost"]}))).await
	})
	.await;
	std::env::remove_var("OCTOMIND_DATA_DIR");

	let err = result.expect_err("one ID is not in the registry");
	assert!(err.to_string().contains("unknown block ID"), "err: {err}");
	assert!(err.to_string().contains("b:ghost"), "err: {err}");
}

#[tokio::test]
#[serial_test::serial]
async fn recall_groups_ids_per_sidecar_index() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let session = format!("recall-groups-{}", std::process::id());
	// Two compression cycles → two sidecar indexes, one block each.
	write_sidecar(
		data_dir.path(),
		&session,
		"cid-a",
		&[msg("user", "a1"), msg("tool", "a2")],
		&[("b:one", 1, 2)],
	);
	write_sidecar(
		data_dir.path(),
		&session,
		"cid-b",
		&[msg("assistant", "b1")],
		&[("b:two", 1, 1)],
	);

	let result = crate::session::context::with_session_id(session, async {
		execute_recall(&call(json!({"ids": ["b:one", "b:two"]}))).await
	})
	.await;
	std::env::remove_var("OCTOMIND_DATA_DIR");

	let content = result.expect("both blocks resolve").extract_content();
	let one = content
		.find("<recall ids=\"b:one\">")
		.expect("group one: {content}");
	let two = content
		.find("<recall ids=\"b:two\">")
		.expect("group two: {content}");
	assert!(one < two, "indexes iterate in path order: {content}");
	assert!(content.contains("[user] a1"));
	assert!(content.contains("[tool] a2"));
	assert!(content.contains("[assistant] b1"));
	assert_eq!(
		content.matches("</recall>").count(),
		2,
		"content: {content}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn recall_filters_non_string_id_entries() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data_dir.path());

	let session = format!("recall-filter-{}", std::process::id());
	write_sidecar(
		data_dir.path(),
		&session,
		"cid-1",
		&[msg("user", "kept")],
		&[("b:alpha", 1, 1)],
	);

	// Only non-string entries → treated as an empty request.
	let result = crate::session::context::with_session_id(session.clone(), async {
		execute_recall(&call(json!({"ids": [42, true]}))).await
	})
	.await;
	assert!(result.is_err(), "non-string-only ids must be rejected");

	// Mixed: the string entry survives, the number is dropped.
	let result = crate::session::context::with_session_id(session, async {
		execute_recall(&call(json!({"ids": ["b:alpha", 42]}))).await
	})
	.await;
	std::env::remove_var("OCTOMIND_DATA_DIR");

	let content = result.expect("the string entry survives").extract_content();
	assert!(content.contains("[user] kept"), "content: {content}");
}

#[test]
fn recall_function_schema_bounds_the_batch_size() {
	let f = get_recall_function();
	assert_eq!(f.name, "recall");
	assert_eq!(f.parameters["required"][0], "ids");
	assert_eq!(f.parameters["properties"]["ids"]["minItems"], 1);
	assert_eq!(
		f.parameters["properties"]["ids"]["maxItems"],
		MAX_BLOCKS_PER_CALL
	);
}
