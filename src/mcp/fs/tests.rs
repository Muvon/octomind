#[cfg(test)]
mod tests {
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
		assert!(!result.result.get("error").is_some());

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
		assert!(!result.result.get("error").is_some());

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
}
