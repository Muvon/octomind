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

//! Tests for the `/list` session command against a throwaway sessions
//! directory (`OCTOMIND_DATA_DIR`): pagination, page-parameter validation,
//! empty output, markdown rendering, and the current-session marker.

use super::*;
use crate::session::SessionInfo;
use serial_test::serial;
use zstd::stream::write::Encoder as ZstdEncoder;

struct TestDataDir {
	previous: Option<std::ffi::OsString>,
	dir: tempfile::TempDir,
}

impl TestDataDir {
	fn new() -> Self {
		let dir = tempfile::tempdir().expect("temporary data dir");
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self { previous, dir }
	}

	fn sessions_dir(&self) -> std::path::PathBuf {
		self.dir.path().join("sessions")
	}
}

impl Drop for TestDataDir {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(value) => std::env::set_var("OCTOMIND_DATA_DIR", value),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

fn test_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

/// Write a minimal session file: one zstd-compressed JSONL line holding a
/// SUMMARY entry, the format `list_available_sessions` scans for.
fn write_session_file(sessions_dir: &std::path::Path, name: &str, info: &SessionInfo) {
	std::fs::create_dir_all(sessions_dir).expect("create sessions dir");
	let file = std::fs::File::create(sessions_dir.join(format!("{name}.jsonl.zst")))
		.unwrap_or_else(|e| panic!("create session file {name}: {e}"));
	let mut encoder = ZstdEncoder::new(file, 0).expect("zstd encoder");
	let entry = serde_json::json!({"type": "SUMMARY", "session_info": info});
	std::io::Write::write_all(&mut encoder, format!("{entry}\n").as_bytes())
		.unwrap_or_else(|e| panic!("write summary {name}: {e}"));
	encoder.finish().expect("finish zstd stream");
}

fn session_info(name: &str, created_at: u64) -> SessionInfo {
	SessionInfo {
		name: name.to_string(),
		created_at,
		model: "octohub/big".to_string(),
		input_tokens: 1_234,
		output_tokens: 567,
		total_cost: 0.5,
		..Default::default()
	}
}

/// Run one `/list` invocation and return its typed output.
fn run(session: &ChatSession, config: &Config, params: &[&str]) -> CommandOutput {
	let result = handle_list(session, config, params)
		.unwrap_or_else(|e| panic!("list {params:?} errored: {e}"));
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	*output
}

#[tokio::test]
#[serial]
async fn test_empty_sessions_dir_reports_no_sessions() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let session = ChatSession::for_tests(Vec::new());
	let config = test_config();

	let output = run(&session, &config, &[]);
	let CommandOutput::List {
		sessions,
		total_sessions,
		total_pages,
		plain_text,
		..
	} = output
	else {
		panic!("expected List output");
	};
	assert_eq!(total_sessions, 0);
	assert_eq!(total_pages, 0);
	assert!(sessions.is_empty());
	assert_eq!(plain_text.as_deref(), Some("No sessions found."));
}

#[tokio::test]
#[serial]
async fn test_lists_sessions_with_metadata() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = TestDataDir::new();
	write_session_file(&data.sessions_dir(), "alpha", &session_info("alpha", 2_000));
	write_session_file(&data.sessions_dir(), "beta", &session_info("beta", 1_000));
	let session = ChatSession::for_tests(Vec::new());
	let config = test_config();

	let output = run(&session, &config, &[]);
	let CommandOutput::List {
		sessions,
		total_sessions,
		page,
		total_pages,
		plain_text,
	} = output
	else {
		panic!("expected List output");
	};
	assert_eq!(total_sessions, 2);
	assert_eq!(page, 1);
	assert_eq!(total_pages, 1);
	assert_eq!(sessions.len(), 2);

	// Sorted newest first: alpha (2000) before beta (1000)
	assert_eq!(sessions[0]["name"], "alpha");
	assert_eq!(sessions[0]["model"], "octohub/big");
	assert_eq!(sessions[0]["tokens"], 1_234 + 567);
	assert_eq!(sessions[0]["is_current"], false);

	let text = plain_text.expect("markdown");
	assert!(
		text.contains("# Available Sessions (Page 1 of 1)"),
		"text: {text}"
	);
	assert!(text.contains("Showing 2 of 2 sessions"), "text: {text}");
	assert!(text.contains("| alpha |"), "text: {text}");
	assert!(text.contains("| beta |"), "text: {text}");
	// Model prefix stripped in the table, tokens thousand-separated, cost row
	assert!(text.contains("| big |"), "text: {text}");
	assert!(text.contains("| 1,801 |"), "text: {text}");
	assert!(text.contains("$0.50000"), "text: {text}");
	assert!(text.contains("## Session Management"), "text: {text}");
}

#[tokio::test]
#[serial]
async fn test_page_parameter_must_be_positive_integer() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let session = ChatSession::for_tests(Vec::new());
	let config = test_config();

	for bad in ["0", "-1", "abc"] {
		let output = run(&session, &config, &[bad]);
		let CommandOutput::Error { error, .. } = output else {
			panic!("expected Error output for page {bad}");
		};
		assert_eq!(error, "Page number must be a positive integer");
	}
}

#[tokio::test]
#[serial]
async fn test_page_out_of_range() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = TestDataDir::new();
	write_session_file(&data.sessions_dir(), "only", &session_info("only", 1_000));
	let session = ChatSession::for_tests(Vec::new());
	let config = test_config();

	let output = run(&session, &config, &["2"]);
	let CommandOutput::Error { error, context } = output else {
		panic!("expected Error output");
	};
	assert_eq!(error, "Page 2 not found. Total pages: 1");
	assert_eq!(context.and_then(|c| c["total_pages"].as_i64()), Some(1));
}

#[tokio::test]
#[serial]
async fn test_pagination_splits_pages_and_renders_navigation() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = TestDataDir::new();
	// 16 sessions: s01 oldest … s16 newest; 15 per page → 2 pages
	for i in 1..=16 {
		let name = format!("s{i:02}");
		write_session_file(&data.sessions_dir(), &name, &session_info(&name, i * 100));
	}
	let session = ChatSession::for_tests(Vec::new());
	let config = test_config();

	let first = run(&session, &config, &["1"]);
	let CommandOutput::List {
		sessions,
		total_sessions,
		total_pages,
		plain_text,
		..
	} = first
	else {
		panic!("expected List output");
	};
	assert_eq!(total_sessions, 16);
	assert_eq!(total_pages, 2);
	assert_eq!(sessions.len(), 15);
	let text = plain_text.expect("markdown");
	assert!(text.contains("Showing 15 of 16 sessions"), "text: {text}");
	assert!(text.contains("- Next: `/list 2`"), "text: {text}");
	assert!(!text.contains("Previous"), "text: {text}");

	let second = run(&session, &config, &["2"]);
	let CommandOutput::List {
		sessions,
		plain_text,
		..
	} = second
	else {
		panic!("expected List output");
	};
	assert_eq!(sessions.len(), 1);
	assert_eq!(sessions[0]["name"], "s01", "oldest session lands on page 2");
	let text = plain_text.expect("markdown");
	assert!(text.contains("Showing 1 of 16 sessions"), "text: {text}");
	assert!(text.contains("- Previous: `/list 1`"), "text: {text}");
	assert!(!text.contains("- Next:"), "text: {text}");
}

#[tokio::test]
#[serial]
async fn test_current_session_is_marked() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = TestDataDir::new();
	write_session_file(&data.sessions_dir(), "alpha", &session_info("alpha", 2_000));
	write_session_file(&data.sessions_dir(), "beta", &session_info("beta", 1_000));
	let mut session = ChatSession::for_tests(Vec::new());
	// handle_list marks the session whose file_stem equals the scanned name
	// ("alpha" from "alpha.jsonl.zst"), so the current-session path must stem
	// to "alpha".
	session.session.session_file = Some(data.sessions_dir().join("alpha.jsonl"));
	let config = test_config();

	let output = run(&session, &config, &[]);
	let CommandOutput::List {
		sessions,
		plain_text,
		..
	} = output
	else {
		panic!("expected List output");
	};
	assert_eq!(sessions[0]["name"], "alpha");
	assert_eq!(sessions[0]["is_current"], true);
	assert_eq!(sessions[1]["is_current"], false);
	let text = plain_text.expect("markdown");
	assert!(text.contains("**alpha** *(current)*"), "text: {text}");
	assert!(!text.contains("**beta**"), "text: {text}");
}
