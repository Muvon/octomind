#[cfg(test)]
mod tests {
	use crate::mcp::fs::core::execute_extract_lines;
	use crate::mcp::fs::text_editing::line_replace_spec;
	use crate::mcp::McpToolCall;
	use serde_json::json;
	use tempfile::NamedTempFile;
	use tokio::fs;

	async fn create_test_file(content: &str) -> NamedTempFile {
		let temp_file = NamedTempFile::new().unwrap();
		fs::write(temp_file.path(), content).await.unwrap();
		temp_file
	}

	async fn test_line_replace(
		content: &str,
		start_line: usize,
		end_line: usize,
		new_str: &str,
		expected: &str,
	) {
		let temp_file = create_test_file(content).await;
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "text_editor".to_string(),
			parameters: json!({}),
		};

		let result = line_replace_spec(&call, temp_file.path(), (start_line, end_line), new_str)
			.await
			.unwrap();

		// Check that operation succeeded
		assert!(result.result.get("error").is_none());

		// Check file content
		let actual = fs::read_to_string(temp_file.path()).await.unwrap();
		assert_eq!(actual, expected, "Content mismatch");
	}

	#[tokio::test]
	async fn test_single_line_replace() {
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"REPLACED",
			"line 1\nREPLACED\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_multiple_lines_replace() {
		test_line_replace(
			"line 1\nline 2\nline 3\nline 4\n",
			2,
			3,
			"SINGLE REPLACEMENT",
			"line 1\nSINGLE REPLACEMENT\nline 4\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_replace_with_multiline() {
		test_line_replace(
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
		test_line_replace(
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
		test_line_replace(
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
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			1,
			3,
			"EVERYTHING REPLACED",
			"EVERYTHING REPLACED\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_no_final_newline() {
		test_line_replace(
			"line 1\nline 2\nline 3",
			2,
			2,
			"REPLACED",
			"line 1\nREPLACED\nline 3",
		)
		.await;
	}

	#[tokio::test]
	async fn test_windows_line_endings() {
		test_line_replace(
			"line 1\r\nline 2\r\nline 3\r\n",
			2,
			2,
			"REPLACED",
			"line 1\r\nREPLACED\r\nline 3\r\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_empty_replacement() {
		test_line_replace("line 1\nline 2\nline 3\n", 2, 2, "", "line 1\n\nline 3\n").await;
	}

	#[tokio::test]
	async fn test_single_line_file() {
		test_line_replace("only line", 1, 1, "REPLACED", "REPLACED").await;
	}

	#[tokio::test]
	async fn test_tricky_characters_quotes() {
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"\"123\"",
			"line 1\n\"123\"\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_tricky_characters_actual_newlines() {
		// Test with actual newline characters in the replacement string
		let replacement_with_newlines = "hello\nworld\ntest";
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			replacement_with_newlines,
			"line 1\nhello\nworld\ntest\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_tricky_characters_actual_tabs() {
		// Test with actual tab characters in the replacement string
		let replacement_with_tabs = "\thello\tworld";
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			replacement_with_tabs,
			"line 1\n\thello\tworld\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_tricky_characters_literal_backslash_n() {
		// Test with literal \n characters (not actual newlines)
		let replacement_with_literal = "hello\\nworld";
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			replacement_with_literal,
			"line 1\nhello\\nworld\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_tricky_characters_mixed_actual_and_literal() {
		// Test mixing actual newlines and literal \n
		let replacement_mixed = "actual\nnewline and literal\\nbackslash";
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			replacement_mixed,
			"line 1\nactual\nnewline and literal\\nbackslash\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_tricky_characters_backslashes() {
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"path\\to\\file",
			"line 1\npath\\to\\file\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_tricky_characters_special_symbols() {
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"!@#$%^&*()[]{}|;':\",./<>?",
			"line 1\n!@#$%^&*()[]{}|;':\",./<>?\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_tricky_characters_unicode() {
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"🚀 Hello 世界 🎉",
			"line 1\n🚀 Hello 世界 🎉\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_tricky_characters_carriage_return() {
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"hello\rworld",
			"line 1\nhello\rworld\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_tricky_characters_null_and_control() {
		test_line_replace(
			"line 1\nline 2\nline 3\n",
			2,
			2,
			"test\x00null\x01control",
			"line 1\ntest\x00null\x01control\nline 3\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_tricky_characters_mixed_complex() {
		test_line_replace(
            "line 1\nline 2\nline 3\n",
            2,
            2,
            "fn test() {\n    println!(\"Hello\\tworld\\n\");\n    let x = \"\\\"quoted\\\"\";\n}",
            "line 1\nfn test() {\n    println!(\"Hello\\tworld\\n\");\n    let x = \"\\\"quoted\\\"\";\n}\nline 3\n",
        )
        .await;
	}

	#[tokio::test]
	async fn test_byte_level_verification() {
		// Create test with actual newlines and verify byte-by-byte
		let temp_file = create_test_file("line 1\nline 2\nline 3\n").await;
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "text_editor".to_string(),
			parameters: json!({}),
		};

		// Replace with content containing actual newline character
		let replacement = "hello\nworld"; // This contains an actual newline, not \\n
		let result = line_replace_spec(&call, temp_file.path(), (2, 2), replacement)
			.await
			.unwrap();

		// Check that operation succeeded
		assert!(result.result.get("error").is_none());

		// Read and verify byte content
		let actual_bytes = fs::read(temp_file.path()).await.unwrap();
		let expected_bytes = b"line 1\nhello\nworld\nline 3\n";

		assert_eq!(actual_bytes, expected_bytes, "Byte-level content mismatch");

		// Also verify as string
		let actual_string = String::from_utf8(actual_bytes.clone()).unwrap();
		let expected_string = "line 1\nhello\nworld\nline 3\n";
		assert_eq!(actual_string, expected_string, "String content mismatch");

		// Verify the newline characters are actual newlines (byte value 10)
		assert_eq!(actual_bytes[6], 10u8, "First newline should be byte 10");
		assert_eq!(actual_bytes[12], 10u8, "Second newline should be byte 10");
		assert_eq!(actual_bytes[18], 10u8, "Third newline should be byte 10");
		assert_eq!(actual_bytes[25], 10u8, "Fourth newline should be byte 10");
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
		assert!(result.result.get("error").is_some());
		assert!(result.result.get("is_error").unwrap().as_bool().unwrap());
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
		assert!(result.result.get("error").is_some());
		assert!(result.result.get("is_error").unwrap().as_bool().unwrap());
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

	// ========== INSERT_TEXT TESTS ==========

	async fn test_insert_text(content: &str, insert_line: usize, new_str: &str, expected: &str) {
		let temp_file = create_test_file(content).await;
		let call = McpToolCall {
			tool_id: "test".to_string(),
			tool_name: "text_editor".to_string(),
			parameters: json!({}),
		};

		let result = crate::mcp::fs::text_editing::insert_text_spec(
			&call,
			temp_file.path(),
			insert_line,
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
	async fn test_insert_text_beginning() {
		test_insert_text(
			"line 1\nline 2\nline 3",
			0,
			"INSERTED",
			"INSERTED\nline 1\nline 2\nline 3",
		)
		.await;
	}

	#[tokio::test]
	async fn test_insert_text_middle() {
		test_insert_text(
			"line 1\nline 2\nline 3",
			1,
			"INSERTED",
			"line 1\nINSERTED\nline 2\nline 3",
		)
		.await;
	}

	#[tokio::test]
	async fn test_insert_text_end() {
		test_insert_text(
			"line 1\nline 2\nline 3",
			3,
			"INSERTED",
			"line 1\nline 2\nline 3\nINSERTED",
		)
		.await;
	}

	#[tokio::test]
	async fn test_insert_text_multiline() {
		test_insert_text(
			"line 1\nline 3",
			1,
			"line 2a\nline 2b",
			"line 1\nline 2a\nline 2b\nline 3",
		)
		.await;
	}

	#[tokio::test]
	async fn test_insert_text_with_actual_newlines() {
		let insert_content = "hello\nworld";
		test_insert_text(
			"before\nafter",
			1,
			insert_content,
			"before\nhello\nworld\nafter",
		)
		.await;
	}

	#[tokio::test]
	async fn test_insert_text_with_tabs() {
		test_insert_text(
			"function() {\n}",
			1,
			"\tconsole.log('inserted');",
			"function() {\n\tconsole.log('inserted');\n}",
		)
		.await;
	}

	#[tokio::test]
	async fn test_insert_text_preserve_final_newline() {
		test_insert_text(
			"line 1\nline 2\n",
			1,
			"INSERTED",
			"line 1\nINSERTED\nline 2\n",
		)
		.await;
	}

	#[tokio::test]
	async fn test_insert_text_no_final_newline() {
		test_insert_text("line 1\nline 2", 1, "INSERTED", "line 1\nINSERTED\nline 2").await;
	}

	#[tokio::test]
	async fn test_list_files_truncation() {
		use crate::mcp::fs::directory::execute_list_files;
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

		// Test with default max_lines (20) - should truncate
		let call = McpToolCall {
			tool_name: "list_files".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"pattern": "*.txt"
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result = execute_list_files(&call).await.unwrap();
		let output = result.result.as_object().unwrap();

		// Should have truncation info
		assert!(output.contains_key("truncation_info"));
		assert_eq!(output["count"], 30); // Total count
		assert_eq!(output["displayed_count"], 20); // 19 files + 1 truncation marker (max_lines=20)

		// Test with max_lines = 0 (unlimited) - should not truncate
		let call_unlimited = McpToolCall {
			tool_name: "list_files".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"pattern": "*.txt",
				"max_lines": 0
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result_unlimited = execute_list_files(&call_unlimited).await.unwrap();
		let output_unlimited = result_unlimited.result.as_object().unwrap();

		// Should not have truncation info
		assert!(!output_unlimited.contains_key("truncation_info"));
		assert_eq!(output_unlimited["count"], 30);
		assert_eq!(output_unlimited["displayed_count"], 30);

		// Test with max_lines = 5 - should truncate more aggressively
		let call_small = McpToolCall {
			tool_name: "list_files".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"pattern": "*.txt",
				"max_lines": 5
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result_small = execute_list_files(&call_small).await.unwrap();
		let output_small = result_small.result.as_object().unwrap();

		// Should have truncation info
		assert!(output_small.contains_key("truncation_info"));
		assert_eq!(output_small["count"], 30);
		assert_eq!(output_small["displayed_count"], 5); // 4 files + 1 truncation marker (max_lines=5)

		// Check that truncation marker is present in the files array
		let files = output_small["files"].as_array().unwrap();
		let has_truncation_marker = files
			.iter()
			.any(|f| f.as_str().unwrap_or("").contains("lines truncated"));
		assert!(has_truncation_marker);
	}

	#[tokio::test]
	async fn test_list_files_content_search_preserves_format() {
		use crate::mcp::fs::directory::execute_list_files;
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
			tool_name: "list_files".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"content": "println!",
				"line_numbers": true,
				"max_lines": 0  // unlimited
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result = execute_list_files(&call).await.unwrap();
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
			tool_name: "list_files".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"content": "println!",
				"line_numbers": true,
				"context": 1,
				"max_lines": 0
			}),
			tool_id: "test-call-id".to_string(),
		};

		let result_with_context = execute_list_files(&call_with_context).await.unwrap();
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
		use crate::mcp::fs::directory::execute_list_files;
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
			tool_name: "list_files".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"pattern": "*.rs"
			}),
			tool_id: "test-call-id".to_string(),
		};

		let file_list_result = execute_list_files(&file_list_call).await.unwrap();
		let file_list_output = file_list_result.result.as_object().unwrap();

		// Should be file listing
		assert_eq!(file_list_output["type"], "file listing");
		assert!(file_list_output.contains_key("files"));
		assert!(file_list_output.contains_key("count"));

		// Test 2: Content search (should return formatted matches)
		let content_search_call = McpToolCall {
			tool_name: "list_files".to_string(),
			parameters: json!({
				"directory": temp_path.to_str().unwrap(),
				"content": "println!"
			}),
			tool_id: "test-call-id".to_string(),
		};

		let content_search_result = execute_list_files(&content_search_call).await.unwrap();
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

		let result = execute_extract_lines(&call, None).await.unwrap();

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

		let result = execute_extract_lines(&call, None).await.unwrap();
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

		let result = execute_extract_lines(&call, None).await.unwrap();
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

		let result = execute_extract_lines(&call, None).await.unwrap();
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

		let result = execute_extract_lines(&call, None).await.unwrap();
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

		let result = execute_extract_lines(&call, None).await.unwrap();
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

		let result = execute_extract_lines(&call, None).await.unwrap();
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

		let result = execute_extract_lines(&call, None).await.unwrap();
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

		let result = execute_extract_lines(&call, None).await.unwrap();
		assert!(
			result.result.get("isError") == Some(&json!(true)),
			"Should fail with empty from_path"
		);
		assert!(result.result["content"][0]["text"]
			.as_str()
			.unwrap()
			.contains("cannot be empty"));
	}
}
