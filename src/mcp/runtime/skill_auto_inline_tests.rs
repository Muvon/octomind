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
use serial_test::serial;

#[test]
fn intent_gate_rejects_short_acknowledgments() {
	// Common chatter that should never drive auto-activation.
	for short in [
		"", " ", "try", "ok", "yes", "no", "hmm", "do it", "thanks!", "what?",
	] {
		assert!(
			!intent_has_enough_signal(short),
			"expected {short:?} to be rejected by intent gate"
		);
	}
}

#[test]
fn intent_gate_accepts_two_word_intents() {
	// Real 2-word intents at the boundary should pass.
	for ok in [
		"run tests",
		"list files",
		"deploy app",
		"build code",
		"show me logs",
		"explain this code to me",
	] {
		assert!(
			intent_has_enough_signal(ok),
			"expected {ok:?} to pass intent gate"
		);
	}
}

#[test]
fn intent_gate_ignores_whitespace_padding() {
	// Pure whitespace and padded short inputs are still rejected.
	assert!(!intent_has_enough_signal("   \n\t  "));
	assert!(!intent_has_enough_signal("  try   "));
	// Whitespace doesn't pad a real intent up to the threshold.
	assert!(!intent_has_enough_signal("a b c"));
}

#[test]
fn system_managed_content_is_not_user_intent() {
	// Supervisor steers / recalls, skill replays and continuation wrappers
	// must never drive auto-activation — run_activation returns early on them.
	for synthetic in [
		"<pay-attention>\nYou have made several single-call turns in a row.\n</pay-attention>",
		"<recall>\npast-session lesson\n</recall>",
		"<system-note>\nbackground job finished\n</system-note>",
		"<skill name=\"tap-agent-authoring\" description=\"x\">\nbody\n</skill>",
		"<continuation>\n<task>resume</task>\n</continuation>",
	] {
		assert!(
			crate::session::is_system_managed_user_content(synthetic),
			"expected {synthetic:?} to be classified as system-managed"
		);
	}
	assert!(!crate::session::is_system_managed_user_content(
		"please create an agent manifest for developer:plan"
	));
}

// -------------------------------------------------------------------------
// strip_xml_blocks
// -------------------------------------------------------------------------

#[test]
fn strip_xml_no_tags_returns_borrowed_input() {
	let out = strip_xml_blocks("plain text with no markup");
	assert_eq!(out, "plain text with no markup");
	// Fast path must not allocate.
	assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
}

#[test]
fn strip_xml_removes_block_and_keeps_surrounding_text() {
	assert_eq!(
		strip_xml_blocks("before <log>noise</log> after"),
		"before  after"
	);
	// Multiline blocks are removed whole.
	assert_eq!(
		strip_xml_blocks("head\n<log>\nline1\nline2\n</log>\ntail"),
		"head\n\ntail"
	);
	assert_eq!(strip_xml_blocks("<skill name=\"x\">body</skill>"), "");
}

#[test]
fn strip_xml_removes_multiple_disjoint_blocks() {
	assert_eq!(strip_xml_blocks("a<x>1</x>b<y>2</y>c"), "abc");
}

#[test]
fn strip_xml_removes_outer_block_with_nested_inner_tags() {
	// The outer <a> block swallows the nested <b> block entirely.
	assert_eq!(
		strip_xml_blocks("keep <a>x<b>y</b>z</a> tail"),
		"keep  tail"
	);
}

#[test]
fn strip_xml_keeps_unclosed_tag_verbatim() {
	let out = strip_xml_blocks("hello <b>world");
	assert_eq!(out, "hello <b>world");
	assert!(matches!(out, std::borrow::Cow::Owned(_)));
}

#[test]
fn strip_xml_keeps_non_tag_lt_characters() {
	// Comparisons, empty tags, and "<3" are not block openers.
	assert_eq!(strip_xml_blocks("a < b"), "a < b");
	assert_eq!(strip_xml_blocks("x <> y"), "x <> y");
	assert_eq!(strip_xml_blocks("i <3 u"), "i <3 u");
}

#[test]
fn strip_xml_matches_close_tag_with_attributed_open_tag() {
	// Attributes on the open tag don't break close-tag matching.
	assert_eq!(strip_xml_blocks("<log type=\"err\">boom</log>"), "");
}

#[test]
fn strip_xml_block_content_may_contain_lt() {
	assert_eq!(strip_xml_blocks("<log>a<b and more</log>"), "");
}

#[test]
fn strip_xml_same_name_nesting_stops_at_first_close() {
	// Documents current behavior: the first matching close tag wins, so
	// the trailing "</a>" survives as literal text.
	assert_eq!(strip_xml_blocks("<a>x<a>y</a>z</a>"), "z</a>");
}

// -------------------------------------------------------------------------
// Intent gate
// -------------------------------------------------------------------------

#[test]
fn intent_gate_boundary_is_eight_non_whitespace_chars() {
	assert!(!intent_has_enough_signal("1234567")); // 7 non-ws chars
	assert!(intent_has_enough_signal("12345678")); // 8 non-ws chars
												// Whitespace never counts toward the threshold.
	assert!(intent_has_enough_signal("a b c d e f g h"));
	assert!(!intent_has_enough_signal("a b c d e f g"));
	// Multibyte chars count as chars, not bytes.
	assert!(!intent_has_enough_signal("日本語です")); // 5 chars
	assert!(intent_has_enough_signal("日本語テストです")); // 8 chars
}

// -------------------------------------------------------------------------
// Skills config
// -------------------------------------------------------------------------

#[test]
fn skills_config_defaults_outside_session() {
	// Outside a session scope the built-in defaults apply.
	let cfg = get_skills_config();
	assert!(cfg.auto_activation);
	assert!(cfg.auto_validation);
	assert_eq!(cfg.activation_timeout, 3);
	assert_eq!(cfg.validation_timeout, 60);
	assert_eq!(cfg.max_retries, 3);
}

// -------------------------------------------------------------------------
// Semantic score precomputation
// -------------------------------------------------------------------------

fn semantic_entry(name: &str, phrase: &str) -> PoolEntry {
	PoolEntry {
		name: name.to_string(),
		rules: vec![vec![crate::mcp::runtime::skill::ActivateCheck::Semantic {
			phrase: phrase.to_string(),
			threshold: 0.45,
		}]],
		evolution: None,
	}
}

#[tokio::test]
async fn semantic_scores_absent_when_no_semantic_checks() {
	let entries = vec![PoolEntry {
		name: "det-only".to_string(),
		rules: vec![vec![crate::mcp::runtime::skill::ActivateCheck::File(
			"Cargo.toml".to_string(),
		)]],
		evolution: None,
	}];
	// Returns before ever touching the embedding model.
	assert!(compute_semantic_scores("deploy the app", &entries, &[])
		.await
		.is_none());
}

#[tokio::test]
async fn semantic_scores_absent_when_semantic_skills_already_active() {
	let entries = vec![semantic_entry("sem", "deploying")];
	let active = vec!["sem".to_string()];
	assert!(compute_semantic_scores("deploy", &entries, &active)
		.await
		.is_none());
}

#[tokio::test]
async fn semantic_scores_absent_when_embedding_model_not_ready() {
	if crate::embeddings::is_ready() {
		// Another test in this binary may have warmed a locally-cached
		// model; the not-ready branch is then unreachable without a real
		// embed call, so there is nothing deterministic to assert.
		return;
	}
	let entries = vec![semantic_entry("sem", "deploying")];
	assert!(compute_semantic_scores("deploy", &entries, &[])
		.await
		.is_none());
}

// -------------------------------------------------------------------------
// Validator orchestration
// -------------------------------------------------------------------------

#[tokio::test]
async fn validators_return_empty_without_session() {
	let failures = run_validators("assistant text", std::path::Path::new("/tmp")).await;
	assert!(failures.is_empty());
}

#[tokio::test]
#[serial]
async fn validators_return_empty_when_auto_validation_disabled() {
	let sid = "__skillauto_no_validation".to_string();
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.skills.auto_validation = false;
	crate::session::context::set_session_config(&sid, &config);

	let failures = crate::session::context::with_session_id(sid.clone(), async {
		run_validators("assistant text", std::path::Path::new("/tmp")).await
	})
	.await;
	assert!(failures.is_empty());

	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
async fn validators_return_empty_when_no_active_skills() {
	let sid = "__skillauto_no_skills".to_string();
	let failures = crate::session::context::with_session_id(sid.clone(), async {
		run_validators("assistant text", std::path::Path::new("/tmp")).await
	})
	.await;
	assert!(failures.is_empty());
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn validators_return_empty_when_active_skill_has_no_validate_script() {
	let sid = "__skillauto_no_script".to_string();
	crate::session::context::add_active_skill(&sid, "skillauto-no-such-skill");

	let failures = crate::session::context::with_session_id(sid.clone(), async {
		run_validators("assistant text", std::path::Path::new("/tmp")).await
	})
	.await;
	// No installed tap carries that skill, so nothing is scheduled.
	assert!(failures.is_empty());

	crate::session::context::cleanup_session(&sid);
}

// -------------------------------------------------------------------------
// Validate script subprocess contract
// -------------------------------------------------------------------------

// Callers all run `#!/bin/sh` scripts and are `#[cfg(unix)]`-gated below;
// the helper itself stays cross-platform (chmod is Unix-only) so Windows
// test builds compile it without a dead-code warning.
#[cfg_attr(not(unix), allow(dead_code))]
fn write_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
	let path = dir.join("validate");
	std::fs::write(&path, body).expect("write validate script");
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		let mut perms = std::fs::metadata(&path)
			.expect("script metadata")
			.permissions();
		perms.set_mode(0o755);
		std::fs::set_permissions(&path, perms).expect("make script executable");
	}
	path
}

#[cfg(unix)]
#[tokio::test]
async fn validate_script_exit_zero_with_no_output_is_ok() {
	let dir = tempfile::tempdir().expect("tempdir");
	let script = write_script(dir.path(), "#!/bin/sh\nexit 0\n");
	let (code, output) =
		run_validate_script(&script, "content", dir.path(), Duration::from_secs(10))
			.await
			.expect("script runs");
	assert_eq!(code, 0);
	assert_eq!(output, "");
}

#[cfg(unix)]
#[tokio::test]
async fn validate_script_failure_captures_stderr() {
	let dir = tempfile::tempdir().expect("tempdir");
	let script = write_script(dir.path(), "#!/bin/sh\necho 'boom' >&2\nexit 1\n");
	let (code, output) =
		run_validate_script(&script, "content", dir.path(), Duration::from_secs(10))
			.await
			.expect("script runs");
	assert_eq!(code, 1);
	assert_eq!(output, "boom\n");
}

#[cfg(unix)]
#[tokio::test]
async fn validate_script_uses_stdout_when_stderr_empty() {
	let dir = tempfile::tempdir().expect("tempdir");
	let script = write_script(dir.path(), "#!/bin/sh\necho 'stdout noise'\nexit 2\n");
	let (code, output) =
		run_validate_script(&script, "content", dir.path(), Duration::from_secs(10))
			.await
			.expect("script runs");
	assert_eq!(code, 2);
	assert_eq!(output, "stdout noise\n");
}

#[cfg(unix)]
#[tokio::test]
async fn validate_script_receives_content_on_stdin() {
	let dir = tempfile::tempdir().expect("tempdir");
	let script = write_script(dir.path(), "#!/bin/sh\ncat >&2\nexit 1\n");
	let (code, output) = run_validate_script(
		&script,
		"ASSISTANT-BODY",
		dir.path(),
		Duration::from_secs(10),
	)
	.await
	.expect("script runs");
	assert_eq!(code, 1);
	assert_eq!(output, "ASSISTANT-BODY");
}

#[cfg(unix)]
#[tokio::test]
async fn validate_script_receives_assistant_role_arg() {
	let dir = tempfile::tempdir().expect("tempdir");
	let script = write_script(dir.path(), "#!/bin/sh\necho \"$1\" >&2\nexit 3\n");
	let (code, output) =
		run_validate_script(&script, "content", dir.path(), Duration::from_secs(10))
			.await
			.expect("script runs");
	assert_eq!(code, 3);
	assert_eq!(output, "assistant\n");
}

#[cfg(unix)]
#[tokio::test]
async fn validate_script_runs_in_workdir() {
	let dir = tempfile::tempdir().expect("tempdir");
	let script = write_script(dir.path(), "#!/bin/sh\npwd >&2\nexit 1\n");
	let (_code, output) =
		run_validate_script(&script, "content", dir.path(), Duration::from_secs(10))
			.await
			.expect("script runs");
	// `pwd` reports the physical path; canonicalize the tempdir to match.
	let canonical = std::fs::canonicalize(dir.path()).expect("canonicalize workdir");
	assert_eq!(output.trim(), canonical.to_str().expect("utf8 workdir"));
}

#[cfg(unix)]
#[tokio::test]
async fn validate_script_times_out() {
	let dir = tempfile::tempdir().expect("tempdir");
	let script = write_script(dir.path(), "#!/bin/sh\nsleep 5\n");
	let err = run_validate_script(&script, "content", dir.path(), Duration::from_millis(200))
		.await
		.expect_err("script must time out");
	assert!(err.to_string().contains("Validator timed out"));
}

#[tokio::test]
async fn validate_script_missing_script_is_error() {
	let err = run_validate_script(
		std::path::Path::new("/nonexistent/skillauto-validate"),
		"content",
		std::path::Path::new("/tmp"),
		Duration::from_secs(10),
	)
	.await
	.expect_err("spawn must fail");
	assert!(err.to_string().contains("Failed to spawn"));
}

// -------------------------------------------------------------------------
// Retry tracker and pool init
// -------------------------------------------------------------------------

#[test]
#[serial]
fn retry_tracker_counts_and_resets() {
	let tracker = get_retry_tracker();
	tracker.write().unwrap().clear();
	tracker
		.write()
		.unwrap()
		.insert("__skillauto_retry".to_string(), 2);
	assert_eq!(
		tracker.read().unwrap().get("__skillauto_retry").copied(),
		Some(2)
	);
	tracker.write().unwrap().remove("__skillauto_retry");
	assert!(tracker.read().unwrap().get("__skillauto_retry").is_none());
	tracker.write().unwrap().clear();
}

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir so tap enumeration sees an
/// empty tap set. Tests using it must be `#[serial]` (env is process-global).
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
#[serial]
fn init_pool_with_fresh_data_dir_yields_empty_pool_and_clears_retries() {
	let _guard = DataDirGuard::new();

	// Seed a stale retry counter — init_pool must clear it (new session pool).
	get_retry_tracker()
		.write()
		.unwrap()
		.insert("__skillauto_stale".to_string(), 9);

	init_pool("skillauto-test-domain");

	{
		let pool = get_pool().read().unwrap();
		let pool = pool.get("__default__").expect("pool initialized");
		assert!(
			pool.entries.is_empty(),
			"no taps in the fresh data dir, so no entries"
		);
	}
	assert!(
		get_retry_tracker()
			.read()
			.unwrap()
			.get("__skillauto_stale")
			.is_none(),
		"init_pool resets retry counters"
	);

	// Restore pre-test global state for other tests in this binary.
	get_pool().write().unwrap().clear();
	get_retry_tracker().write().unwrap().clear();
}
