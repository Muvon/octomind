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
use serde_json::json;

fn refs(strings: &[&str]) -> Vec<String> {
	strings.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_merge_file_refs_overlaps_and_gaps() {
	// Overlapping ranges merge
	assert_eq!(
		merge_file_refs(&refs(&["a.rs:10:50", "a.rs:30:100"])),
		vec!["a.rs:10:100"]
	);
	// Gap of ≤10 lines merges
	assert_eq!(
		merge_file_refs(&refs(&["a.rs:10:20", "a.rs:25:30"])),
		vec!["a.rs:10:30"]
	);
	// Gap of 11 lines stays separate
	assert_eq!(
		merge_file_refs(&refs(&["a.rs:10:20", "a.rs:31:40"])),
		vec!["a.rs:10:20", "a.rs:31:40"]
	);
	// Unsorted input is sorted before merging
	assert_eq!(
		merge_file_refs(&refs(&["a.rs:200:250", "a.rs:10:50"])),
		vec!["a.rs:10:50", "a.rs:200:250"]
	);
}

#[test]
fn test_merge_file_refs_whole_files_and_malformed() {
	// Whole-file ref supersedes any ranges for the same file
	assert_eq!(merge_file_refs(&refs(&["a.rs:1:5", "a.rs"])), vec!["a.rs"]);
	// Malformed two-part ref is dropped
	assert!(merge_file_refs(&refs(&["a.rs:5"])).is_empty());
	// Files are kept independent
	let merged = merge_file_refs(&refs(&["b.rs:1:5", "a.rs:1:5"]));
	assert_eq!(merged, vec!["a.rs:1:5", "b.rs:1:5"]);
}

#[test]
fn test_extract_file_refs_from_args() {
	let mut out = Vec::new();

	// Non-file tools contribute nothing
	extract_file_refs_from_args("shell", &json!({"path": "a.rs"}), &mut out);
	assert!(out.is_empty());

	// view with both line ends → ranged ref; a line id counts by its number
	extract_file_refs_from_args("view", &json!({"path": "a.rs", "start": 10, "end": "20:a3"}), &mut out);
	assert_eq!(out, vec!["a.rs:10:20"]);

	// view without both ends, or with an end counted from the file end → whole file
	out.clear();
	extract_file_refs_from_args("view", &json!({"path": "t.rs"}), &mut out);
	extract_file_refs_from_args("view", &json!({"path": "u.rs", "start": 5, "end": -1}), &mut out);
	assert_eq!(out, vec!["t.rs", "u.rs"]);

	// view with several spans of one file
	out.clear();
	extract_file_refs_from_args(
		"view",
		&json!({"path": "x.rs", "ranges": [{"start": 1, "end": 4}, {"start": "80:3f", "end": 95}]}),
		&mut out,
	);
	assert_eq!(out, vec!["x.rs:1:4", "x.rs:80:95"]);

	// view of several files in one call
	out.clear();
	extract_file_refs_from_args("view", &json!({"path": "p.rs | q.rs"}), &mut out);
	assert_eq!(out, vec!["p.rs", "q.rs"]);

	// batch_edit: an operation names its own file or works on the call's
	out.clear();
	extract_file_refs_from_args(
		"batch_edit",
		&json!({"path": "e.rs", "operations": [
			{"operation": "view", "path": "s.rs", "start": 1, "end": 4},
			{"operation": "insert", "start": "7:0c", "content": "x"}
		]}),
		&mut out,
	);
	assert_eq!(out, vec!["s.rs:1:4", "e.rs"]);

	// text_edit: the call's file, or one file per replacement
	out.clear();
	extract_file_refs_from_args("text_edit", &json!({"path": "c.rs", "command": "create"}), &mut out);
	extract_file_refs_from_args(
		"text_edit",
		&json!({"edits": [{"path": "m.rs", "old_text": "a", "new_text": "b"}]}),
		&mut out,
	);
	assert_eq!(out, vec!["c.rs", "m.rs"]);

	// symbol: the file it read definitions from
	out.clear();
	extract_file_refs_from_args("symbol", &json!({"name": "load", "path": "src/config.rs"}), &mut out);
	assert_eq!(out, vec!["src/config.rs"]);
}

#[test]
fn test_generate_file_context_content() {
	use std::io::Write;

	assert_eq!(
		generate_file_context_content(&[]),
		"No specific file context requested."
	);

	let mut temp = tempfile::NamedTempFile::new().expect("temp file");
	writeln!(temp, "line 1").expect("write");
	writeln!(temp, "line 2").expect("write");
	writeln!(temp, "line 3").expect("write");
	temp.flush().expect("flush");
	let path = temp.path().to_string_lossy().to_string();

	let content = generate_file_context_content(&[(path.clone(), 1, 2)]);
	assert!(content.contains("<content path="));
	assert!(content.contains("1:8b line 1"));
	assert!(content.contains("2:c8 line 2"));
	assert!(!content.contains("3:5f line 3"));

	// Invalid range (start 0) is skipped entirely
	assert_eq!(
		generate_file_context_content(&[(path, 0, 5)]),
		"No specific file context requested."
	);
}
