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

//! External tests for `src/agent/inputs.rs` — key extraction, INPUT resolution
//! against the persistent store, ENV resolution against the process
//! environment, and the non-interactive fail-closed path. Tests touching
//! `OCTOMIND_DATA_DIR` or other env vars are `#[serial]` because env vars are
//! process-global; async ones also hold `ENV_LOCK`.

use super::*;

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir for the duration of a test,
/// restoring the previous value on drop.
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

// --- key extraction ---------------------------------------------------------

#[test]
fn extract_input_keys_dedupes_and_preserves_order() {
	let raw = "a {{INPUT:ONE}} b {{INPUT:TWO}} c {{INPUT:ONE}} d {{INPUT:}} e";
	assert_eq!(extract_input_keys(raw), vec!["ONE", "TWO"]);
}

#[test]
fn extract_input_keys_ignores_unterminated_placeholders() {
	// No closing braces after the prefix → scan stops, nothing extracted.
	assert!(extract_input_keys("{{INPUT:KEY").is_empty());
	assert!(extract_input_keys("plain text").is_empty());
	assert!(extract_input_keys("").is_empty());
}

#[test]
fn extract_env_keys_dedupes_and_ignores_malformed() {
	let raw = "{{ENV:A}} {{ENV:B}} {{ENV:A}} {{ENV:}} {{ENV:C";
	assert_eq!(extract_env_keys(raw), vec!["A", "B"]);
	assert!(extract_env_keys("no placeholders").is_empty());
	assert!(extract_env_keys("{{ENV:KEY").is_empty());
}

// --- non-interactive scope ----------------------------------------------------

#[test]
fn is_non_interactive_is_false_outside_scope() {
	assert!(!is_non_interactive());
}

#[tokio::test]
async fn with_non_interactive_sets_flag_inside_scope_only() {
	assert!(!is_non_interactive());
	let inner = with_non_interactive(async { is_non_interactive() }).await;
	assert!(inner, "flag must be set inside the scope");
	assert!(!is_non_interactive(), "flag must not leak out of the scope");
}

// --- resolve_inputs ------------------------------------------------------------

#[tokio::test]
async fn resolve_inputs_without_placeholders_is_passthrough() {
	// Early return before any store access — no data dir needed.
	let resolved = resolve_inputs("no placeholders at all").await.unwrap();
	assert_eq!(resolved, "no placeholders at all");
}

#[tokio::test]
#[serial_test::serial]
async fn resolve_inputs_substitutes_stored_values() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = DataDirGuard::new();
	// Non-string values in the store are ignored by the loader.
	fs::write(
		inputs_file_path().unwrap(),
		"token = \"abc\"\nhost = \"example.com\"\ncount = 5\n",
	)
	.unwrap();

	let resolved = resolve_inputs("connect {{INPUT:host}} with {{INPUT:token}} / {{INPUT:token}}")
		.await
		.unwrap();
	assert_eq!(resolved, "connect example.com with abc / abc");
}

#[tokio::test]
#[serial_test::serial]
async fn resolve_inputs_missing_key_fails_closed_when_non_interactive() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _data = DataDirGuard::new();
	// Empty store + non-interactive scope → structured error, no stdin read.
	let result =
		with_non_interactive(async { resolve_inputs("needs {{INPUT:MISSING_KEY}}").await }).await;
	let err = result.unwrap_err();
	assert!(err.to_string().contains("non-interactive"), "{err}");
	assert!(err.to_string().contains("MISSING_KEY"), "{err}");
}

// --- resolve_env_vars ------------------------------------------------------------

#[tokio::test]
#[serial_test::serial]
async fn resolve_env_vars_uses_process_environment() {
	let key = "OCTOMIND_INPUTS_TEST_ENV_KEY";
	std::env::set_var(key, "from-env");
	let raw = format!("url http://{{{{ENV:{key}}}}}/api");
	let resolved = resolve_env_vars(&raw).await.unwrap();
	std::env::remove_var(key);
	assert_eq!(resolved, "url http://from-env/api");
}

#[tokio::test]
#[serial_test::serial]
async fn resolve_env_vars_treats_empty_env_value_as_intentionally_set() {
	let key = "OCTOMIND_INPUTS_TEST_ENV_EMPTY";
	std::env::set_var(key, "");
	let raw = format!("x={{{{ENV:{key}}}}}");
	let resolved = resolve_env_vars(&raw).await.unwrap();
	std::env::remove_var(key);
	assert_eq!(resolved, "x=", "empty stored value must not re-prompt");
}

#[tokio::test]
#[serial_test::serial]
async fn resolve_env_vars_resolves_multiple_keys_and_occurrences() {
	let k1 = "OCTOMIND_INPUTS_TEST_MULTI_A";
	let k2 = "OCTOMIND_INPUTS_TEST_MULTI_B";
	std::env::set_var(k1, "1");
	std::env::set_var(k2, "2");
	let raw = format!("{{{{ENV:{k1}}}}}+{{{{ENV:{k2}}}}}+{{{{ENV:{k1}}}}}");
	let resolved = resolve_env_vars(&raw).await.unwrap();
	std::env::remove_var(k1);
	std::env::remove_var(k2);
	assert_eq!(resolved, "1+2+1");
}

#[tokio::test]
#[serial_test::serial]
async fn resolve_env_vars_missing_key_fails_closed_when_non_interactive() {
	let key = "OCTOMIND_INPUTS_TEST_ENV_MISSING";
	std::env::remove_var(key);
	let raw = format!("{{{{ENV:{key}}}}}");
	let result = with_non_interactive(async { resolve_env_vars(&raw).await }).await;
	let err = result.unwrap_err();
	assert!(err.to_string().contains("non-interactive"), "{err}");
	assert!(err.to_string().contains(key), "{err}");
}

#[tokio::test]
async fn resolve_env_vars_without_placeholders_is_passthrough() {
	let resolved = resolve_env_vars("plain text, nothing to resolve")
		.await
		.unwrap();
	assert_eq!(resolved, "plain text, nothing to resolve");
}
