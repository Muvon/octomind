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

// File-context auto-expansion for compression summaries.
//
// AI emits file ranges via the schema's structured `file_context` array
// (see `conversation_compression::schema::FileContextEntry`). The compression
// path converts that array to tuples and calls `generate_file_context_content`
// here to read each range from disk and render it as XML — same auto-expansion
// behaviour as before, just without a text-parsing step in the middle.
//
// The legacy regex parser (`parse_file_contexts`) was removed once the schema
// rollout eliminated its sole production caller. The underlying file-reference
// parser still lives in `utils::file_parser` and is used by other code paths
// (chat-message file context via `file_renderer`).

use crate::utils::file_renderer::render_files_as_xml;

/// Generate file context content from parsed file requirements in XML format
pub fn generate_file_context_content(file_contexts: &[(String, usize, usize)]) -> String {
	if file_contexts.is_empty() {
		return "No specific file context requested.".to_string();
	}

	// Convert to new format and use XML renderer
	use crate::utils::file_parser::{read_file_lines, LineRange};
	use std::collections::HashMap;

	let mut file_contents = HashMap::new();

	for (filepath, start_line, end_line) in file_contexts {
		if let Some(range) = LineRange::new(*start_line, *end_line) {
			let content = read_file_lines(filepath, &range);
			file_contents
				.entry(filepath.clone())
				.or_insert_with(Vec::new)
				.push(content);
		}
	}

	render_files_as_xml(&file_contents)
}

/// octofs tools whose arguments name the files and lines a call read or changed.
const FILE_TOOLS: [&str; 4] = ["view", "symbol", "text_edit", "batch_edit"];
/// Arguments that split one call into parts, each with its own file or lines:
/// `view` spans, `batch_edit` operations, `text_edit` replacements.
const PART_KEYS: [&str; 3] = ["ranges", "operations", "edits"];

/// Extract file references from tool call arguments
/// Returns raw refs (path or path:start:end) that need merging
pub fn extract_file_refs_from_args(tool_name: &str, args: &serde_json::Value, refs: &mut Vec<String>) {
	if !FILE_TOOLS.contains(&tool_name) {
		return;
	}

	// `view` names several files in one call as `a.rs|b.rs`.
	let call_paths: Vec<&str> = args
		.get("path")
		.and_then(|p| p.as_str())
		.map(|p| p.split('|').map(str::trim).collect())
		.unwrap_or_default();
	let parts = PART_KEYS
		.iter()
		.find_map(|key| args.get(*key).and_then(|v| v.as_array()))
		.filter(|parts| !parts.is_empty());

	let Some(parts) = parts else {
		refs.extend(call_paths.iter().map(|path| file_ref(path, args)));
		return;
	};

	for part in parts {
		match part.get("path").and_then(|p| p.as_str()) {
			Some(path) => refs.push(file_ref(path, part)),
			None => refs.extend(call_paths.iter().map(|path| file_ref(path, part))),
		}
	}
}

/// `path:start:end` when the arguments carry both line ends, else the whole file.
fn file_ref(path: &str, args: &serde_json::Value) -> String {
	match (
		args.get("start").and_then(line_number),
		args.get("end").and_then(line_number),
	) {
		(Some(start), Some(end)) => format!("{}:{}:{}", path, start, end),
		_ => path.to_string(),
	}
}

/// Line number of a `start`/`end` value: an integer or a line id (`12:a3`).
/// A negative integer counts from the file end, so it names no fixed line.
fn line_number(value: &serde_json::Value) -> Option<u64> {
	match value {
		serde_json::Value::Number(number) => number.as_u64(),
		serde_json::Value::String(id) => id.split(':').next()?.parse().ok(),
		_ => None,
	}
}

/// Merge overlapping file ranges to produce compact references
/// Input: ["src/main.rs:10:50", "src/main.rs:30:100", "src/main.rs:200:250"]
/// Output: ["src/main.rs:10:100", "src/main.rs:200:250"]
pub fn merge_file_refs(refs: &[String]) -> Vec<String> {
	use std::collections::{BTreeMap, BTreeSet};

	let mut by_file: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
	let mut whole_files: BTreeSet<String> = BTreeSet::new();

	for ref_str in refs {
		let parts: Vec<&str> = ref_str.split(':').collect();
		match parts.len() {
			1 => {
				whole_files.insert(parts[0].to_string());
			}
			3 => {
				if let (Ok(start), Ok(end)) = (parts[1].parse::<u64>(), parts[2].parse::<u64>()) {
					by_file.entry(parts[0].to_string()).or_default().push((start, end));
				}
			}
			_ => {}
		}
	}

	let mut merged: Vec<String> = Vec::new();

	// Add whole files first (they supersede any ranges)
	for path in whole_files {
		by_file.remove(&path);
		merged.push(path);
	}

	// Merge overlapping ranges per file
	for (path, mut ranges) in by_file {
		if ranges.is_empty() {
			continue;
		}

		// Sort by start
		ranges.sort_by_key(|r| r.0);

		// Merge overlapping/adjacent ranges
		let mut merged_ranges: Vec<(u64, u64)> = Vec::new();
		for (start, end) in ranges {
			if let Some(last) = merged_ranges.last_mut() {
				// Overlap or adjacent (within 10 lines - merge small gaps)
				if start <= last.1 + 10 {
					last.1 = last.1.max(end);
					continue;
				}
			}
			merged_ranges.push((start, end));
		}

		// Convert to strings
		for (start, end) in merged_ranges {
			merged.push(format!("{}:{}:{}", path, start, end));
		}
	}

	merged
}

#[cfg(test)]
#[path = "file_context_tests.rs"]
mod tests;
