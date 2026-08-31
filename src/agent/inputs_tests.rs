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

// --- interactive prompt paths (stdin at EOF) -------------------------------

/// Replace fd 0 with `/dev/null` for the duration of a test so the blocking
/// `prompt_user` stdin read returns EOF immediately and deterministically,
/// whatever stdin the test harness inherited.
#[cfg(unix)]
struct StdinNullGuard {
	saved_fd: i32,
}

#[cfg(unix)]
impl StdinNullGuard {
	#[cfg(unix)]
	fn new() -> Self {
		use std::os::unix::io::AsRawFd;
		let saved_fd = unsafe { libc::dup(0) };
		assert!(saved_fd >= 0, "failed to dup stdin for the guard");
		let devnull = std::fs::File::open("/dev/null").expect("open /dev/null");
		unsafe {
			assert!(
				libc::dup2(devnull.as_raw_fd(), 0) == 0,
				"failed to redirect stdin"
			);
		}
		Self { saved_fd }
	}
}

#[cfg(unix)]
impl Drop for StdinNullGuard {
	fn drop(&mut self) {
		unsafe {
			assert!(libc::dup2(self.saved_fd, 0) == 0, "failed to restore stdin");
			libc::close(self.saved_fd);
		}
	}
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial]
async fn resolve_inputs_prompts_on_missing_key_and_persists_the_eof_value() {
	let _guard = DataDirGuard::new();
	let _stdin = StdinNullGuard::new();

	// No stored value → prompt_user runs; stdin is at EOF so the value is the
	// empty string, which must be persisted so later runs stop prompting.
	let resolved = resolve_inputs("token={{INPUT:ProbeMissingKey}}")
		.await
		.expect("resolve with missing key");
	assert_eq!(resolved, "token=");

	let data_dir = std::env::var("OCTOMIND_DATA_DIR").expect("data dir is set");
	let stored = std::fs::read_to_string(std::path::Path::new(&data_dir).join("inputs.toml"))
		.expect("inputs.toml persisted after prompting");
	assert!(
		stored.contains("ProbeMissingKey"),
		"prompted key must be persisted: {stored}"
	);

	// A second resolution reads the stored value instead of prompting again.
	let again = resolve_inputs("token={{INPUT:ProbeMissingKey}}")
		.await
		.expect("resolve with stored key");
	assert_eq!(again, "token=");
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial]
async fn resolve_env_vars_prompts_missing_key_writes_dotenv_and_sets_process_env() {
	let dir = tempfile::tempdir().expect("tempdir for cwd");
	let previous_cwd = std::env::current_dir().expect("current dir");
	std::env::set_current_dir(dir.path()).expect("enter temp cwd");
	// Unique key so the process-env mutation cannot clash with anything else.
	const KEY: &str = "OCTOMIND_PROBE_ENV_KEY";
	let had_value = std::env::var_os(KEY);
	std::env::remove_var(KEY);
	let _stdin = StdinNullGuard::new();

	let resolved = resolve_env_vars("key={{ENV:OCTOMIND_PROBE_ENV_KEY}}")
		.await
		.expect("resolve with missing env key");
	assert_eq!(resolved, "key=");

	// The EOF value is persisted to ./.env and set for the running process.
	assert_eq!(
		std::env::var(KEY).as_deref(),
		Ok(""),
		"prompted env value must be set in the process"
	);
	let dotenv = std::fs::read_to_string(dir.path().join(".env")).expect(".env written");
	assert!(
		dotenv.contains(&format!("{KEY}=")),
		"key must be appended to .env: {dotenv}"
	);

	// Restore the process state the test mutated.
	std::env::set_current_dir(previous_cwd).expect("restore cwd");
	match had_value {
		Some(value) => std::env::set_var(KEY, value),
		None => std::env::remove_var(KEY),
	}
}
