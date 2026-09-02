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

/// Run a list of `org/tool` dep entries against `<tap_root>/deps/<org>/<tool>.sh`.
///
/// `tap_root` is the tap that owns the entries (e.g.
/// `~/.local/share/octomind/taps/muvon/octomind-tap/`): boot-time resolution
/// groups a manifest's deps by owning tap, runtime capability activation
/// passes the capability's own list.
///
/// `status_cb` is called with a human-readable status string before each dep
/// runs (e.g. for spinner updates).
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

	#[cfg(windows)]
	let mut command = std::process::Command::new(bash_path());
	#[cfg(not(windows))]
	let mut command = std::process::Command::new("bash");
	#[cfg(windows)]
	command.arg(script_path.to_string_lossy().replace('\\', "/"));
	#[cfg(not(windows))]
	command.arg(&script_path);

	let output = command
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

/// Locate bash on Windows. Plain "bash" on PATH often resolves to the WSL
/// stub in System32, which exits 1 without a distro installed and can't run
/// tap scripts — prefer Git Bash explicitly.
#[cfg(windows)]
pub(crate) fn bash_path() -> std::path::PathBuf {
	for var in ["ProgramFiles", "ProgramFiles(x86)"] {
		if let Some(pf) = std::env::var_os(var) {
			let candidate = Path::new(&pf).join("Git").join("bin").join("bash.exe");
			if candidate.exists() {
				return candidate;
			}
		}
	}
	std::path::PathBuf::from("bash")
}

#[cfg(test)]
#[path = "deps_tests.rs"]
mod tests;
