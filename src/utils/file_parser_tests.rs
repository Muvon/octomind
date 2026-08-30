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
use std::fs;
use std::io::Write;
use tempfile::TempDir;

fn create_test_file(dir: &TempDir, name: &str, content: &str) -> String {
	let file_path = dir.path().join(name);
	let mut file = fs::File::create(&file_path).unwrap();
	writeln!(file, "{}", content).unwrap();
	file_path.to_string_lossy().to_string()
}

#[test]
fn test_has_context_blocks() {
	// Test with valid context blocks
	assert!(has_context_blocks("<context>src/main.rs:1:10</context>"));
	assert!(has_context_blocks(
		"Some text <context>file.rs:5:15</context> more text"
	));
	assert!(has_context_blocks(
		"<context>\nsrc/main.rs:1:10\nsrc/lib.rs:20:30\n</context>"
	));

	// Test multiple context blocks
	assert!(has_context_blocks(
		"<context>file1.rs:1:5</context> and <context>file2.rs:10:20</context>"
	));

	// Test without context blocks
	assert!(!has_context_blocks("No context blocks here"));
	assert!(!has_context_blocks("src/main.rs:1:10"));
	assert!(!has_context_blocks("Some text without context"));
	assert!(!has_context_blocks(""));

	// Test incomplete/malformed context blocks (should not match)
	assert!(!has_context_blocks("<context>malformed"));
	assert!(!has_context_blocks("malformed</context>"));
	assert!(!has_context_blocks("<context>incomplete"));
	assert!(!has_context_blocks("incomplete</context>"));
}

#[test]
fn test_parse_file_references_code_block() {
	let content = r#"
## REQUIRED FILE CONTEXTS
List ALL files needed as context to continue work. Use EXACT format:
```
src/main.rs:1:50
src/lib.rs:100:150
config/settings.toml:10:20
```
        "#;

	let refs = parse_file_references(content);
	assert_eq!(refs.len(), 3);

	assert_eq!(refs["src/main.rs"].len(), 1);
	assert_eq!(refs["src/main.rs"][0], LineRange { start: 1, end: 50 });

	assert_eq!(refs["src/lib.rs"].len(), 1);
	assert_eq!(
		refs["src/lib.rs"][0],
		LineRange {
			start: 100,
			end: 150
		}
	);

	assert_eq!(refs["config/settings.toml"].len(), 1);
	assert_eq!(
		refs["config/settings.toml"][0],
		LineRange { start: 10, end: 20 }
	);
}

#[test]
fn test_parse_file_references_section() {
	let content = r#"
## REQUIRED FILE CONTEXTS
The following files need context:
- src/session/mod.rs:200:300
- tests/integration.rs:1:100

## NEXT STEPS
Continue with implementation...
        "#;

	let refs = parse_file_references(content);
	assert_eq!(refs.len(), 2);

	assert_eq!(
		refs["src/session/mod.rs"][0],
		LineRange {
			start: 200,
			end: 300
		}
	);
	assert_eq!(
		refs["tests/integration.rs"][0],
		LineRange { start: 1, end: 100 }
	);
}

#[test]
fn test_parse_file_references_fallback() {
	let content = r#"
We need to look at src/core.rs:50:100 and also check lib/utils.rs:1:25 for the implementation.
        "#;

	let refs = parse_file_references(content);
	assert_eq!(refs.len(), 2);

	assert_eq!(
		refs["src/core.rs"][0],
		LineRange {
			start: 50,
			end: 100
		}
	);
	assert_eq!(refs["lib/utils.rs"][0], LineRange { start: 1, end: 25 });
}

#[test]
fn test_parse_file_references_invalid_ranges() {
	let content = r#"
```
src/main.rs:0:50
src/lib.rs:100:50
src/test.rs:1:20000
```
        "#;

	let refs = parse_file_references(content);
	// Should filter out invalid ranges (start=0, end<start, end>10000)
	assert_eq!(refs.len(), 0);
}

#[test]
fn test_line_range_validation() {
	assert!(LineRange::new(1, 10).is_some());
	assert!(LineRange::new(0, 10).is_none()); // start=0 invalid
	assert!(LineRange::new(10, 5).is_none()); // end<start invalid
	assert!(LineRange::new(1, 20000).is_none()); // end>10000 invalid
}

#[test]
fn test_read_file_lines() {
	let temp_dir = TempDir::new().unwrap();
	let file_path = create_test_file(&temp_dir, "test.txt", "line1\nline2\nline3\nline4\nline5");

	let range = LineRange::new(2, 4).unwrap();
	let content = read_file_lines(&file_path, &range);

	assert!(content.error.is_none());
	assert_eq!(content.lines.len(), 3);
	assert_eq!(content.lines[0], "2: line2");
	assert_eq!(content.lines[1], "3: line3");
	assert_eq!(content.lines[2], "4: line4");
}

#[test]
fn test_read_file_lines_missing_file() {
	let range = LineRange::new(1, 10).unwrap();
	let content = read_file_lines("nonexistent.txt", &range);

	assert!(content.error.is_some());
	assert!(content.error.unwrap().contains("File not found"));
	assert!(content.lines.is_empty());
}

#[test]
fn test_read_multiple_files() {
	let temp_dir = TempDir::new().unwrap();
	let file1 = create_test_file(&temp_dir, "file1.txt", "line1\nline2\nline3");
	let file2 = create_test_file(&temp_dir, "file2.txt", "lineA\nlineB\nlineC");

	let mut file_refs = HashMap::new();
	file_refs.insert(file1.clone(), vec![LineRange::new(1, 2).unwrap()]);
	file_refs.insert(file2.clone(), vec![LineRange::new(2, 3).unwrap()]);

	let results = read_multiple_files(&file_refs);

	assert_eq!(results.len(), 2);
	assert_eq!(results[&file1].len(), 1);
	assert_eq!(results[&file1][0].lines.len(), 2);
	assert_eq!(results[&file2].len(), 1);
	assert_eq!(results[&file2][0].lines.len(), 2);
}

#[test]
fn test_duplicate_removal() {
	let content = r#"
```
src/main.rs:1:10
src/main.rs:1:10
src/main.rs:5:15
```
        "#;

	let refs = parse_file_references(content);
	assert_eq!(refs.len(), 1);
	assert_eq!(refs["src/main.rs"].len(), 2); // Duplicates removed
	assert_eq!(refs["src/main.rs"][0], LineRange { start: 1, end: 10 });
	assert_eq!(refs["src/main.rs"][1], LineRange { start: 5, end: 15 });
}

#[test]
fn test_parse_context_tags() {
	let content = r#"
## REQUIRED FILE CONTEXTS
<context>
src/session/chat/continuation.rs:100:200
src/config/mod.rs:50:100
tests/integration_test.rs:1:50
</context>
        "#;

	let refs = parse_file_references(content);
	assert_eq!(refs.len(), 3);
	assert_eq!(
		refs["src/session/chat/continuation.rs"][0],
		LineRange {
			start: 100,
			end: 200
		}
	);
	assert_eq!(
		refs["src/config/mod.rs"][0],
		LineRange {
			start: 50,
			end: 100
		}
	);
	assert_eq!(
		refs["tests/integration_test.rs"][0],
		LineRange { start: 1, end: 50 }
	);
}

#[test]
fn test_parse_context_tags_priority() {
	// Context tags should take priority over code blocks
	let content = r#"
<context>
src/main.rs:1:10
</context>

```
src/lib.rs:20:30
```
        "#;

	let refs = parse_file_references(content);
	// Should only parse context tags, not code blocks
	assert_eq!(refs.len(), 1);
	assert!(refs.contains_key("src/main.rs"));
	assert!(!refs.contains_key("src/lib.rs"));
}

#[test]
fn test_parse_context_tags_with_empty_lines() {
	let content = r#"
<context>
src/main.rs:1:10

src/lib.rs:20:30

</context>
        "#;

	let refs = parse_file_references(content);
	assert_eq!(refs.len(), 2);
	assert_eq!(refs["src/main.rs"][0], LineRange { start: 1, end: 10 });
	assert_eq!(refs["src/lib.rs"][0], LineRange { start: 20, end: 30 });
}

#[test]
fn test_parse_file_references_empty_and_malformed() {
	// Empty and ref-free content yields no references
	assert!(parse_file_references("").is_empty());
	assert!(parse_file_references("Just plain text, no refs").is_empty());

	// Malformed line numbers never match
	assert!(parse_file_references("src/main.rs:a:b").is_empty());
	assert!(parse_file_references("src/main.rs:1:").is_empty());
	assert!(parse_file_references("src/main.rs:-1:5").is_empty());
	assert!(parse_file_references("1:2").is_empty());
}

#[test]
fn test_parse_file_references_windows_paths() {
	// Backslash drive path inside <context> tags
	let content = r"<context>
C:\Users\dk\main.rs:10:20
</context>";
	let refs = parse_file_references(content);
	assert_eq!(refs.len(), 1);
	assert_eq!(
		refs[r"C:\Users\dk\main.rs"][0],
		LineRange { start: 10, end: 20 }
	);

	// Forward-slash drive path in free text (fallback pattern)
	let refs = parse_file_references(r"See C:/dev/project/src/main.rs:1:5 for details");
	assert_eq!(refs.len(), 1);
	assert_eq!(
		refs["C:/dev/project/src/main.rs"][0],
		LineRange { start: 1, end: 5 }
	);

	// Backslash drive path in free text (fallback pattern)
	let refs = parse_file_references(r"See C:\Users\dk\main.rs:5:10 for details");
	assert_eq!(refs.len(), 1);
	assert_eq!(
		refs[r"C:\Users\dk\main.rs"][0],
		LineRange { start: 5, end: 10 }
	);
}

#[test]
fn test_line_range_boundaries() {
	assert_eq!(LineRange::new(1, 1), Some(LineRange { start: 1, end: 1 }));
	assert_eq!(
		LineRange::new(1, 10000),
		Some(LineRange {
			start: 1,
			end: 10000
		})
	);
	assert!(LineRange::new(1, 10001).is_none());
	assert!(LineRange::new(10000, 10000).is_some());
}

#[test]
fn test_parse_file_references_ten_file_limit() {
	let mut content = String::from("<context>\n");
	for i in 1..=12 {
		content.push_str(&format!("file{:02}.rs:1:10\n", i));
	}
	content.push_str("</context>");

	let refs = parse_file_references(&content);
	assert_eq!(refs.len(), 10); // capped at 10 files for performance
}

#[test]
fn test_read_file_lines_range_beyond_eof() {
	let temp_dir = TempDir::new().unwrap();
	let file_path = create_test_file(&temp_dir, "short.txt", "line1\nline2\nline3");

	let content = read_file_lines(&file_path, &LineRange::new(2, 100).unwrap());

	// Reading past EOF just stops at the last line — not an error
	assert!(content.error.is_none());
	assert_eq!(content.lines.len(), 2);
	assert_eq!(content.lines[0], "2: line2");
	assert_eq!(content.lines[1], "3: line3");
}

#[test]
fn test_read_file_lines_single_line_range() {
	let temp_dir = TempDir::new().unwrap();
	let file_path = create_test_file(&temp_dir, "one.txt", "line1\nline2\nline3");

	let content = read_file_lines(&file_path, &LineRange::new(2, 2).unwrap());

	assert!(content.error.is_none());
	assert_eq!(content.lines, vec!["2: line2".to_string()]);
}

#[test]
fn test_read_multiple_files_missing_file_and_multiple_ranges() {
	let temp_dir = TempDir::new().unwrap();
	let existing = create_test_file(&temp_dir, "exists.txt", "line1\nline2\nline3\nline4");

	let mut file_refs = HashMap::new();
	file_refs.insert(
		existing.clone(),
		vec![LineRange::new(1, 2).unwrap(), LineRange::new(3, 4).unwrap()],
	);
	file_refs.insert(
		"does_not_exist.txt".to_string(),
		vec![LineRange::new(1, 10).unwrap()],
	);

	let results = read_multiple_files(&file_refs);

	assert_eq!(results.len(), 2);
	assert_eq!(results[&existing].len(), 2);
	assert_eq!(
		results[&existing][0].lines,
		vec!["1: line1".to_string(), "2: line2".to_string()]
	);
	assert_eq!(
		results[&existing][1].lines,
		vec!["3: line3".to_string(), "4: line4".to_string()]
	);

	let missing = &results["does_not_exist.txt"];
	assert_eq!(missing.len(), 1);
	assert!(missing[0]
		.error
		.as_deref()
		.unwrap_or_default()
		.contains("File not found"));
	assert!(missing[0].lines.is_empty());
}
