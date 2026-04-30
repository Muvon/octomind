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

//! Handles `[deps] require = [...]` in agent manifests.
//!
//! Each entry like `"astral-sh/uv"` maps to `<tap_root>/deps/astral-sh/uv.sh`.
//! Scripts are run in order before MCP initialisation. They must be idempotent:
//! exit 0 immediately if the tool is already installed, exit 1 on failure.
//!
//! Output contract:
//! - stdout is suppressed (reserved for Octomind)
//! - stderr is inherited so the user sees install progress
//! - exit 0 = ok, exit non-zero = abort with error

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Stdio;

/// Parse `[deps] require = [...]` from a manifest TOML and run each script.
///
/// `tap_root` is the root directory of the tap (e.g. `~/.local/share/octomind/taps/muvon/octomind-tap/`).
/// Scripts are expected at `<tap_root>/deps/<org>/<tool>.sh`.
///
/// `status_cb` is called with a human-readable status string before each dep runs (e.g. for spinner updates).
///
/// Runs after INPUT and ENV resolution, before MCP initialisation.
pub async fn resolve_deps(
	manifest_toml: &str,
	tap_root: &Path,
	status_cb: Option<&(dyn Fn(&str) + Send + Sync)>,
) -> Result<()> {
	let entries = parse_dep_entries(manifest_toml)?;
	run_dep_entries(&entries, tap_root, status_cb).await
}

/// Run a list of `org/tool` dep entries against `<tap_root>/deps/<org>/<tool>.sh`.
///
/// Lower-level entry point used by both `resolve_deps` (boot-time, parses from a
/// manifest) and runtime capability activation (already has the list in hand,
/// no manifest to round-trip through).
pub async fn run_dep_entries(
	entries: &[String],
	tap_root: &Path,
	status_cb: Option<&(dyn Fn(&str) + Send + Sync)>,
) -> Result<()> {
	if entries.is_empty() {
		return Ok(());
	}

	let deps_root = tap_root.join("deps");

	for entry in entries {
		if let Some(cb) = status_cb {
			cb(&format!("Checking dep: {entry}"));
		} else {
			crate::log_debug!("checking dep: {}", entry);
		}
		run_dep_script(entry, &deps_root)
			.with_context(|| format!("Dependency '{entry}' failed"))?;
	}

	Ok(())
}

/// Extract `[deps] require` entries from the manifest TOML.
/// Returns an empty vec if the section is absent.
fn parse_dep_entries(toml_str: &str) -> Result<Vec<String>> {
	let value: toml::Value =
		toml::from_str(toml_str).context("Failed to parse manifest TOML for deps")?;

	let Some(deps) = value.get("deps") else {
		return Ok(vec![]);
	};

	let Some(require) = deps.get("require") else {
		return Ok(vec![]);
	};

	let toml::Value::Array(arr) = require else {
		anyhow::bail!("[deps] require must be an array of strings");
	};

	arr.iter()
		.map(|v| match v {
			toml::Value::String(s) => Ok(s.clone()),
			_ => anyhow::bail!("[deps] require entries must be strings"),
		})
		.collect()
}

/// Run a single dep script synchronously.
///
/// `entry` is `"org/tool"` — maps to `<deps_root>/org/tool.sh`.
/// stdout and stderr are suppressed; progress is reported via the caller's status callback.
fn run_dep_script(entry: &str, deps_root: &Path) -> Result<()> {
	let script_path = deps_root.join(format!("{entry}.sh"));

	if !script_path.exists() {
		anyhow::bail!(
			"Dep script not found: {} (looked in {})",
			entry,
			script_path.display()
		);
	}

	crate::log_debug!("running dep script: {}", entry);

	let output = std::process::Command::new("bash")
		.arg(&script_path)
		.stdin(Stdio::null()) // never inherit parent stdin (piped prompt)
		.stdout(Stdio::null()) // stdout reserved for Octomind
		.stderr(Stdio::piped()) // capture stderr for error reporting
		.output()
		.with_context(|| format!("Failed to execute dep script: {}", script_path.display()))?;

	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr);
		let stderr_msg = if stderr.trim().is_empty() {
			String::new()
		} else {
			format!("\n{}", stderr.trim())
		};
		anyhow::bail!(
			"Dep script '{}' exited with status {}{}",
			entry,
			output.status.code().unwrap_or(-1),
			stderr_msg
		);
	}

	Ok(())
}
