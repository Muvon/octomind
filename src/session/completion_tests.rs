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

use super::{ensure_structured_output_support, load_structured_output_schema};
use std::io::Write;

#[test]
fn structured_output_supported_for_openai() {
	// OpenAI reports structured-output support for all of its models.
	assert!(ensure_structured_output_support("openai:gpt-4.1").is_ok());
}

#[test]
fn structured_output_unsupported_for_anthropic() {
	let err = ensure_structured_output_support("anthropic:claude-sonnet-4-6")
		.expect_err("anthropic must be rejected")
		.to_string();
	assert!(
		err.contains("does not support structured output"),
		"unexpected error: {err}"
	);
}

#[test]
fn loads_valid_object_schema() {
	let mut f = tempfile::NamedTempFile::new().unwrap();
	f.write_all(br#"{"type":"object","properties":{"x":{"type":"string"}}}"#)
		.unwrap();
	f.flush().unwrap();
	let schema = load_structured_output_schema(f.path().to_str().unwrap()).unwrap();
	assert_eq!(schema["type"].as_str(), Some("object"));
}

#[test]
fn rejects_non_object_schema() {
	let mut f = tempfile::NamedTempFile::new().unwrap();
	f.write_all(b"[1, 2, 3]").unwrap();
	f.flush().unwrap();
	let err = load_structured_output_schema(f.path().to_str().unwrap())
		.expect_err("array must be rejected")
		.to_string();
	assert!(
		err.contains("must contain a JSON object"),
		"unexpected error: {err}"
	);
}

#[test]
fn rejects_invalid_json() {
	let mut f = tempfile::NamedTempFile::new().unwrap();
	f.write_all(b"{not valid json").unwrap();
	f.flush().unwrap();
	let err = load_structured_output_schema(f.path().to_str().unwrap())
		.expect_err("invalid json must be rejected")
		.to_string();
	assert!(err.contains("Invalid JSON"), "unexpected error: {err}");
}

#[test]
fn reports_missing_schema_file() {
	let err = load_structured_output_schema("/nonexistent/path/schema-xyzzy.json")
		.expect_err("missing file must error")
		.to_string();
	assert!(
		err.contains("Failed to read schema file"),
		"unexpected error: {err}"
	);
}
