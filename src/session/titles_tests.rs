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

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir for the duration of a test,
/// restoring the previous value on drop. Tests using it must be `#[serial]`
/// because env vars are process-global.
struct DataDirGuard {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl DataDirGuard {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("failed to create tempdir");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for DataDirGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

#[test]
fn default_meta_has_all_fields_none() {
	let meta = SessionMeta::default();
	assert!(meta.title.is_none());
	assert!(meta.role.is_none());
	assert!(meta.model.is_none());
}

#[test]
fn max_title_chars_is_160() {
	assert_eq!(MAX_TITLE_CHARS, 160);
}

#[test]
fn meta_serde_round_trip_skips_none_fields() {
	let empty = SessionMeta::default();
	assert_eq!(serde_json::to_string(&empty).unwrap(), "{}");

	let full = SessionMeta {
		title: Some("t".into()),
		role: Some("developer".into()),
		model: Some("openrouter:m".into()),
	};
	let json = serde_json::to_string(&full).unwrap();
	let back: SessionMeta = serde_json::from_str(&json).unwrap();
	assert_eq!(back.title.as_deref(), Some("t"));
	assert_eq!(back.role.as_deref(), Some("developer"));
	assert_eq!(back.model.as_deref(), Some("openrouter:m"));

	// Absent fields deserialize to None (serde default)
	let partial: SessionMeta = serde_json::from_str(r#"{"role":"dev"}"#).unwrap();
	assert!(partial.title.is_none());
	assert_eq!(partial.role.as_deref(), Some("dev"));
}

#[test]
#[serial_test::serial]
fn missing_session_has_no_meta() {
	let _guard = DataDirGuard::new();
	assert!(get_session_meta("no-such-session").is_none());
}

#[test]
#[serial_test::serial]
fn set_session_title_trims_whitespace() {
	let _guard = DataDirGuard::new();
	let applied = set_session_title("s", Some("  hello world  "), None, None).unwrap();
	assert_eq!(applied.as_deref(), Some("hello world"));
	assert_eq!(
		get_session_meta("s").unwrap().title.as_deref(),
		Some("hello world")
	);
}

#[test]
#[serial_test::serial]
fn set_session_title_caps_at_max_chars() {
	let _guard = DataDirGuard::new();
	let long = format!("  {}  ", "x".repeat(MAX_TITLE_CHARS + 40));
	let applied = set_session_title("s", Some(long.as_str()), None, None)
		.unwrap()
		.expect("non-empty title");
	assert_eq!(applied.chars().count(), MAX_TITLE_CHARS);
	assert!(applied.chars().all(|c| c == 'x'));
}

#[test]
#[serial_test::serial]
fn set_session_title_caps_on_char_boundary_for_multibyte() {
	let _guard = DataDirGuard::new();
	let long = "é".repeat(MAX_TITLE_CHARS + 10);
	let applied = set_session_title("s", Some(long.as_str()), None, None)
		.unwrap()
		.expect("non-empty title");
	// Capped by chars, not bytes — é is 2 bytes in UTF-8.
	assert_eq!(applied.chars().count(), MAX_TITLE_CHARS);
	assert_eq!(applied.len(), MAX_TITLE_CHARS * 2);
}

#[test]
#[serial_test::serial]
fn whitespace_only_title_clears() {
	let _guard = DataDirGuard::new();
	set_session_title("s", Some("real title"), None, None).unwrap();
	let applied = set_session_title("s", Some("   "), None, None).unwrap();
	assert_eq!(applied, None);
	assert!(get_session_meta("s").unwrap().title.is_none());
}

#[test]
#[serial_test::serial]
fn none_title_clears_but_keeps_role_and_model() {
	let _guard = DataDirGuard::new();
	set_session_title("s", Some("title"), Some("developer"), Some("openrouter:m")).unwrap();
	let applied = set_session_title("s", None, None, None).unwrap();
	assert_eq!(applied, None);
	let meta = get_session_meta("s").unwrap();
	assert!(meta.title.is_none());
	assert_eq!(meta.role.as_deref(), Some("developer"));
	assert_eq!(meta.model.as_deref(), Some("openrouter:m"));
}

#[test]
#[serial_test::serial]
fn set_session_title_updates_role_and_model_snapshots() {
	let _guard = DataDirGuard::new();
	set_session_title(
		"s",
		Some("t"),
		Some("developer:general"),
		Some("anthropic:claude-sonnet-4-5"),
	)
	.unwrap();
	let meta = get_session_meta("s").unwrap();
	assert_eq!(meta.role.as_deref(), Some("developer:general"));
	assert_eq!(meta.model.as_deref(), Some("anthropic:claude-sonnet-4-5"));
}

#[test]
#[serial_test::serial]
fn record_session_meta_stores_role_and_model_without_title() {
	let _guard = DataDirGuard::new();
	record_session_meta("s", "developer", "openrouter:m");
	let meta = get_session_meta("s").unwrap();
	assert!(meta.title.is_none());
	assert_eq!(meta.role.as_deref(), Some("developer"));
	assert_eq!(meta.model.as_deref(), Some("openrouter:m"));
}

#[test]
#[serial_test::serial]
fn record_session_meta_preserves_existing_title() {
	let _guard = DataDirGuard::new();
	set_session_title("s", Some("keep me"), None, None).unwrap();
	record_session_meta("s", "developer", "openrouter:m");
	assert_eq!(
		get_session_meta("s").unwrap().title.as_deref(),
		Some("keep me")
	);
}

#[test]
#[serial_test::serial]
fn corrupt_store_falls_back_to_empty_and_recovers() {
	let _guard = DataDirGuard::new();
	let sessions = crate::directories::get_sessions_dir().unwrap();
	std::fs::write(sessions.join("titles.json"), "not valid json {{{").unwrap();
	assert!(get_session_meta("s").is_none());
	// The store still accepts writes after corruption
	set_session_title("s", Some("fresh"), None, None).unwrap();
	assert_eq!(
		get_session_meta("s").unwrap().title.as_deref(),
		Some("fresh")
	);
}
