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

//! Handles `{{INPUT:KEY}}` and `{{ENV:KEY}}` placeholders in agent manifests.
//!
//! **`{{INPUT:KEY}}`** — persistent credential store:
//! 1. Loads previously stored values from `~/.local/share/octomind/inputs.toml`
//! 2. Prompts the user for any missing values (once, then saves them)
//! 3. Substitutes all placeholders and returns the resolved string
//!
//! **`{{ENV:KEY}}`** — environment variable with `.env` fallback:
//! 1. If `KEY` is set in the environment (and non-empty), use it directly
//! 2. Otherwise prompt the user, then persist to `./.env` in the current directory
//!    (the tool already loads `.env` automatically on next run)
//!
//! ENV resolution always runs after INPUT resolution.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};

const INPUT_PLACEHOLDER_PREFIX: &str = "{{INPUT:";
const INPUT_PLACEHOLDER_SUFFIX: &str = "}}";

const ENV_PLACEHOLDER_PREFIX: &str = "{{ENV:";
const ENV_PLACEHOLDER_SUFFIX: &str = "}}";

/// Extract all unique `{{INPUT:KEY}}` keys from a raw string.
fn extract_input_keys(raw: &str) -> Vec<String> {
	let mut keys = Vec::new();
	let mut search = raw;
	while let Some(start) = search.find(INPUT_PLACEHOLDER_PREFIX) {
		let after_prefix = &search[start + INPUT_PLACEHOLDER_PREFIX.len()..];
		if let Some(end) = after_prefix.find(INPUT_PLACEHOLDER_SUFFIX) {
			let key = after_prefix[..end].to_string();
			if !key.is_empty() && !keys.contains(&key) {
				keys.push(key);
			}
			search = &after_prefix[end + INPUT_PLACEHOLDER_SUFFIX.len()..];
		} else {
			break;
		}
	}
	keys
}

/// Path to the persistent inputs store.
fn inputs_file_path() -> Result<std::path::PathBuf> {
	Ok(crate::directories::get_octomind_data_dir()?.join("inputs.toml"))
}

/// Load all stored inputs from disk. Returns empty map if file doesn't exist.
fn load_inputs() -> Result<HashMap<String, String>> {
	let path = inputs_file_path()?;
	if !path.exists() {
		return Ok(HashMap::new());
	}
	let content = fs::read_to_string(&path)
		.context(format!("Failed to read inputs file: {}", path.display()))?;
	let table: toml::Table = toml::from_str(&content).context("Failed to parse inputs.toml")?;
	Ok(table
		.into_iter()
		.filter_map(|(k, v)| {
			if let toml::Value::String(s) = v {
				Some((k, s))
			} else {
				None
			}
		})
		.collect())
}

/// Persist a single key=value to the inputs store (upsert).
fn save_input(key: &str, value: &str) -> Result<()> {
	let path = inputs_file_path()?;
	let mut inputs = load_inputs()?;
	inputs.insert(key.to_string(), value.to_string());

	let mut table = toml::Table::new();
	for (k, v) in &inputs {
		table.insert(k.clone(), toml::Value::String(v.clone()));
	}
	let content =
		toml::to_string_pretty(&toml::Value::Table(table)).context("Failed to serialize inputs")?;
	fs::write(&path, content)
		.context(format!("Failed to write inputs file: {}", path.display()))?;
	Ok(())
}

/// Prompt the user for a value on stderr (so stdout stays clean for piped output).
fn prompt_user(key: &str) -> Result<String> {
	let stderr = io::stderr();
	let mut err = stderr.lock();
	write!(err, "Enter value for {key}: ").ok();
	err.flush().ok();

	let mut value = String::new();
	io::stdin()
		.read_line(&mut value)
		.context(format!("Failed to read input for {key}"))?;
	Ok(value.trim().to_string())
}

/// Resolve all `{{INPUT:KEY}}` placeholders in `raw`.
///
/// For each key:
/// - If already stored in `~/.local/share/octomind/inputs.toml`, use that value.
/// - Otherwise prompt the user, then save for future runs.
pub async fn resolve_inputs(raw: &str) -> Result<String> {
	let keys = extract_input_keys(raw);
	if keys.is_empty() {
		return Ok(raw.to_string());
	}

	let mut stored = load_inputs()?;
	let mut result = protect_escaped_braces(raw);

	for key in &keys {
		let value = if let Some(v) = stored.get(key) {
			v.clone()
		} else {
			let v = prompt_user(key)?;
			save_input(key, &v)?;
			stored.insert(key.clone(), v.clone());
			v
		};
		let placeholder = format!("{INPUT_PLACEHOLDER_PREFIX}{key}{INPUT_PLACEHOLDER_SUFFIX}");
		result = result.replace(&placeholder, &value);
	}

	Ok(restore_escaped_braces(&result))
}

/// Protect escaped `{{{{...}}}}` sequences before substitution by replacing them
/// with a sentinel that contains no `{{` so the substitution engine ignores them.
/// Call `restore_escaped_braces` after all substitutions to get the final `{{...}}`.
pub fn protect_escaped_braces(s: &str) -> String {
	s.replace("{{{{", "\x00LBRACE\x00")
		.replace("}}}}", "\x00RBRACE\x00")
}

/// Restore sentinels inserted by `protect_escaped_braces` back to literal `{{` / `}}`.
pub fn restore_escaped_braces(s: &str) -> String {
	s.replace("\x00LBRACE\x00", "{{")
		.replace("\x00RBRACE\x00", "}}")
}

/// Extract all unique `{{ENV:KEY}}` keys from a raw string.
fn extract_env_keys(raw: &str) -> Vec<String> {
	let mut keys = Vec::new();
	let mut search = raw;
	while let Some(start) = search.find(ENV_PLACEHOLDER_PREFIX) {
		let after_prefix = &search[start + ENV_PLACEHOLDER_PREFIX.len()..];
		if let Some(end) = after_prefix.find(ENV_PLACEHOLDER_SUFFIX) {
			let key = after_prefix[..end].to_string();
			if !key.is_empty() && !keys.contains(&key) {
				keys.push(key);
			}
			search = &after_prefix[end + ENV_PLACEHOLDER_SUFFIX.len()..];
		} else {
			break;
		}
	}
	keys
}

/// Append `KEY=VALUE` to `./.env` in the current working directory.
///
/// Creates the file if it doesn't exist. Appends so existing entries are preserved.
fn save_env_to_dotenv(key: &str, value: &str) -> Result<()> {
	let dotenv_path = std::path::Path::new(".env");
	let line = format!("{}={}\n", key, value);
	let mut file = fs::OpenOptions::new()
		.create(true)
		.append(true)
		.open(dotenv_path)
		.context(format!(
			"Failed to open .env for writing: {}",
			dotenv_path.display()
		))?;
	file.write_all(line.as_bytes())
		.context(format!("Failed to write {key} to .env"))?;
	Ok(())
}

/// Resolve all `{{ENV:KEY}}` placeholders in `raw`.
///
/// For each key:
/// - If already set in the environment (and non-empty), use it directly.
/// - Otherwise prompt the user, persist to `./.env`, then substitute.
///
/// Call this **after** `resolve_inputs()` so INPUT prompts come first.
pub async fn resolve_env_vars(raw: &str) -> Result<String> {
	let keys = extract_env_keys(raw);
	if keys.is_empty() {
		return Ok(raw.to_string());
	}

	let mut result = protect_escaped_braces(raw);

	for key in &keys {
		let value = match std::env::var(key) {
			Ok(v) if !v.trim().is_empty() => v,
			_ => {
				// Not in env — prompt and persist to .env so next run picks it up automatically
				let v = prompt_user(key)?;
				save_env_to_dotenv(key, &v)?;
				// Also set in current process so the session can use it immediately
				std::env::set_var(key, &v);
				v
			}
		};
		let placeholder = format!("{ENV_PLACEHOLDER_PREFIX}{key}{ENV_PLACEHOLDER_SUFFIX}");
		result = result.replace(&placeholder, &value);
	}

	Ok(restore_escaped_braces(&result))
}

#[cfg(test)]
mod tests {
	use super::*;

	// --- protect/restore escaped braces ---

	#[test]
	fn test_protect_restore_roundtrip() {
		// protect then restore converts {{{{...}}}} → {{...}} (that's the intended transformation)
		let input = "Use {{{{INPUT:KEY}}}} or {{{{ENV:URL}}}} as examples";
		assert_eq!(
			restore_escaped_braces(&protect_escaped_braces(input)),
			"Use {{INPUT:KEY}} or {{ENV:URL}} as examples"
		);
	}

	#[test]
	fn test_restore_basic() {
		// After protect+restore the escaped form becomes literal {{...}}
		let protected = protect_escaped_braces("{{{{INPUT:KEY}}}}");
		assert_eq!(restore_escaped_braces(&protected), "{{INPUT:KEY}}");
		let protected = protect_escaped_braces("{{{{ENV:KEY}}}}");
		assert_eq!(restore_escaped_braces(&protected), "{{ENV:KEY}}");
		let protected = protect_escaped_braces("{{{{CWD}}}}");
		assert_eq!(restore_escaped_braces(&protected), "{{CWD}}");
	}

	#[test]
	fn test_protect_hides_from_substitution() {
		// protect must replace {{ so a naive str.replace("{{CWD}}") won't match
		let protected = protect_escaped_braces("{{{{CWD}}}}");
		assert!(
			!protected.contains("{{"),
			"sentinel must not contain {{: {protected}"
		);
	}

	#[test]
	fn test_no_escaped_braces_unchanged() {
		// Strings without escape sequences pass through unchanged
		let plain = "no placeholders here";
		assert_eq!(protect_escaped_braces(plain), plain);
		assert_eq!(restore_escaped_braces(plain), plain);
	}

	#[test]
	fn test_multiple_escaped_occurrences() {
		let input = "Use {{{{INPUT:TOKEN}}}} or {{{{ENV:URL}}}} as examples";
		let result = restore_escaped_braces(&protect_escaped_braces(input));
		assert_eq!(result, "Use {{INPUT:TOKEN}} or {{ENV:URL}} as examples");
	}

	// --- process_placeholders_async_with_role (escaped syntax survives substitution) ---

	#[tokio::test]
	async fn test_escaped_placeholder_survives_substitution() {
		// {{{{CWD}}}} must not be replaced by the real CWD — it should become {{CWD}}
		let prompt = "Example: {{{{CWD}}}}";
		let dir = std::path::Path::new("/tmp");
		let result = crate::session::helper_functions::process_placeholders_async_with_role(
			prompt, dir, None,
		)
		.await;
		assert_eq!(result, "Example: {{CWD}}");
	}

	#[tokio::test]
	async fn test_real_and_escaped_placeholder_together() {
		// {{CWD}} gets replaced, {{{{CWD}}}} becomes literal {{CWD}}
		let prompt = "Real: {{CWD}}, Escaped: {{{{CWD}}}}";
		let dir = std::path::Path::new("/tmp");
		let result = crate::session::helper_functions::process_placeholders_async_with_role(
			prompt, dir, None,
		)
		.await;
		assert_eq!(result, "Real: /tmp, Escaped: {{CWD}}");
	}
}
