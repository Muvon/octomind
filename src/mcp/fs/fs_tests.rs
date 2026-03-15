#[cfg(test)]
mod tests {
	use crate::mcp::fs::core::{execute_batch_edit, execute_extract_lines, execute_view};

	use crate::mcp::McpToolCall;
	use serde_json::json;
	use tempfile::NamedTempFile;
	use tokio::fs;

	async fn create_test_file(content: &str) -> NamedTempFile {
		let temp_file = NamedTempFile::new().unwrap();
		fs::write(temp_file.path(), content).await.unwrap();
		temp_file
	}

	// Helper: run a single-replace batch_edit and assert file content
	async fn test_batch_replace(
		content: &str,
		start_line: usize,
		end_line: usize,
		new_str: &str,
		expected: &str,
	) {
		let temp_file = create_test_file(content).await;
		let path = temp_file.path().to_string_lossy().to_string();
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": path,
				"operations": [{
					"operation": "replace",
					"line_range": [start_line, end_line],
					"content": new_str
				}]
			}),
		};
		let result = execute_batch_edit(&call).await.unwrap();
		assert!(
			result.result.get("error").is_none(),
			"Expected success: {:?}",
			result.result
		);
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		assert_eq!(actual, expected, "Content mismatch");
	}

	#[tokio::test]
	async fn test_replace_single_line() {
		test_batch_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"REPLACED",
			"line 1\nREPLACED\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_multiple_lines() {
		test_batch_replace(
			"line 1\nline 2\nline 3\nline 4\n",
			2,
			3,
			"SINGLE REPLACEMENT",
			"line 1\nSINGLE REPLACEMENT\nline 4\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_with_multiline_content() {
		test_batch_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"FIRST\nSECOND",
			"line 1\nFIRST\nSECOND\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_first_line() {
		test_batch_replace(
			"line 1\nline 2\nline 3\n",
			1,
			1,
			"NEW FIRST",
			"NEW FIRST\nline 2\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_last_line() {
		test_batch_replace(
			"line 1\nline 2\nline 3\n",
			3,
			3,
			"NEW LAST",
			"line 1\nline 2\nNEW LAST\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_all_lines() {
		test_batch_replace(
			"line 1\nline 2\nline 3\n",
			1,
			3,
			"EVERYTHING REPLACED",
			"EVERYTHING REPLACED\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_no_final_newline() {
		test_batch_replace(
			"line 1\nline 2\nline 3",
			2,
			2,
			"REPLACED",
			"line 1\nREPLACED\nline 3",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_crlf_line_endings() {
		// CRLF files: batch_edit normalises to LF on write
		test_batch_replace(
			"line 1\r\nline 2\r\nline 3\r\n",
			2,
			2,
			"REPLACED",
			"line 1\nREPLACED\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_empty_content_deletes_lines() {
		// Empty content removes the targeted lines entirely
		test_batch_replace("line 1\nline 2\nline 3\n", 2, 2, "", "line 1\nline 3\n").await;
	}

	#[tokio::test]
	async fn test_replace_single_line_file() {
		test_batch_replace("only line", 1, 1, "REPLACED", "REPLACED").await;
	}

	#[tokio::test]
	async fn test_replace_unicode() {
		test_batch_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"🚀 Hello 世界 🎉",
			"line 1\n🚀 Hello 世界 🎉\nline 3\n",
		)
		.await;
	}
	#[tokio::test]
	async fn test_replace_special_chars() {
		test_batch_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"!@#$%^&*()[]{}|;':\",./<>?",
			"line 1\n!@#$%^&*()[]{}|;':\",./<>?\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_content_with_quotes() {
		test_batch_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"\"quoted value\"",
			"line 1\n\"quoted value\"\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_content_with_tabs() {
		test_batch_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"\tindented line",
			"line 1\n\tindented line\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_content_with_embedded_newlines() {
		test_batch_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"hello\nworld\ntest",
			"line 1\nhello\nworld\ntest\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_no_false_positive_on_structural_noise() {
		// Lines that are pure structural noise (}, );, ], etc.) must NOT trigger
		// duplicate detection even when they appear at the boundary of the range.
		let content = "fn foo() {\n\tlet x = 1;\n}\nfn bar() {\n\tlet y = 2;\n}\n";
		let temp_file = create_test_file(content).await;
		let path = temp_file.path().to_string_lossy().to_string();
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": path,
				"operations": [{
					"operation": "replace",
					"line_range": [4, 6],
					// First line of content is `}` — same as line 3 (just before range).
					// Must NOT be blocked because `}` is structural noise.
					"content": "}\nfn bar() {\n\tlet y = 99;\n}"
				}]
			}),
		};
		let result = execute_batch_edit(&call).await.unwrap();
		assert_eq!(
			result.result["isError"], false,
			"}} boundary must not trigger duplicate detection: {:?}",
			result.result
		);
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		assert_eq!(
			actual,
			"fn foo() {\n\tlet x = 1;\n}\n}\nfn bar() {\n\tlet y = 99;\n}\n"
		);
	}

	#[tokio::test]
	async fn test_replace_duplicate_detection_before() {
		// Blocks write when first content line duplicates the line just before the range
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\n").await;
		let path = temp_file.path().to_string_lossy().to_string();
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": path,
				"operations": [{
					"operation": "replace",
					"line_range": [3, 4],
					"content": "line 2\nnew line 3\nnew line 4"
				}]
			}),
		};
		let result = execute_batch_edit(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&serde_json::json!(true)));
		let error_msg = result.result["content"][0]["text"].as_str().unwrap();
		assert!(error_msg.contains("Duplicate line detected"));
		// File must be unchanged
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		assert_eq!(actual, "line 1\nline 2\nline 3\nline 4\n");
	}

	#[tokio::test]
	async fn test_replace_duplicate_detection_after() {
		// Blocks write when last content line duplicates the line just after the range
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\n").await;
		let path = temp_file.path().to_string_lossy().to_string();
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": path,
				"operations": [{
					"operation": "replace",
					"line_range": [1, 2],
					"content": "new line 1\nnew line 2\nline 3"
				}]
			}),
		};
		let result = execute_batch_edit(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&serde_json::json!(true)));
		let error_msg = result.result["content"][0]["text"].as_str().unwrap();
		assert!(error_msg.contains("Duplicate line detected"));
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		assert_eq!(actual, "line 1\nline 2\nline 3\nline 4\n");
	}

	#[tokio::test]
	async fn test_replace_no_false_duplicate_warning() {
		// Genuinely different content must not be blocked
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\n").await;
		let path = temp_file.path().to_string_lossy().to_string();
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": path,
				"operations": [{
					"operation": "replace",
					"line_range": [2, 3],
					"content": "new line 2\nnew line 3"
				}]
			}),
		};
		let result = execute_batch_edit(&call).await.unwrap();
		assert_eq!(
			result.result["isError"], false,
			"Expected success: {:?}",
			result.result
		);
		// Diff is returned as the text content of the result
		assert!(result.result["content"][0]["text"].as_str().is_some());
	}

	#[tokio::test]
	async fn test_replace_diff_output_present() {
		// batch_edit must return a diff field so the AI can verify the edit
		let temp_file = create_test_file("line 1\nline 2\nline 3\n").await;
		let path = temp_file.path().to_string_lossy().to_string();
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": path,
				"operations": [{
					"operation": "replace",
					"line_range": [2, 2],
					"content": "REPLACED"
				}]
			}),
		};
		let result = execute_batch_edit(&call).await.unwrap();
		assert_eq!(
			result.result["isError"], false,
			"Expected success: {:?}",
			result.result
		);
		let diff = result.result["content"][0]["text"]
			.as_str()
			.expect("diff must be present in content text");
		assert!(diff.contains("-2:"), "diff must show removed line");
		assert!(diff.contains("+2:"), "diff must show added line");
	}

	#[tokio::test]
	async fn test_replace_negative_line_index() {
		// Negative indices: -1 = last line
		let temp_file = create_test_file("line 1\nline 2\nline 3\n").await;
		let path = temp_file.path().to_string_lossy().to_string();
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": path,
				"operations": [{
					"operation": "replace",
					"line_range": [-1, -1],
					"content": "NEW LAST"
				}]
			}),
		};
		let result = execute_batch_edit(&call).await.unwrap();
		assert_eq!(
			result.result["isError"], false,
			"Expected success: {:?}",
			result.result
		);
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		assert_eq!(actual, "line 1\nline 2\nNEW LAST\n");
	}

	// ========== STR_REPLACE TESTS ==========

	async fn test_str_replace(content: &str, old_str: &str, new_str: &str, expected: &str) {
		let temp_file = create_test_file(content).await;
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "text_editor".to_string(),
			parameters: json!({}),
		};

		let result = crate::mcp::fs::text_editing::str_replace_spec(
			&call,
			temp_file.path(),
			old_str,
			new_str,
		)
		.await
		.unwrap();

		// Check that operation succeeded
		assert!(result.result.get("error").is_none());

		// Check file content
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		assert_eq!(actual, expected, "Content mismatch");
	}

	#[tokio::test]
	async fn test_str_replace_basic() {
		test_str_replace(
			"Hello world\nThis is a test\nGoodbye universe",
			"world",
			"universe",
			"Hello universe\nThis is a test\nGoodbye universe",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_multiline_old() {
		test_str_replace(
			"line 1\nline 2\nline 3\nline 4",
			"line 2\nline 3",
			"REPLACED",
			"line 1\nREPLACED\nline 4",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_multiline_new() {
		test_str_replace(
			"line 1\nREPLACE_ME\nline 3",
			"REPLACE_ME",
			"new line 1\nnew line 2",
			"line 1\nnew line 1\nnew line 2\nline 3",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_with_quotes() {
		test_str_replace(
			"let x = \"old_value\";",
			"\"old_value\"",
			"\"new_value\"",
			"let x = \"new_value\";",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_with_actual_newlines() {
		// Test replacing content that contains actual newlines
		test_str_replace(
			"hello\nworld\ntest",
			"hello\nworld",
			"goodbye\nuniverse",
			"goodbye\nuniverse\ntest",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_with_literal_backslash_n() {
		// Test replacing literal \n characters (not actual newlines)
		test_str_replace(
			"hello\\nworld\\ntest",
			"hello\\nworld",
			"goodbye\\nuniverse",
			"goodbye\\nuniverse\\ntest",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_with_tabs() {
		test_str_replace(
			"function() {\n\told_code();\n}",
			"\told_code();",
			"\tnew_code();\n\tmore_code();",
			"function() {\n\tnew_code();\n\tmore_code();\n}",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_with_special_chars() {
		test_str_replace(
			"regex = /[a-z]+/g;",
			"/[a-z]+/g",
			"/[A-Z]+/i",
			"regex = /[A-Z]+/i;",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_with_unicode() {
		test_str_replace("Hello 世界! 🚀", "世界", "Universe", "Hello Universe! 🚀").await;
	}

	#[tokio::test]
	async fn test_str_replace_windows_line_endings() {
		test_str_replace(
			"line 1\r\nline 2\r\nline 3\r\n",
			"line 2",
			"REPLACED",
			"line 1\r\nREPLACED\r\nline 3\r\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_complex_code() {
		let old_code = "fn old_function() {\n    println!(\"old\");\n}";
		let new_code = "fn new_function() {\n    println!(\"new\");\n    return 42;\n}";

		test_str_replace(
			"// Some comment\nfn old_function() {\n    println!(\"old\");\n}\n// End",
			old_code,
			new_code,
			"// Some comment\nfn new_function() {\n    println!(\"new\");\n    return 42;\n}\n// End",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_with_control_chars() {
		test_str_replace(
			"data\x00null\x01control",
			"\x00null\x01",
			"\x02new\x03",
			"data\x02new\x03control",
		)
		.await;
	}

	#[tokio::test]
	async fn test_str_replace_error_no_match() {
		let temp_file = create_test_file("Hello world").await;
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "text_editor".to_string(),
			parameters: json!({}),
		};

		let result = crate::mcp::fs::text_editing::str_replace_spec(
			&call,
			temp_file.path(),
			"not_found",
			"replacement",
		)
		.await
		.unwrap();

		// Should return error for no match
		assert!(
			result.result.get("isError").unwrap().as_bool().unwrap(),
			"Should have isError: true"
		);
		let content = result.result["content"].as_array().unwrap()[0]["text"]
			.as_str()
			.unwrap();
		assert!(
			content.contains("No match found"),
			"Should contain no match error message"
		);
	}

	#[tokio::test]
	async fn test_str_replace_error_multiple_matches() {
		let temp_file = create_test_file("test test test").await;
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "text_editor".to_string(),
			parameters: json!({}),
		};

		let result = crate::mcp::fs::text_editing::str_replace_spec(
			&call,
			temp_file.path(),
			"test",
			"replacement",
		)
		.await
		.unwrap();

		// Should return error for multiple matches
		assert!(
			result.result.get("isError").unwrap().as_bool().unwrap(),
			"Should have isError: true"
		);
		let content = result.result["content"].as_array().unwrap()[0]["text"]
			.as_str()
			.unwrap();
		assert!(
			content.contains("Found 3 matches"),
			"Should contain multiple matches error message"
		);
	}

	#[tokio::test]
	async fn test_str_replace_byte_level_verification() {
		let temp_file = create_test_file("hello\nworld\ntest").await;
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "text_editor".to_string(),
			parameters: json!({}),
		};

		// Replace with content containing actual newlines
		let result = crate::mcp::fs::text_editing::str_replace_spec(
			&call,
			temp_file.path(),
			"world",
			"new\nline",
		)
		.await
		.unwrap();

		// Check that operation succeeded
		assert!(result.result.get("error").is_none());

		// Read and verify byte content
		let actual_bytes = fs::read(temp_file.path()).await.unwrap();
		let expected_bytes = b"hello\nnew\nline\ntest";

		assert_eq!(actual_bytes, expected_bytes, "Byte-level content mismatch");

		// Verify the newline characters are actual newlines (byte value 10)
		assert_eq!(actual_bytes[5], 10u8, "First newline should be byte 10");
		assert_eq!(actual_bytes[9], 10u8, "Second newline should be byte 10");
		assert_eq!(actual_bytes[14], 10u8, "Third newline should be byte 10");
	}

	#[tokio::test]
	async fn test_list_files_basic_functionality() {
		use crate::mcp::fs::directory::list_directory;
		use std::fs;
		use tempfile::TempDir;

		// Create a temporary directory with many files
		let temp_dir = TempDir::new().unwrap();
		let temp_path = temp_dir.path();

		// Create 30 test files
		for i in 1..=30 {
			let file_path = temp_path.join(format!("test_file_{:02}.txt", i));
			fs::write(&file_path, format!("Content of file {}", i)).unwrap();
		}

		// Test basic file listing functionality
		let call = McpToolCall {
			tool_name: "view".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"pattern": "*.txt"
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result = list_directory(
			&call,
			call.parameters
				.get("directory")
				.and_then(|v| v.as_str())
				.unwrap_or("."),
		)
		.await
		.unwrap();
		let output = result.result.as_object().unwrap();

		// Should have basic file listing info (no tool-level truncation)
		assert_eq!(output["count"], 30); // Total count
		assert_eq!(output["displayed_count"], 30); // All files displayed (global truncation handles limits)
		assert_eq!(output["success"], true);
		assert_eq!(output["type"], "file listing");

		// Should have files array
		assert!(output.contains_key("files"));
		let files = output["files"].as_array().unwrap();
		assert_eq!(files.len(), 30);

		// Test with different pattern
		let call_limited = McpToolCall {
			tool_name: "view".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"pattern": "*_01.txt"
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result_limited = list_directory(
			&call_limited,
			call_limited
				.parameters
				.get("directory")
				.and_then(|v| v.as_str())
				.unwrap_or("."),
		)
		.await
		.unwrap();
		let output_limited = result_limited.result.as_object().unwrap();

		// Should find only one file matching the pattern
		assert_eq!(output_limited["count"], 1);
		assert_eq!(output_limited["displayed_count"], 1);

		let files_limited = output_limited["files"].as_array().unwrap();
		assert_eq!(files_limited.len(), 1);
		assert!(files_limited[0]
			.as_str()
			.unwrap()
			.contains("test_file_01.txt"));
	}

	#[tokio::test]
	async fn test_list_files_content_search_preserves_format() {
		use crate::mcp::fs::directory::list_directory;
		use std::fs;
		use tempfile::TempDir;

		// Create a temporary directory with test files containing searchable content
		let temp_dir = TempDir::new().unwrap();
		let temp_path = temp_dir.path();

		// Create test files with specific content
		let file1_path = temp_path.join("test1.rs");
		fs::write(
			&file1_path,
			"fn main() {\n    println!(\"Hello, world!\");\n    let x = 42;\n}\n",
		)
		.unwrap();

		let file2_path = temp_path.join("test2.rs");
		fs::write(&file2_path, "pub fn helper() {\n    println!(\"Helper function\");\n}\n\nfn main() {\n    helper();\n}\n").unwrap();

		// Test content search with line numbers
		let call = McpToolCall {
			tool_name: "view".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"content": "println!",
				"line_numbers": true,
				"max_lines": 0  // unlimited
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result = list_directory(
			&call,
			call.parameters
				.get("directory")
				.and_then(|v| v.as_str())
				.unwrap_or("."),
		)
		.await
		.unwrap();
		let output = result.result.as_object().unwrap();

		// Should be content search
		assert_eq!(output["type"], "content search");
		assert!(output["success"].as_bool().unwrap());

		// Should have lines (not files) for content search
		assert!(output.contains_key("lines"));
		assert!(output.contains_key("total_lines"));
		assert!(output.contains_key("displayed_lines"));

		// Check that output preserves ripgrep format with line numbers
		let output_str = output["output"].as_str().unwrap();
		println!("Content search output:\n{}", output_str);

		// Should contain filenames and line numbers (ripgrep format)
		assert!(output_str.contains("test1.rs:") || output_str.contains("test2.rs:"));

		// Test content search with context
		let call_with_context = McpToolCall {
			tool_name: "view".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"content": "println!",
				"line_numbers": true,
				"context": 1,
				"max_lines": 0
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result_with_context = list_directory(
			&call_with_context,
			call_with_context
				.parameters
				.get("directory")
				.and_then(|v| v.as_str())
				.unwrap_or("."),
		)
		.await
		.unwrap();
		let output_with_context = result_with_context.result.as_object().unwrap();

		let output_str_with_context = output_with_context["output"].as_str().unwrap();
		println!("Content search with context:\n{}", output_str_with_context);

		// With context, should have more lines
		let lines_no_context = output["lines"].as_array().unwrap().len();
		let lines_with_context = output_with_context["lines"].as_array().unwrap().len();
		assert!(
			lines_with_context >= lines_no_context,
			"Context should add more lines: {} vs {}",
			lines_with_context,
			lines_no_context
		);
	}

	#[tokio::test]
	async fn test_list_files_vs_content_search_different_output() {
		use crate::mcp::fs::directory::list_directory;
		use std::fs;
		use tempfile::TempDir;

		// Create a temporary directory with test files
		let temp_dir = TempDir::new().unwrap();
		let temp_path = temp_dir.path();

		// Create test files
		for i in 1..=5 {
			let file_path = temp_path.join(format!("test_{}.rs", i));
			fs::write(
				&file_path,
				format!("fn test_{}() {{\n    println!(\"Test {}\");\n}}\n", i, i),
			)
			.unwrap();
		}

		// Test 1: File listing (should return just filenames)
		let file_list_call = McpToolCall {
			tool_name: "view".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"pattern": "*.rs"
			}),
			tool_id: "test-call-id".to_string(),
		};

		let file_list_result = list_directory(
			&file_list_call,
			file_list_call
				.parameters
				.get("directory")
				.and_then(|v| v.as_str())
				.unwrap_or("."),
		)
		.await
		.unwrap();
		let file_list_output = file_list_result.result.as_object().unwrap();

		// Should be file listing
		assert_eq!(file_list_output["type"], "file listing");
		assert!(file_list_output.contains_key("files"));
		assert!(file_list_output.contains_key("count"));

		// Test 2: Content search (should return formatted matches)
		let content_search_call = McpToolCall {
			tool_name: "view".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"content": "println!"
			}),
			tool_id: "test-call-id".to_string(),
		};

		let content_search_result = list_directory(
			&content_search_call,
			content_search_call
				.parameters
				.get("directory")
				.and_then(|v| v.as_str())
				.unwrap_or("."),
		)
		.await
		.unwrap();
		let content_search_output = content_search_result.result.as_object().unwrap();

		// Should be content search
		assert_eq!(content_search_output["type"], "content search");
		assert!(content_search_output.contains_key("lines"));
		assert!(content_search_output.contains_key("total_lines"));

		// Outputs should be completely different
		let file_list_str = file_list_output["output"].as_str().unwrap();
		let content_search_str = content_search_output["output"].as_str().unwrap();

		println!("File listing output:\n{}", file_list_str);
		println!("Content search output:\n{}", content_search_str);

		// File listing should just be filenames
		assert!(file_list_str.contains("test_1.rs"));
		// Check that file listing doesn't contain line numbers
		// Look for line number patterns: either ":digit:" or newline followed by "digit:"
		// Use regex to avoid false positives from Windows drive letters (C:)
		let line_number_pattern = regex::Regex::new(r"(:\d+:|^\d+:)").unwrap();
		assert!(!line_number_pattern.is_match(file_list_str)); // No line numbers

		// Content search should have line numbers and content
		// Content search format is either "filename:line:content" or "filename:\nline:content"
		let has_line_numbers = content_search_str.contains("2:    println!")
			|| line_number_pattern.is_match(content_search_str);
		assert!(has_line_numbers); // Line numbers
		assert!(content_search_str.contains("println!")); // Actual content
	}

	// ===== EXTRACT_LINES TESTS =====

	async fn test_extract_lines(
		source_content: &str,
		from_range: (usize, usize),
		target_content: &str,
		append_line: i64,
		expected_target: &str,
	) {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let source_path = temp_dir.path().join("source.txt");
		let target_path = temp_dir.path().join("target.txt");

		// Create source file
		fs::write(&source_path, source_content).await.unwrap();

		// Create target file if it has content
		if !target_content.is_empty() {
			fs::write(&target_path, target_content).await.unwrap();
		}

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [from_range.0, from_range.1],
				"append_path": target_path.to_string_lossy(),
				"append_line": append_line
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();

		// Check that operation succeeded
		assert!(
			result.result.get("isError") == Some(&json!(false)),
			"Extract lines should succeed"
		);

		// Check source file unchanged
		let source_after = fs::read_to_string(&source_path).await.unwrap();
		assert_eq!(
			source_after, source_content,
			"Source file should be unchanged"
		);

		// Check target file content
		let target_after = fs::read_to_string(&target_path).await.unwrap();
		assert_eq!(
			target_after, expected_target,
			"Target file content mismatch"
		);
	}

	#[tokio::test]
	async fn test_extract_single_line_to_empty_file() {
		test_extract_lines("line 1\nline 2\nline 3\n", (2, 2), "", -1, "line 2").await;
	}

	#[tokio::test]
	async fn test_extract_multiple_lines_to_empty_file() {
		test_extract_lines(
			"line 1\nline 2\nline 3\nline 4\n",
			(2, 3),
			"",
			-1,
			"line 2\nline 3",
		)
		.await;
	}

	#[tokio::test]
	async fn test_extract_append_to_end() {
		test_extract_lines(
			"source 1\nsource 2\nsource 3\n",
			(1, 2),
			"existing 1\nexisting 2\n",
			-1,
			"existing 1\nexisting 2\nsource 1\nsource 2",
		)
		.await;
	}

	#[tokio::test]
	async fn test_extract_insert_at_beginning() {
		test_extract_lines(
			"new 1\nnew 2\n",
			(1, 2),
			"old 1\nold 2\n",
			0,
			"new 1\nnew 2\nold 1\nold 2\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_extract_insert_after_line() {
		test_extract_lines(
			"inserted 1\ninserted 2\n",
			(1, 2),
			"line 1\nline 2\nline 3\n",
			2,
			"line 1\nline 2\ninserted 1\ninserted 2\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_extract_first_line() {
		test_extract_lines("first\nsecond\nthird\n", (1, 1), "", -1, "first").await;
	}

	#[tokio::test]
	async fn test_extract_last_line() {
		test_extract_lines("first\nsecond\nlast\n", (3, 3), "", -1, "last\n").await;
	}

	#[tokio::test]
	async fn test_extract_all_lines() {
		test_extract_lines(
			"all 1\nall 2\nall 3\n",
			(1, 3),
			"",
			-1,
			"all 1\nall 2\nall 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_extract_lines_with_special_characters() {
		test_extract_lines(
			"fn main() {\n    println!(\"Hello, world!\");\n}\n",
			(1, 3),
			"",
			-1,
			"fn main() {\n    println!(\"Hello, world!\");\n}\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_extract_lines_error_invalid_range() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let source_path = temp_dir.path().join("source.txt");
		let target_path = temp_dir.path().join("target.txt");

		fs::write(&source_path, "line 1\nline 2\n").await.unwrap();

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [1, 5], // Line 5 doesn't exist
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert!(
			result.result.get("isError") == Some(&json!(true)),
			"Should fail with invalid range"
		);
		assert!(result.result["content"][0]["text"]
			.as_str()
			.unwrap()
			.contains("exceeds file length"));
	}

	#[tokio::test]
	async fn test_extract_lines_error_start_greater_than_end() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let source_path = temp_dir.path().join("source.txt");
		let target_path = temp_dir.path().join("target.txt");

		fs::write(&source_path, "line 1\nline 2\n").await.unwrap();

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [2, 1], // Start > end
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert!(
			result.result.get("isError") == Some(&json!(true)),
			"Should fail when start > end"
		);
		assert!(result.result["content"][0]["text"]
			.as_str()
			.unwrap()
			.contains("cannot be greater than"));
	}

	#[tokio::test]
	async fn test_extract_lines_error_missing_source_file() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let source_path = temp_dir.path().join("nonexistent.txt");
		let target_path = temp_dir.path().join("target.txt");

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [1, 1],
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert!(
			result.result.get("isError") == Some(&json!(true)),
			"Should fail with missing source file"
		);
		assert!(result.result["content"][0]["text"]
			.as_str()
			.unwrap()
			.contains("does not exist"));
	}

	#[tokio::test]
	async fn test_extract_lines_error_invalid_append_position() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let source_path = temp_dir.path().join("source.txt");
		let target_path = temp_dir.path().join("target.txt");

		fs::write(&source_path, "line 1\nline 2\n").await.unwrap();
		fs::write(&target_path, "existing\n").await.unwrap();

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [1, 1],
				"append_path": target_path.to_string_lossy(),
				"append_line": 5 // Position beyond file length
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert!(
			result.result.get("isError") == Some(&json!(true)),
			"Should fail with invalid append position"
		);
		assert!(result.result["content"][0]["text"]
			.as_str()
			.unwrap()
			.contains("exceeds target file length"));
	}

	#[tokio::test]
	async fn test_extract_lines_creates_parent_directories() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let source_path = temp_dir.path().join("source.txt");
		let target_path = temp_dir.path().join("nested/deep/target.txt");

		fs::write(&source_path, "content\n").await.unwrap();

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [1, 1],
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert!(
			result.result.get("isError") == Some(&json!(false)),
			"Should succeed creating parent directories"
		);

		// Check that target file was created with correct content
		let target_content = fs::read_to_string(&target_path).await.unwrap();
		assert_eq!(target_content, "content\n");
	}

	#[tokio::test]
	async fn test_extract_lines_parameter_validation() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let source_path = temp_dir.path().join("source.txt");
		let target_path = temp_dir.path().join("target.txt");

		fs::write(&source_path, "line 1\n").await.unwrap();

		// Test missing from_path
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_range": [1, 1],
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert!(
			result.result.get("isError") == Some(&json!(true)),
			"Should fail with missing from_path"
		);
		assert!(result.result["content"][0]["text"]
			.as_str()
			.unwrap()
			.contains("Missing required parameter 'from_path'"));

		// Test invalid from_range format
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [1], // Only one element
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert!(
			result.result.get("isError") == Some(&json!(true)),
			"Should fail with invalid from_range"
		);
		assert!(result.result["content"][0]["text"]
			.as_str()
			.unwrap()
			.contains("exactly 2 elements"));

		// Test empty from_path
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": "",
				"from_range": [1, 1],
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert!(
			result.result.get("isError") == Some(&json!(true)),
			"Should fail with empty from_path"
		);
		assert!(result.result["content"][0]["text"]
			.as_str()
			.unwrap()
			.contains("cannot be empty"));
	}

	// ===============================
	// BATCH_EDIT TESTS - NEW REVOLUTIONARY ARCHITECTURE
	// ===============================

	async fn create_batch_edit_call(path: &str, operations: serde_json::Value) -> McpToolCall {
		McpToolCall {
			tool_id: "test_batch_edit".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": path,
				"operations": operations
			}),
		}
	}

	#[tokio::test]
	async fn test_batch_edit_single_insert() {
		let temp_file = create_test_file("line 1\nline 2\nline 3\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "insert",
				"line_range": 2,
				"content": "inserted line"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Check success
		assert!(
			result.result.get("error").is_none(),
			"Operation should succeed"
		);

		// Verify file content
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		let expected = "line 1\nline 2\ninserted line\nline 3\n";
		assert_eq!(
			actual, expected,
			"Content should match expected after insert"
		);
	}

	#[tokio::test]
	async fn test_batch_edit_multiple_operations_original_line_numbers() {
		// Test the CORE FEATURE: all operations use ORIGINAL line numbers
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\nline 5\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "insert",
				"line_range": 1,
				"content": "inserted after line 1"
			},
			{
				"operation": "replace",
				"line_range": [3, 3],
				"content": "replaced original line 3"
			},
			{
				"operation": "insert",
				"line_range": 5,
				"content": "inserted after original line 5"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Check success
		assert!(
			result.result.get("error").is_none(),
			"Operation should succeed"
		);

		// Verify file content - operations applied in reverse order to maintain line stability
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		let expected = "line 1\ninserted after line 1\nline 2\nreplaced original line 3\nline 4\nline 5\ninserted after original line 5\n";
		assert_eq!(
			actual, expected,
			"Content should reflect all operations using original line numbers"
		);
	}

	#[tokio::test]
	async fn test_batch_edit_conflict_detection() {
		let temp_file = create_test_file("line 1\nline 2\nline 3\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		// Conflicting operations: insert after line 2 AND replace line 2
		let operations = json!([
			{
				"operation": "insert",
				"line_range": 2,
				"content": "inserted after line 2"
			},
			{
				"operation": "replace",
				"line_range": [2, 2],
				"content": "replaced line 2"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Check that it failed due to conflict
		assert!(
			result.result.get("content").is_some(),
			"Should have error content"
		);
		let content = result.result["content"].as_array().unwrap()[0]["text"]
			.as_str()
			.unwrap();
		assert!(
			content.contains("Conflicting operations"),
			"Should detect conflict"
		);
		assert!(
			content.contains("both affect line 2"),
			"Should specify conflicting line"
		);
	}

	#[tokio::test]
	async fn test_batch_edit_overlapping_replace_ranges() {
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\nline 5\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		// Overlapping replace ranges: [2,3] and [3,4]
		let operations = json!([
			{
				"operation": "replace",
				"line_range": [2, 3],
				"content": "replaced 2-3"
			},
			{
				"operation": "replace",
				"line_range": [3, 4],
				"content": "replaced 3-4"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Check that it failed due to overlap
		assert!(
			result.result.get("content").is_some(),
			"Should have error content"
		);
		let content = result.result["content"].as_array().unwrap()[0]["text"]
			.as_str()
			.unwrap();
		assert!(
			content.contains("Conflicting operations"),
			"Should detect overlap"
		);
		assert!(
			content.contains("both affect line 3"),
			"Should specify overlapping line"
		);
	}

	#[tokio::test]
	async fn test_batch_edit_missing_path() {
		let operations = json!([
			{
				"operation": "insert",
				"line_range": 1,
				"content": "test"
			}
		]);

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "text_editor".to_string(),
			parameters: json!({
				"command": "batch_edit",
				"operations": operations
				// Missing "path"
			}),
		};

		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Check that it failed due to missing path
		assert!(
			result.result.get("content").is_some(),
			"Should have error content"
		);
		let content = result.result["content"].as_array().unwrap()[0]["text"]
			.as_str()
			.unwrap();
		assert!(
			content.contains("Missing required 'path' parameter"),
			"Should indicate missing path"
		);
	}

	#[tokio::test]
	async fn test_batch_edit_invalid_operation_type() {
		let temp_file = create_test_file("line 1\nline 2\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "invalid_op",
				"line_range": 1,
				"content": "test"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Check that it failed due to invalid operation
		assert!(
			result.result.get("content").is_some(),
			"Should have error content"
		);
		let content = result.result["content"].as_array().unwrap()[0]["text"]
			.as_str()
			.unwrap();
		assert!(
			content.contains("No valid operations found"),
			"Should indicate no valid operations"
		);
		assert!(
			content.contains("operations failed during parsing"),
			"Should indicate parsing failure"
		);
	}

	#[tokio::test]
	async fn test_batch_edit_comprehensive_scenario() {
		// Test a comprehensive scenario with multiple operation types
		let temp_file = create_test_file(
			"# Header\nfunction main() {\n    console.log('hello');\n    return 0;\n}\n// Footer\n",
		)
		.await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "insert",
				"line_range": 1,
				"content": "// Added by batch_edit"
			},
			{
				"operation": "replace",
				"line_range": [3, 3],
				"content": "    console.log('Hello, World!');\n    console.log('Batch edit works!');"
			},
			{
				"operation": "insert",
				"line_range": 6,
				"content": "// End of file"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Check success
		assert!(
			result.result.get("error").is_none(),
			"Comprehensive operation should succeed"
		);

		// Verify file content
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		let expected = "# Header\n// Added by batch_edit\nfunction main() {\n    console.log('Hello, World!');\n    console.log('Batch edit works!');\n    return 0;\n}\n// Footer\n// End of file\n";
		assert_eq!(
			actual, expected,
			"Should handle comprehensive batch edit scenario"
		);

		// Check result details
		let batch_summary = &result.result["metadata"]["batch_summary"];
		assert_eq!(batch_summary["total_operations"], 3);
		assert_eq!(batch_summary["successful_operations"], 3);
		assert_eq!(batch_summary["failed_operations"], 0);
		assert_eq!(batch_summary["overall_success"], true);
	}

	#[tokio::test]
	async fn test_batch_edit_with_undo_functionality() {
		// Test that batch_edit properly stores history for undo functionality
		let temp_file =
			create_test_file("original line 1\noriginal line 2\noriginal line 3\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		// Perform batch edit operations
		let operations = json!([
			{
				"operation": "insert",
				"line_range": 1,
				"content": "inserted after line 1"
			},
			{
				"operation": "replace",
				"line_range": [3, 3],
				"content": "replaced original line 3"
			}
		]);

		let batch_call = create_batch_edit_call(&path, operations).await;
		let batch_result = crate::mcp::fs::core::execute_batch_edit(&batch_call)
			.await
			.unwrap();

		// Check batch edit succeeded
		assert!(
			batch_result.result.get("error").is_none(),
			"Batch edit should succeed"
		);

		// Verify file content after batch edit
		let content_after_batch = fs::read_to_string(temp_file.path()).await.unwrap();
		let expected_after_batch =
			"original line 1\ninserted after line 1\noriginal line 2\nreplaced original line 3\n";
		assert_eq!(
			content_after_batch, expected_after_batch,
			"Content should reflect batch edit changes"
		);

		// Now test undo functionality
		let undo_call = McpToolCall {
			tool_id: "test_undo".to_string(),
			tool_name: "text_editor".to_string(),
			parameters: json!({
				"command": "undo_edit",
				"path": path
			}),
		};

		let undo_result = crate::mcp::fs::core::undo_edit(&undo_call, temp_file.path())
			.await
			.unwrap();

		// Check undo succeeded
		assert!(
			undo_result.result.get("error").is_none(),
			"Undo should succeed"
		);

		// Verify file content is restored to original
		let content_after_undo = fs::read_to_string(temp_file.path()).await.unwrap();
		let expected_original = "original line 1\noriginal line 2\noriginal line 3\n";
		assert_eq!(
			content_after_undo, expected_original,
			"Content should be restored to original after undo"
		);

		// Verify undo result message
		let content = undo_result.result["content"].as_array().unwrap()[0]["text"]
			.as_str()
			.unwrap();
		assert!(
			content.contains("Successfully undid the last edit"),
			"Should contain undo confirmation message, got: {}",
			content
		);
	}

	// ===== NEGATIVE LINE INDEXING TESTS =====

	#[tokio::test]
	async fn test_text_editor_view_negative_indexing() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let file_path = temp_dir.path().join("test.txt");

		// Create a file with 5 lines
		fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n")
			.await
			.unwrap();

		// Test -1 (last line)
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"lines": [-1, -1]
			}),
		};

		let result = execute_view(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));
		let content = result.result["content"][0]["text"].as_str().unwrap();
		assert!(
			content.contains("5: line 5"),
			"Should show last line: {}",
			content
		);

		// Test -2 (second-to-last line)
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"lines": [-2, -2]
			}),
		};

		let result = execute_view(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));
		let content = result.result["content"][0]["text"].as_str().unwrap();
		assert!(
			content.contains("4: line 4"),
			"Should show second-to-last line: {}",
			content
		);

		// Test range with negative indices
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"lines": [-3, -1]
			}),
		};

		let result = execute_view(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));
		let content = result.result["content"][0]["text"].as_str().unwrap();
		assert!(
			content.contains("3: line 3"),
			"Should show line 3: {}",
			content
		);
		assert!(
			content.contains("4: line 4"),
			"Should show line 4: {}",
			content
		);
		assert!(
			content.contains("5: line 5"),
			"Should show line 5: {}",
			content
		);

		// Test mixed positive and negative indices
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"lines": [2, -2]
			}),
		};

		let result = execute_view(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));
		let content = result.result["content"][0]["text"].as_str().unwrap();
		assert!(
			content.contains("2: line 2"),
			"Should show line 2: {}",
			content
		);
		assert!(
			content.contains("3: line 3"),
			"Should show line 3: {}",
			content
		);
		assert!(
			content.contains("4: line 4"),
			"Should show line 4: {}",
			content
		);
	}

	#[tokio::test]
	async fn test_text_editor_view_negative_indexing_errors() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let file_path = temp_dir.path().join("test.txt");

		// Create a file with 3 lines
		fs::write(&file_path, "line 1\nline 2\nline 3\n")
			.await
			.unwrap();

		// Test negative index beyond file length
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"lines": [-5, -1]
			}),
		};

		let result = execute_view(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(true)));
		let content = result.result["content"][0]["text"].as_str().unwrap();
		assert!(
			content.contains("exceeds file length"),
			"Should show error: {}",
			content
		);
	}

	#[tokio::test]
	async fn test_extract_lines_negative_indexing() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let source_path = temp_dir.path().join("source.txt");
		let target_path = temp_dir.path().join("target.txt");

		// Create source file with 5 lines
		fs::write(&source_path, "line 1\nline 2\nline 3\nline 4\nline 5\n")
			.await
			.unwrap();
		fs::write(&target_path, "").await.unwrap();

		// Test extracting last line with -1
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [-1, -1],
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));

		let target_content = fs::read_to_string(&target_path).await.unwrap();
		assert_eq!(target_content.trim(), "line 5", "Should extract last line");

		// Test extracting last 2 lines
		fs::write(&target_path, "").await.unwrap(); // Clear target
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [-2, -1],
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));

		let target_content = fs::read_to_string(&target_path).await.unwrap();
		assert_eq!(
			target_content.trim(),
			"line 4\nline 5",
			"Should extract last 2 lines"
		);

		// Test mixed positive and negative indices
		fs::write(&target_path, "").await.unwrap(); // Clear target
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [2, -2],
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));

		let target_content = fs::read_to_string(&target_path).await.unwrap();
		assert_eq!(
			target_content.trim(),
			"line 2\nline 3\nline 4",
			"Should extract lines 2-4"
		);
	}

	#[tokio::test]
	async fn test_extract_lines_negative_indexing_errors() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let source_path = temp_dir.path().join("source.txt");
		let target_path = temp_dir.path().join("target.txt");

		// Create source file with 3 lines
		fs::write(&source_path, "line 1\nline 2\nline 3\n")
			.await
			.unwrap();
		fs::write(&target_path, "").await.unwrap();

		// Test negative index beyond file length
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "extract_lines".to_string(),
			parameters: json!({
				"from_path": source_path.to_string_lossy(),
				"from_range": [-5, -1],
				"append_path": target_path.to_string_lossy(),
				"append_line": -1
			}),
		};

		let result = execute_extract_lines(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(true)));
		let content = result.result["content"][0]["text"].as_str().unwrap();
		assert!(
			content.contains("exceeds file length"),
			"Should show error: {}",
			content
		);
	}

	#[tokio::test]
	async fn test_batch_edit_negative_indexing() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let file_path = temp_dir.path().join("test.txt");

		// Create file with 5 lines
		fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n")
			.await
			.unwrap();

		// Test replacing last line with negative index
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"operations": [
					{
						"operation": "replace",
						"line_range": [-1, -1],
						"content": "LAST LINE REPLACED"
					}
				]
			}),
		};

		let result = execute_batch_edit(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));

		let content = fs::read_to_string(&file_path).await.unwrap();
		assert!(
			content.contains("LAST LINE REPLACED"),
			"Should replace last line: {}",
			content
		);
		assert!(
			!content.contains("line 5"),
			"Should not contain original last line"
		);

		// Test replacing last 2 lines with negative range
		fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n")
			.await
			.unwrap();
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"operations": [
					{
						"operation": "replace",
						"line_range": [-2, -1],
						"content": "REPLACED LINES 4-5"
					}
				]
			}),
		};

		let result = execute_batch_edit(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));

		let content = fs::read_to_string(&file_path).await.unwrap();
		assert!(
			content.contains("REPLACED LINES 4-5"),
			"Should replace last 2 lines: {}",
			content
		);
		assert!(
			!content.contains("line 4"),
			"Should not contain original line 4"
		);
		assert!(
			!content.contains("line 5"),
			"Should not contain original line 5"
		);

		// Test insert after second-to-last line
		fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n")
			.await
			.unwrap();

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"operations": [
					{
						"operation": "insert",
						"line_range": -2,
						"content": "INSERTED AFTER LINE 4"
					}
				]
			}),
		};

		let result = execute_batch_edit(&call).await.unwrap();

		// Check if operation succeeded
		if let Some(content_array) = result.result["content"].as_array() {
			if let Some(first_content) = content_array.first() {
				if let Some(text) = first_content["text"].as_str() {
					if text.contains("error")
						|| text.contains("Error")
						|| text.contains("failed")
						|| text.contains("Failed")
					{
						panic!("Batch edit failed: {}", text);
					}
				}
			}
		}

		let content = fs::read_to_string(&file_path).await.unwrap();
		let lines: Vec<&str> = content.lines().collect();

		assert_eq!(
			lines[4], "INSERTED AFTER LINE 4",
			"Should insert after line 4"
		);
		assert_eq!(lines[5], "line 5", "Line 5 should be moved down");
	}

	#[tokio::test]
	async fn test_batch_edit_negative_indexing_errors() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let file_path = temp_dir.path().join("test.txt");

		// Create file with 3 lines
		fs::write(&file_path, "line 1\nline 2\nline 3\n")
			.await
			.unwrap();

		// Test negative index beyond file length
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "batch_edit".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"operations": [
					{
						"operation": "replace",
						"line_range": [-5, -1],
						"content": "SHOULD FAIL"
					}
				]
			}),
		};

		let result = execute_batch_edit(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(true)));
		let content = result.result["content"][0]["text"].as_str().unwrap();
		assert!(
			content.contains("exceeds file length"),
			"Should show error: {}",
			content
		);
	}

	#[tokio::test]
	async fn test_negative_indexing_edge_cases() {
		let temp_dir = tempfile::TempDir::new().unwrap();
		let file_path = temp_dir.path().join("test.txt");

		// Test with single line file
		fs::write(&file_path, "only line\n").await.unwrap();

		// Test -1 on single line file
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"lines": [-1, -1]
			}),
		};

		let result = execute_view(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));
		let content = result.result["content"][0]["text"].as_str().unwrap();
		assert!(
			content.contains("1: only line"),
			"Should show the only line: {}",
			content
		);

		// Test -2 on single line file (should fail)
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({
				"path": file_path.to_string_lossy(),
				"lines": [-2, -1]
			}),
		};

		let result = execute_view(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(true)));
		let content = result.result["content"][0]["text"].as_str().unwrap();
		assert!(
			content.contains("exceeds file length"),
			"Should show error: {}",
			content
		);
	}

	// ===== VIEW TOOL: DIRECTORY DISPATCH TESTS =====

	#[tokio::test]
	async fn test_view_directory_lists_files() {
		// view with a directory path must list files (not error with "missing directory param")
		let temp_dir = tempfile::TempDir::new().unwrap();
		fs::write(temp_dir.path().join("alpha.rs"), "fn a() {}")
			.await
			.unwrap();
		fs::write(temp_dir.path().join("beta.rs"), "fn b() {}")
			.await
			.unwrap();

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({ "path": temp_dir.path().to_string_lossy() }),
		};

		let result = execute_view(&call).await.unwrap();
		assert!(
			!result
				.result
				.get("isError")
				.and_then(|v| v.as_bool())
				.unwrap_or(false),
			"Should not error: {:?}",
			result.result
		);
		// result must contain file listing output
		let output = result.result["output"].as_str().unwrap_or("");
		assert!(
			output.contains("alpha.rs") || output.contains("beta.rs"),
			"Should list files: {output}"
		);
	}

	#[tokio::test]
	async fn test_view_directory_content_search() {
		// view with path=dir + content= must search file contents, not error
		let temp_dir = tempfile::TempDir::new().unwrap();
		fs::write(temp_dir.path().join("foo.rs"), "fn hello_world() {}")
			.await
			.unwrap();
		fs::write(temp_dir.path().join("bar.rs"), "fn unrelated() {}")
			.await
			.unwrap();

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({
				"path": temp_dir.path().to_string_lossy(),
				"content": "hello_world"
			}),
		};

		let result = execute_view(&call).await.unwrap();
		assert!(
			!result
				.result
				.get("isError")
				.and_then(|v| v.as_bool())
				.unwrap_or(false),
			"Should not error: {:?}",
			result.result
		);
		let output = result.result["output"].as_str().unwrap_or("");
		assert!(
			output.contains("hello_world"),
			"Should find match: {output}"
		);
	}

	#[tokio::test]
	async fn test_view_directory_pattern_filter() {
		// view with path=dir + pattern= must filter by filename glob
		let temp_dir = tempfile::TempDir::new().unwrap();
		fs::write(temp_dir.path().join("main.rs"), "fn main() {}")
			.await
			.unwrap();
		fs::write(temp_dir.path().join("config.toml"), "[package]")
			.await
			.unwrap();

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({
				"path": temp_dir.path().to_string_lossy(),
				"pattern": "*.toml"
			}),
		};

		let result = execute_view(&call).await.unwrap();
		assert!(
			!result
				.result
				.get("isError")
				.and_then(|v| v.as_bool())
				.unwrap_or(false),
			"Should not error: {:?}",
			result.result
		);
		let files = result.result["files"].as_array().unwrap();
		assert_eq!(files.len(), 1, "Should find exactly one .toml file");
		assert!(files[0].as_str().unwrap().contains("config.toml"));
	}

	#[tokio::test]
	async fn test_view_file_path_reads_content() {
		// view with a file path must return file content, not try directory listing
		let temp_dir = tempfile::TempDir::new().unwrap();
		let file_path = temp_dir.path().join("hello.txt");
		fs::write(&file_path, "line one\nline two\n").await.unwrap();

		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({ "path": file_path.to_string_lossy() }),
		};

		let result = execute_view(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(false)));
		let content = result.result["content"][0]["text"].as_str().unwrap();
		assert!(
			content.contains("1: line one"),
			"Should show line 1: {content}"
		);
		assert!(
			content.contains("2: line two"),
			"Should show line 2: {content}"
		);
	}

	#[tokio::test]
	async fn test_view_missing_path_errors() {
		// view with no path and no paths must return a clear error, not panic
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "view".to_string(),
			parameters: json!({}),
		};

		let result = execute_view(&call).await.unwrap();
		assert_eq!(result.result.get("isError"), Some(&json!(true)));
		let msg = result.result["content"][0]["text"].as_str().unwrap();
		assert!(msg.contains("path"), "Error should mention 'path': {msg}");
	}

	#[tokio::test]
	async fn test_batch_edit_four_operations_original_line_numbers() {
		// Test the CRITICAL SCENARIO: 4 batch operations using ORIGINAL line numbers
		// This test verifies that line shifts from earlier operations don't affect later ones
		let temp_file = create_test_file(
			"line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10\n",
		)
		.await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "replace",
				"line_range": [2, 2],
				"content": "REPLACED LINE 2"
			},
			{
				"operation": "insert",
				"line_range": 4,
				"content": "INSERTED AFTER ORIGINAL LINE 4"
			},
			{
				"operation": "replace",
				"line_range": [6, 7],
				"content": "REPLACED ORIGINAL LINES 6-7"
			},
			{
				"operation": "insert",
				"line_range": 9,
				"content": "INSERTED AFTER ORIGINAL LINE 9"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Check success
		assert!(
			result.result.get("error").is_none(),
			"Operation should succeed: {:?}",
			result.result
		);

		// Verify file content - ALL operations should use ORIGINAL line positions
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();

		// Expected result if operations are applied to ORIGINAL line numbers:
		// - Replace line 2 with "REPLACED LINE 2"
		// - Insert after line 4 (original): "INSERTED AFTER ORIGINAL LINE 4"
		// - Replace lines 6-7 (original) with "REPLACED ORIGINAL LINES 6-7"
		// - Insert after line 9 (original): "INSERTED AFTER ORIGINAL LINE 9"
		let expected = "line 1\nREPLACED LINE 2\nline 3\nline 4\nINSERTED AFTER ORIGINAL LINE 4\nline 5\nREPLACED ORIGINAL LINES 6-7\nline 8\nline 9\nINSERTED AFTER ORIGINAL LINE 9\nline 10\n";

		assert_eq!(
			actual, expected,
			"Content should reflect all 4 operations using original line numbers.\nActual:\n{}\nExpected:\n{}",
			actual, expected
		);
	}

	#[tokio::test]
	async fn test_batch_edit_overlapping_operations_should_fail() {
		// CRITICAL TEST: Overlapping operations should be detected and rejected
		// This prevents undefined behavior when operations affect the same lines
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\nline 5\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		// These operations overlap: both affect line 3
		let operations = json!([
			{
				"operation": "replace",
				"line_range": [1, 3], // affects lines 1, 2, 3
				"content": "REPLACED 1-3"
			},
			{
				"operation": "replace",
				"line_range": [3, 5], // affects lines 3, 4, 5 - OVERLAPS with line 3!
				"content": "REPLACED 3-5"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Should fail due to conflict
		assert!(
			result.result.get("content").is_some(),
			"Should have error content due to overlapping operations"
		);
		let content = result.result["content"].as_array().unwrap()[0]["text"]
			.as_str()
			.unwrap();
		assert!(
			content.contains("Conflicting operations"),
			"Should detect conflict: {}",
			content
		);
	}

	#[tokio::test]
	async fn test_batch_edit_insert_and_replace_same_line_should_fail() {
		// CRITICAL TEST: Insert after line N and replace line N should conflict
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\nline 5\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "insert",
				"line_range": 2, // insert after line 2
				"content": "INSERTED AFTER 2"
			},
			{
				"operation": "replace",
				"line_range": [2, 2], // replace line 2 - CONFLICTS!
				"content": "REPLACED 2"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Should fail due to conflict
		assert!(
			result.result.get("content").is_some(),
			"Should have error content due to conflicting operations"
		);
		let content = result.result["content"].as_array().unwrap()[0]["text"]
			.as_str()
			.unwrap();
		assert!(
			content.contains("Conflicting operations"),
			"Should detect conflict: {}",
			content
		);
	}

	#[tokio::test]
	async fn test_batch_edit_expansion_operations_atomic() {
		// CRITICAL TEST: Operations that expand content (1 line -> 4 lines) should work atomically
		// This tests the scenario you mentioned: replace 1 line with 4 lines
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\nline 5\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "replace",
				"line_range": [1, 1], // replace line 1 with 4 lines
				"content": "NEW LINE 1A\nNEW LINE 1B\nNEW LINE 1C\nNEW LINE 1D"
			},
			{
				"operation": "replace",
				"line_range": [5, 5], // replace line 5 with 3 lines
				"content": "NEW LINE 5A\nNEW LINE 5B\nNEW LINE 5C"
			},
			{
				"operation": "insert",
				"line_range": 3, // insert after original line 3
				"content": "INSERTED AFTER ORIGINAL 3"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Check success
		assert!(
			result.result.get("error").is_none(),
			"Operation should succeed: {:?}",
			result.result
		);

		// Verify content - all operations should use ORIGINAL line positions
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();

		// Expected: operations applied to original positions regardless of expansion
		let expected = "NEW LINE 1A\nNEW LINE 1B\nNEW LINE 1C\nNEW LINE 1D\nline 2\nline 3\nINSERTED AFTER ORIGINAL 3\nline 4\nNEW LINE 5A\nNEW LINE 5B\nNEW LINE 5C\n";

		assert_eq!(
			actual, expected,
			"Content should reflect atomic operations on original positions.\nActual:\n{}\nExpected:\n{}",
			actual, expected
		);
	}

	#[tokio::test]
	async fn test_batch_edit_complex_mixed_operations() {
		// COMPREHENSIVE TEST: Mix of inserts, single replacements, and multi-line replacements
		let temp_file = create_test_file("A\nB\nC\nD\nE\nF\nG\nH\nI\nJ\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "insert",
				"line_range": 1, // insert after line 1 (A)
				"content": "AFTER_A"
			},
			{
				"operation": "replace",
				"line_range": [2, 4], // replace B,C,D with single line
				"content": "BCD_REPLACED"
			},
			{
				"operation": "insert",
				"line_range": 6, // insert after F
				"content": "AFTER_F"
			},
			{
				"operation": "replace",
				"line_range": [8, 8], // replace H
				"content": "H1\nH2\nH3" // expand to 3 lines
			},
			{
				"operation": "insert",
				"line_range": 10, // insert after J (last line)
				"content": "FOOTER"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Check success
		assert!(
			result.result.get("error").is_none(),
			"Operation should succeed: {:?}",
			result.result
		);

		// Verify content
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();

		// Expected result based on original line positions:
		// - A (line 1 unchanged)
		// - Insert after line 1: AFTER_A
		// - Replace lines 2-4 (B,C,D): BCD_REPLACED
		// - E (line 5 unchanged)
		// - F (line 6 unchanged)
		// - Insert after line 6: AFTER_F
		// - G (line 7 unchanged)
		// - Replace line 8 (H): H1\nH2\nH3
		// - I (line 9 unchanged)
		// - J (line 10 unchanged)
		// - Insert after line 10: FOOTER
		let expected = "A\nAFTER_A\nBCD_REPLACED\nE\nF\nAFTER_F\nG\nH1\nH2\nH3\nI\nJ\nFOOTER\n";

		assert_eq!(
			actual, expected,
			"Complex mixed operations should work atomically.\nActual:\n{}\nExpected:\n{}",
			actual, expected
		);
	}

	#[tokio::test]
	async fn test_batch_edit_edge_case_adjacent_operations() {
		// EDGE CASE: Operations on adjacent lines (should NOT conflict)
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\nline 5\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "replace",
				"line_range": [1, 1], // replace line 1
				"content": "REPLACED 1"
			},
			{
				"operation": "replace",
				"line_range": [2, 2], // replace line 2 (adjacent, should be OK)
				"content": "REPLACED 2"
			},
			{
				"operation": "insert",
				"line_range": 3, // insert after line 3
				"content": "AFTER 3"
			},
			{
				"operation": "replace",
				"line_range": [4, 4], // replace line 4 (should be OK)
				"content": "REPLACED 4"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Should succeed - no conflicts
		assert!(
			result.result.get("error").is_none(),
			"Adjacent operations should not conflict: {:?}",
			result.result
		);

		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		let expected = "REPLACED 1\nREPLACED 2\nline 3\nAFTER 3\nREPLACED 4\nline 5\n";

		assert_eq!(
			actual, expected,
			"Adjacent operations should work correctly.\nActual:\n{}\nExpected:\n{}",
			actual, expected
		);
	}

	#[tokio::test]
	async fn test_batch_edit_your_exact_scenario_should_fail() {
		// YOUR EXACT SCENARIO: replace line 1 with 4 lines AND replace line 3 with 4 lines
		// This should FAIL because both operations affect overlapping content
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\nline 5\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "replace",
				"line_range": [1, 1], // replace line 1 with 4 lines
				"content": "NEW1A\nNEW1B\nNEW1C\nNEW1D"
			},
			{
				"operation": "replace",
				"line_range": [3, 3], // replace line 3 with 4 lines - this is OK, no overlap
				"content": "NEW3A\nNEW3B\nNEW3C\nNEW3D"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// This should SUCCEED because line 1 and line 3 don't overlap
		assert!(
			result.result.get("error").is_none(),
			"Non-overlapping operations should succeed: {:?}",
			result.result
		);

		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		// Expected: line 1 -> 4 lines, line 2 unchanged, line 3 -> 4 lines, lines 4-5 unchanged
		let expected =
			"NEW1A\nNEW1B\nNEW1C\nNEW1D\nline 2\nNEW3A\nNEW3B\nNEW3C\nNEW3D\nline 4\nline 5\n";

		assert_eq!(
			actual, expected,
			"Your scenario should work when lines don't overlap.\nActual:\n{}\nExpected:\n{}",
			actual, expected
		);
	}

	#[tokio::test]
	async fn test_batch_edit_overlapping_ranges_should_fail() {
		// CRITICAL: Overlapping ranges should be detected and rejected
		let temp_file = create_test_file("line 1\nline 2\nline 3\nline 4\nline 5\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "replace",
				"line_range": [1, 3], // replace lines 1-3
				"content": "REPLACED_1_TO_3"
			},
			{
				"operation": "replace",
				"line_range": [3, 5], // replace lines 3-5 - OVERLAPS at line 3!
				"content": "REPLACED_3_TO_5"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Should fail due to overlap at line 3
		assert!(
			result.result.get("content").is_some(),
			"Should have error content due to overlapping ranges"
		);
		let content = result.result["content"].as_array().unwrap()[0]["text"]
			.as_str()
			.unwrap();
		assert!(
			content.contains("Conflicting operations"),
			"Should detect overlap at line 3: {}",
			content
		);
	}

	#[tokio::test]
	async fn test_batch_edit_ultimate_stress_test() {
		// ULTIMATE STRESS TEST: Multiple expansion operations with no conflicts
		// This verifies the algorithm is truly atomic and handles original line positions correctly
		let temp_file = create_test_file("A\nB\nC\nD\nE\nF\nG\nH\nI\nJ\nK\nL\nM\nN\nO\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "replace",
				"line_range": [1, 1], // A -> 3 lines
				"content": "A1\nA2\nA3"
			},
			{
				"operation": "replace",
				"line_range": [3, 3], // C -> 5 lines
				"content": "C1\nC2\nC3\nC4\nC5"
			},
			{
				"operation": "insert",
				"line_range": 5, // insert after E
				"content": "AFTER_E1\nAFTER_E2"
			},
			{
				"operation": "replace",
				"line_range": [7, 9], // G,H,I -> 2 lines
				"content": "GHI_1\nGHI_2"
			},
			{
				"operation": "insert",
				"line_range": 12, // insert after L
				"content": "AFTER_L"
			},
			{
				"operation": "replace",
				"line_range": [15, 15], // O -> 4 lines
				"content": "O1\nO2\nO3\nO4"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Should succeed - no conflicts
		assert!(
			result.result.get("error").is_none(),
			"Ultimate stress test should succeed: {:?}",
			result.result
		);

		let actual = fs::read_to_string(temp_file.path()).await.unwrap();

		// Expected result using ORIGINAL line positions:
		// Line 1 (A) -> A1,A2,A3
		// Line 2 (B) -> B
		// Line 3 (C) -> C1,C2,C3,C4,C5
		// Line 4 (D) -> D
		// Line 5 (E) -> E + AFTER_E1,AFTER_E2
		// Line 6 (F) -> F
		// Lines 7-9 (G,H,I) -> GHI_1,GHI_2
		// Line 10 (J) -> J
		// Line 11 (K) -> K
		// Line 12 (L) -> L + AFTER_L
		// Line 13 (M) -> M
		// Line 14 (N) -> N
		// Line 15 (O) -> O1,O2,O3,O4
		let expected = "A1\nA2\nA3\nB\nC1\nC2\nC3\nC4\nC5\nD\nE\nAFTER_E1\nAFTER_E2\nF\nGHI_1\nGHI_2\nJ\nK\nL\nAFTER_L\nM\nN\nO1\nO2\nO3\nO4\n";

		assert_eq!(
			actual, expected,
			"Ultimate stress test with expansions should work atomically.\nActual:\n{}\nExpected:\n{}",
			actual, expected
		);
	}

	#[tokio::test]
	async fn test_batch_edit_extreme_expansions_and_contractions() {
		// EXTREME TEST: Mix massive expansions (1->10 lines) and contractions (5->1 line)
		// This is the most aggressive test of original line indexing
		let temp_file = create_test_file("L1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\nL9\nL10\nL11\nL12\nL13\nL14\nL15\nL16\nL17\nL18\nL19\nL20\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "replace",
				"line_range": [1, 1], // L1 -> 10 LINES (massive expansion)
				"content": "EXP1_1\nEXP1_2\nEXP1_3\nEXP1_4\nEXP1_5\nEXP1_6\nEXP1_7\nEXP1_8\nEXP1_9\nEXP1_10"
			},
			{
				"operation": "replace",
				"line_range": [3, 7], // L3,L4,L5,L6,L7 -> 1 LINE (massive contraction)
				"content": "CONTRACTED_3_TO_7"
			},
			{
				"operation": "replace",
				"line_range": [9, 9], // L9 -> 8 LINES (big expansion)
				"content": "EXP9_1\nEXP9_2\nEXP9_3\nEXP9_4\nEXP9_5\nEXP9_6\nEXP9_7\nEXP9_8"
			},
			{
				"operation": "replace",
				"line_range": [12, 16], // L12,L13,L14,L15,L16 -> 2 LINES (contraction)
				"content": "CONTRACT_12_16_A\nCONTRACT_12_16_B"
			},
			{
				"operation": "insert",
				"line_range": 18, // insert after L18 -> 6 LINES
				"content": "INS18_1\nINS18_2\nINS18_3\nINS18_4\nINS18_5\nINS18_6"
			},
			{
				"operation": "replace",
				"line_range": [20, 20], // L20 -> 12 LINES (extreme expansion)
				"content": "EXP20_1\nEXP20_2\nEXP20_3\nEXP20_4\nEXP20_5\nEXP20_6\nEXP20_7\nEXP20_8\nEXP20_9\nEXP20_10\nEXP20_11\nEXP20_12"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Should succeed - no conflicts despite extreme size changes
		assert!(
			result.result.get("error").is_none(),
			"Extreme expansions/contractions should succeed: {:?}",
			result.result
		);

		let actual = fs::read_to_string(temp_file.path()).await.unwrap();

		// Expected result using ORIGINAL line positions (critical test):
		// Line 1 (L1) -> 10 lines
		// Line 2 (L2) -> unchanged
		// Lines 3-7 (L3,L4,L5,L6,L7) -> 1 line
		// Line 8 (L8) -> unchanged
		// Line 9 (L9) -> 8 lines
		// Line 10 (L10) -> unchanged
		// Line 11 (L11) -> unchanged
		// Lines 12-16 (L12,L13,L14,L15,L16) -> 2 lines
		// Line 17 (L17) -> unchanged
		// Line 18 (L18) -> unchanged + 6 inserted lines
		// Line 19 (L19) -> unchanged
		// Line 20 (L20) -> 12 lines
		let expected = "EXP1_1\nEXP1_2\nEXP1_3\nEXP1_4\nEXP1_5\nEXP1_6\nEXP1_7\nEXP1_8\nEXP1_9\nEXP1_10\nL2\nCONTRACTED_3_TO_7\nL8\nEXP9_1\nEXP9_2\nEXP9_3\nEXP9_4\nEXP9_5\nEXP9_6\nEXP9_7\nEXP9_8\nL10\nL11\nCONTRACT_12_16_A\nCONTRACT_12_16_B\nL17\nL18\nINS18_1\nINS18_2\nINS18_3\nINS18_4\nINS18_5\nINS18_6\nL19\nEXP20_1\nEXP20_2\nEXP20_3\nEXP20_4\nEXP20_5\nEXP20_6\nEXP20_7\nEXP20_8\nEXP20_9\nEXP20_10\nEXP20_11\nEXP20_12\n";

		assert_eq!(
			actual, expected,
			"CRITICAL: Extreme expansions/contractions must use original line positions!\nActual:\n{}\nExpected:\n{}",
			actual, expected
		);
	}

	#[tokio::test]
	async fn test_batch_edit_massive_file_with_extreme_operations() {
		// MASSIVE FILE TEST: 50 lines with extreme operations throughout
		let mut content = String::new();
		for i in 1..=50 {
			content.push_str(&format!("LINE_{:02}\n", i));
		}
		let temp_file = create_test_file(&content).await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "replace",
				"line_range": [5, 5], // 1 line -> 15 lines (extreme expansion)
				"content": "E5_01\nE5_02\nE5_03\nE5_04\nE5_05\nE5_06\nE5_07\nE5_08\nE5_09\nE5_10\nE5_11\nE5_12\nE5_13\nE5_14\nE5_15"
			},
			{
				"operation": "replace",
				"line_range": [10, 20], // 11 lines -> 1 line (extreme contraction)
				"content": "MEGA_CONTRACTION_10_TO_20"
			},
			{
				"operation": "insert",
				"line_range": 25, // insert 8 lines after line 25
				"content": "I25_1\nI25_2\nI25_3\nI25_4\nI25_5\nI25_6\nI25_7\nI25_8"
			},
			{
				"operation": "replace",
				"line_range": [30, 35], // 6 lines -> 20 lines (massive expansion)
				"content": "M30_01\nM30_02\nM30_03\nM30_04\nM30_05\nM30_06\nM30_07\nM30_08\nM30_09\nM30_10\nM30_11\nM30_12\nM30_13\nM30_14\nM30_15\nM30_16\nM30_17\nM30_18\nM30_19\nM30_20"
			},
			{
				"operation": "replace",
				"line_range": [40, 49], // 10 lines -> 2 lines (big contraction)
				"content": "BIG_CONTRACT_A\nBIG_CONTRACT_B"
			},
			{
				"operation": "insert",
				"line_range": 50, // insert 5 lines after last line
				"content": "FINAL_1\nFINAL_2\nFINAL_3\nFINAL_4\nFINAL_5"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Should succeed
		assert!(
			result.result.get("error").is_none(),
			"Massive file operations should succeed: {:?}",
			result.result
		);

		let actual = fs::read_to_string(temp_file.path()).await.unwrap();

		// Build expected result step by step using ORIGINAL line positions
		let mut expected_lines = Vec::new();

		// Lines 1-4: unchanged
		for i in 1..=4 {
			expected_lines.push(format!("LINE_{:02}", i));
		}

		// Line 5: 1->15 expansion
		for i in 1..=15 {
			expected_lines.push(format!("E5_{:02}", i));
		}

		// Lines 6-9: unchanged
		for i in 6..=9 {
			expected_lines.push(format!("LINE_{:02}", i));
		}

		// Lines 10-20: 11->1 contraction
		expected_lines.push("MEGA_CONTRACTION_10_TO_20".to_string());

		// Lines 21-24: unchanged
		for i in 21..=24 {
			expected_lines.push(format!("LINE_{:02}", i));
		}

		// Line 25: unchanged + 8 insertions
		expected_lines.push("LINE_25".to_string());
		for i in 1..=8 {
			expected_lines.push(format!("I25_{}", i));
		}

		// Lines 26-29: unchanged
		for i in 26..=29 {
			expected_lines.push(format!("LINE_{:02}", i));
		}

		// Lines 30-35: 6->20 expansion
		for i in 1..=20 {
			expected_lines.push(format!("M30_{:02}", i));
		}

		// Lines 36-39: unchanged
		for i in 36..=39 {
			expected_lines.push(format!("LINE_{:02}", i));
		}

		// Lines 40-49: 10->2 contraction
		expected_lines.push("BIG_CONTRACT_A".to_string());
		expected_lines.push("BIG_CONTRACT_B".to_string());

		// Line 50: unchanged + 5 insertions
		expected_lines.push("LINE_50".to_string());
		for i in 1..=5 {
			expected_lines.push(format!("FINAL_{}", i));
		}

		let expected = expected_lines.join("\n") + "\n";

		assert_eq!(
			actual, expected,
			"MASSIVE FILE: All operations must use original line positions!\nActual length: {}, Expected length: {}",
			actual.lines().count(), expected.lines().count()
		);
	}

	#[tokio::test]
	async fn test_batch_edit_pathological_case_all_expansions() {
		// PATHOLOGICAL CASE: Every single operation is a massive expansion
		// This is the ultimate test of original line preservation
		let temp_file = create_test_file("A\nB\nC\nD\nE\nF\nG\nH\nI\nJ\n").await;
		let path = temp_file.path().to_string_lossy().to_string();

		let operations = json!([
			{
				"operation": "replace",
				"line_range": [1, 1], // A -> 7 lines
				"content": "A1\nA2\nA3\nA4\nA5\nA6\nA7"
			},
			{
				"operation": "replace",
				"line_range": [3, 3], // C -> 5 lines
				"content": "C1\nC2\nC3\nC4\nC5"
			},
			{
				"operation": "replace",
				"line_range": [5, 5], // E -> 9 lines
				"content": "E1\nE2\nE3\nE4\nE5\nE6\nE7\nE8\nE9"
			},
			{
				"operation": "replace",
				"line_range": [7, 7], // G -> 12 lines
				"content": "G01\nG02\nG03\nG04\nG05\nG06\nG07\nG08\nG09\nG10\nG11\nG12"
			},
			{
				"operation": "replace",
				"line_range": [9, 9], // I -> 6 lines
				"content": "I1\nI2\nI3\nI4\nI5\nI6"
			}
		]);

		let call = create_batch_edit_call(&path, operations).await;
		let result = crate::mcp::fs::core::execute_batch_edit(&call)
			.await
			.unwrap();

		// Should succeed
		assert!(
			result.result.get("error").is_none(),
			"All expansions should succeed: {:?}",
			result.result
		);

		let actual = fs::read_to_string(temp_file.path()).await.unwrap();

		// Expected: every expansion uses ORIGINAL line position
		let expected = "A1\nA2\nA3\nA4\nA5\nA6\nA7\nB\nC1\nC2\nC3\nC4\nC5\nD\nE1\nE2\nE3\nE4\nE5\nE6\nE7\nE8\nE9\nF\nG01\nG02\nG03\nG04\nG05\nG06\nG07\nG08\nG09\nG10\nG11\nG12\nH\nI1\nI2\nI3\nI4\nI5\nI6\nJ\n";

		assert_eq!(
			actual, expected,
			"PATHOLOGICAL: All expansions must preserve original positions!\nActual:\n{}\nExpected:\n{}",
			actual, expected
		);
	}
}
