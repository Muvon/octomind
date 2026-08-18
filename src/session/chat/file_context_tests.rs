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

	// path + lines → ranged ref
	extract_file_refs_from_args(
		"view",
		&json!({"path": "a.rs", "lines": [10, 20]}),
		&mut out,
	);
	assert_eq!(out, vec!["a.rs:10:20"]);

	// path without lines → whole file
	out.clear();
	extract_file_refs_from_args("text_editor", &json!({"path": "t.rs"}), &mut out);
	assert_eq!(out, vec!["t.rs"]);

	// view with a paths array
	out.clear();
	extract_file_refs_from_args("view", &json!({"paths": ["x.rs", "y.rs"]}), &mut out);
	assert_eq!(out, vec!["x.rs", "y.rs"]);

	// extract_lines with from_path + from_range
	out.clear();
	extract_file_refs_from_args(
		"extract_lines",
		&json!({"from_path": "s.rs", "from_range": [1, 4]}),
		&mut out,
	);
	assert_eq!(out, vec!["s.rs:1:4"]);
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
	assert!(content.contains("1: line 1"));
	assert!(content.contains("2: line 2"));
	assert!(!content.contains("3: line 3"));

	// Invalid range (start 0) is skipped entirely
	assert_eq!(
		generate_file_context_content(&[(path, 0, 5)]),
		"No specific file context requested."
	);
}
