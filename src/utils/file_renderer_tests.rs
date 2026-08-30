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
use crate::utils::file_parser::{FileContent, LineRange};
use std::collections::HashMap;

fn create_test_file_content(
	path: &str,
	start: usize,
	end: usize,
	lines: Vec<&str>,
	error: Option<&str>,
) -> FileContent {
	FileContent {
		path: path.to_string(),
		lines: lines.into_iter().map(|s| s.to_string()).collect(),
		line_range: LineRange::new(start, end).unwrap(),
		error: error.map(|s| s.to_string()),
	}
}

#[test]
fn test_expand_context_blocks() {
	use std::io::Write;
	use tempfile::NamedTempFile;

	// Create a temporary file for testing
	let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");
	writeln!(temp_file, "line 1").expect("Failed to write to temp file");
	writeln!(temp_file, "line 2").expect("Failed to write to temp file");
	writeln!(temp_file, "line 3").expect("Failed to write to temp file");
	writeln!(temp_file, "line 4").expect("Failed to write to temp file");
	writeln!(temp_file, "line 5").expect("Failed to write to temp file");

	// Ensure file is flushed and readable
	temp_file.flush().expect("Failed to flush temp file");

	// Get the path as string - keep original format for Windows compatibility
	let temp_path = temp_file.path().to_string_lossy().to_string();

	// Test basic context expansion
	let input = format!(
		"Some text before\n<context>\n{}:1:2\n</context>\nSome text after",
		temp_path
	);
	let result = expand_context_blocks(&input);

	// Debug output for Windows troubleshooting
	#[cfg(debug_assertions)]
	{
		eprintln!("Input: {}", input);
		eprintln!("Result: {}", result);
		eprintln!("Temp path: {}", temp_path);
	}

	// Should preserve text outside context blocks
	assert!(
		result.contains("Some text before"),
		"Result should contain 'Some text before'. Result: {}",
		result
	);
	assert!(
		result.contains("Some text after"),
		"Result should contain 'Some text after'. Result: {}",
		result
	);

	// Should have expanded XML content
	assert!(
		result.contains("<content path="),
		"Result should contain '<content path='. Result: {}",
		result
	);
	assert!(
		result.contains("lines=\"1:2\""),
		"Result should contain 'lines=\"1:2\"'. Result: {}",
		result
	);

	// Check for line content with more flexible matching (handle different line endings)
	let has_line1 = result.contains("1: line 1") || result.contains("1: line 1\r");
	let has_line2 = result.contains("2: line 2") || result.contains("2: line 2\r");
	let has_line3 = result.contains("3: line 3") || result.contains("3: line 3\r");

	assert!(
		has_line1,
		"Result should contain '1: line 1'. Result: {}",
		result
	);
	assert!(
		has_line2,
		"Result should contain '2: line 2'. Result: {}",
		result
	);
	assert!(
		!has_line3,
		"Result should not contain '3: line 3'. Result: {}",
		result
	);

	// Original context tags should be replaced
	assert!(
		!result.contains("<context>"),
		"Result should not contain '<context>'. Result: {}",
		result
	);
	assert!(
		!result.contains("</context>"),
		"Result should not contain '</context>'. Result: {}",
		result
	);

	// Test multiple context blocks
	let input_multi = format!(
		"Text1\n<context>\n{}:1:1\n</context>\nText2\n<context>\n{}:3:4\n</context>\nText3",
		temp_path, temp_path
	);
	let result_multi = expand_context_blocks(&input_multi);

	assert!(result_multi.contains("Text1"));
	assert!(result_multi.contains("Text2"));
	assert!(result_multi.contains("Text3"));

	// Check for line content with flexible matching
	let multi_has_line1 =
		result_multi.contains("1: line 1") || result_multi.contains("1: line 1\r");
	let multi_has_line2 =
		result_multi.contains("2: line 2") || result_multi.contains("2: line 2\r");
	let multi_has_line3 =
		result_multi.contains("3: line 3") || result_multi.contains("3: line 3\r");
	let multi_has_line4 =
		result_multi.contains("4: line 4") || result_multi.contains("4: line 4\r");

	assert!(
		multi_has_line1,
		"Multi result should contain '1: line 1'. Result: {}",
		result_multi
	);
	assert!(
		multi_has_line3,
		"Multi result should contain '3: line 3'. Result: {}",
		result_multi
	);
	assert!(
		multi_has_line4,
		"Multi result should contain '4: line 4'. Result: {}",
		result_multi
	);
	assert!(
		!multi_has_line2,
		"Multi result should not contain '2: line 2'. Result: {}",
		result_multi
	);

	// Test empty context block
	let input_empty = "Text before <context></context> Text after";
	let result_empty = expand_context_blocks(input_empty);
	assert_eq!(result_empty, "Text before  Text after");

	// Test no context blocks
	let input_none = "No context blocks here";
	let result_none = expand_context_blocks(input_none);
	assert_eq!(result_none, input_none);

	// Test invalid file reference in context
	let input_invalid = "Text <context>nonexistent.rs:1:5</context> More";
	let result_invalid = expand_context_blocks(input_invalid);
	// Should still have the error message in XML format
	assert!(result_invalid.contains("Text "));
	assert!(result_invalid.contains(" More"));
	assert!(result_invalid.contains("error=\"true\""));
}

#[test]
fn test_expand_at_file_refs() {
	use std::io::Write;
	use tempfile::NamedTempFile;

	let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");
	writeln!(temp_file, "line 1").expect("Failed to write to temp file");
	writeln!(temp_file, "line 2").expect("Failed to write to temp file");
	temp_file.flush().expect("Failed to flush temp file");
	let temp_path = temp_file.path().to_string_lossy().to_string();

	// Existing file: mention stays inline, content appended as XML
	let input = format!("check @{} please", temp_path);
	let result = expand_at_file_refs(&input);
	assert!(result.starts_with(&input));
	assert!(result.contains("FILE CONTEXT:"));
	assert!(result.contains(&format!("<content path=\"{}\" lines=\"1:2\">", temp_path)));
	assert!(result.contains("1: line 1"));
	assert!(result.contains("2: line 2"));

	// Nonexistent path and bare @ are left untouched
	let untouched = "see @nonexistent/file.rs and a lone @ here";
	assert_eq!(expand_at_file_refs(untouched), untouched);

	// Duplicate mentions inject the file once
	let dup_input = format!("@{} and again @{}", temp_path, temp_path);
	let dup_result = expand_at_file_refs(&dup_input);
	assert_eq!(dup_result.matches("<content path=").count(), 1);
}

#[test]
fn test_render_files_as_xml() {
	let mut file_contents = HashMap::new();
	let content = create_test_file_content(
		"src/main.rs",
		1,
		3,
		vec!["1: fn main() {", "2:     println!(\"Hello\");", "3: }"],
		None,
	);
	file_contents.insert("src/main.rs".to_string(), vec![content]);

	let result = render_files_as_xml(&file_contents);

	println!("Actual result:\n{}", result); // Debug output

	assert!(result.contains("FILE CONTEXT:"));
	assert!(result.contains("<content path=\"src/main.rs\" lines=\"1:3\">"));
	assert!(result.contains("1: fn main() {"));
	assert!(result.contains("2:     println!(&quot;Hello&quot;);"));
	assert!(result.contains("3: }"));
	assert!(result.contains("</content>"));
}

#[test]
fn test_render_files_as_text() {
	let mut file_contents = HashMap::new();
	let content = create_test_file_content(
		"src/main.rs",
		1,
		3,
		vec!["1: fn main() {", "2:     println!(\"Hello\");", "3: }"],
		None,
	);
	file_contents.insert("src/main.rs".to_string(), vec![content]);

	let result = render_files_as_text(&file_contents);

	assert!(result.contains("FILE CONTEXT:"));
	assert!(result.contains("=== src/main.rs (lines 1-3) ==="));
	assert!(result.contains("1: fn main() {"));
	assert!(result.contains("2:     println!(\"Hello\");"));
	assert!(result.contains("3: }"));
}

#[test]
fn test_xml_escaping() {
	let mut file_contents = HashMap::new();
	let content = create_test_file_content(
		"src/test.rs",
		1,
		1,
		vec!["1: let x = \"<test>\" & 'value';"],
		None,
	);
	file_contents.insert("src/test.rs".to_string(), vec![content]);

	let result = render_files_as_xml(&file_contents);

	assert!(result.contains("&lt;test&gt;"));
	assert!(result.contains("&amp;"));
	assert!(result.contains("&#39;value&#39;"));
	assert!(result.contains("&quot;"));
}

#[test]
fn test_render_error_xml() {
	let mut file_contents = HashMap::new();
	let content = create_test_file_content(
		"missing.rs",
		1,
		10,
		vec![],
		Some("File not found: missing.rs"),
	);
	file_contents.insert("missing.rs".to_string(), vec![content]);

	let result = render_files_as_xml(&file_contents);

	assert!(result.contains("<content path=\"missing.rs\" lines=\"1:10\" error=\"true\">"));
	assert!(result.contains("File not found: missing.rs"));
	assert!(result.contains("</content>"));
}

#[test]
fn test_render_error_text() {
	let mut file_contents = HashMap::new();
	let content = create_test_file_content(
		"missing.rs",
		1,
		10,
		vec![],
		Some("File not found: missing.rs"),
	);
	file_contents.insert("missing.rs".to_string(), vec![content]);

	let result = render_files_as_text(&file_contents);

	assert!(result.contains("=== missing.rs (lines 1-10) ==="));
	assert!(result.contains("// File not found: missing.rs"));
}

#[test]
fn test_multiple_files_sorted() {
	let mut file_contents = HashMap::new();

	let content1 = create_test_file_content("z_file.rs", 1, 1, vec!["1: last"], None);
	let content2 = create_test_file_content("a_file.rs", 1, 1, vec!["1: first"], None);

	file_contents.insert("z_file.rs".to_string(), vec![content1]);
	file_contents.insert("a_file.rs".to_string(), vec![content2]);

	let result = render_files_as_xml(&file_contents);

	// Should be sorted alphabetically
	let a_pos = result.find("a_file.rs").unwrap();
	let z_pos = result.find("z_file.rs").unwrap();
	assert!(a_pos < z_pos);
}

#[test]
fn test_single_line_range() {
	let mut file_contents = HashMap::new();
	let content = create_test_file_content("test.rs", 5, 5, vec!["5: single line"], None);
	file_contents.insert("test.rs".to_string(), vec![content]);

	let result = render_files_as_xml(&file_contents);

	// Single line should show as "5" not "5:5"
	assert!(result.contains("lines=\"5\""));
	assert!(!result.contains("lines=\"5:5\""));
}

#[test]
fn test_merge_line_ranges() {
	let ranges = vec![
		LineRange::new(1, 5).unwrap(),
		LineRange::new(3, 8).unwrap(),   // Overlaps with first
		LineRange::new(10, 15).unwrap(), // Adjacent (within 5 lines)
		LineRange::new(25, 30).unwrap(), // Separate
	];

	let merged = merge_line_ranges(&ranges);

	assert_eq!(merged.len(), 2);
	assert_eq!(merged[0], LineRange::new(1, 15).unwrap()); // Merged first three
	assert_eq!(merged[1], LineRange::new(25, 30).unwrap()); // Separate
}

#[test]
fn test_render_with_custom_options() {
	let mut file_contents = HashMap::new();
	let content = create_test_file_content("test.rs", 1, 2, vec!["1: line1", "2: line2"], None);
	file_contents.insert("test.rs".to_string(), vec![content]);

	let options = RenderOptions {
		format: RenderFormat::Xml,
		show_line_numbers: true,
		include_header: false,
	};

	let result = render_files_with_options(&file_contents, &options);

	assert!(!result.contains("FILE CONTEXT:"));
	assert!(result.contains("<content path=\"test.rs\""));
}

#[test]
fn test_empty_file_contents() {
	let file_contents = HashMap::new();
	let result = render_files_as_xml(&file_contents);

	assert_eq!(result, "No specific file context requested.");
}
