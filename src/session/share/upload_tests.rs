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

fn user_line(content: &str) -> String {
	serde_json::json!({ "role": "user", "content": content }).to_string()
}

fn assistant_line(content: &str) -> String {
	serde_json::json!({ "role": "assistant", "content": content }).to_string()
}

#[test]
#[serial_test::serial]
fn web_host_honors_env_override_and_falls_back() {
	// All env mutation happens before the asserts so a failure cannot
	// leak a stray OCTOMIND_SHARE_URL into other tests.
	std::env::set_var("OCTOMIND_SHARE_URL", "http://localhost:5173");
	let overridden = web_host();
	std::env::set_var("OCTOMIND_SHARE_URL", "");
	let empty = web_host();
	std::env::remove_var("OCTOMIND_SHARE_URL");
	let unset = web_host();

	assert_eq!(overridden, "http://localhost:5173");
	assert_eq!(empty, DEFAULT_SHARE_HOST);
	assert_eq!(unset, DEFAULT_SHARE_HOST);
}

#[test]
fn gzip_emits_magic_bytes_and_round_trips() {
	let bytes: Vec<u8> = (0..=255u8).collect();
	let inputs: Vec<&[u8]> = vec![
		b"".as_slice(),
		b"hello".as_slice(),
		b"hello hello hello hello hello".as_slice(),
		&bytes,
	];
	for input in inputs {
		let compressed = gzip(input).expect("gzip must succeed");
		assert!(
			compressed.starts_with(&[0x1f, 0x8b]),
			"missing gzip magic for {input:?}"
		);
		let mut decoder = flate2::read::GzDecoder::new(compressed.as_slice());
		let mut out = Vec::new();
		decoder
			.read_to_end(&mut out)
			.expect("gzip stream must decode");
		assert_eq!(out.as_slice(), input);
	}
}

#[test]
fn extract_title_returns_first_user_message() {
	let jsonl = format!(
		"{}\n{}\n{}\n",
		"this line is not json",
		assistant_line("I can help with that."),
		user_line("Fix the login bug"),
	);
	assert_eq!(
		extract_title(jsonl.as_bytes()),
		Some("Fix the login bug".to_string())
	);
}

#[test]
fn extract_title_returns_none_without_user_messages() {
	let jsonl = format!("{}\n{}\n", "not json at all", assistant_line("hi"));
	assert_eq!(extract_title(jsonl.as_bytes()), None);
}

#[test]
fn extract_title_skips_octomind_managed_entries() {
	// A session whose only user entry is managed has no title.
	let only_managed = format!("{}\n", user_line("# Octomind session bootstrap"));
	assert_eq!(extract_title(only_managed.as_bytes()), None);

	// Managed entries are skipped in favor of the first real one.
	for managed in ["# Octomind session bootstrap", "## Lessons applied"] {
		let jsonl = format!("{}\n{}\n", user_line(managed), user_line("real task"));
		assert_eq!(
			extract_title(jsonl.as_bytes()),
			Some("real task".to_string()),
			"{managed:?} must be skipped"
		);
	}
}

#[test]
fn extract_title_truncates_to_200_chars() {
	let jsonl = format!("{}\n", user_line(&"x".repeat(300)));
	assert_eq!(extract_title(jsonl.as_bytes()), Some("x".repeat(200)));
}

#[test]
fn extract_title_takes_only_the_first_line() {
	let jsonl = format!("{}\n", user_line("first line\nsecond line"));
	assert_eq!(
		extract_title(jsonl.as_bytes()),
		Some("first line".to_string())
	);
}
