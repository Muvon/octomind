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

//! Linux sandbox backend using Landlock LSM.
//!
//! Landlock is available since kernel 5.13. On older kernels the crate
//! operates in best-effort mode and we log a warning instead of failing.
//!
//! Access model:
//! - Landlock rules are purely additive (union) — a parent grant cannot be
//!   revoked for a subpath. This means read-blocking specific dirs (e.g. ~/.ssh)
//!   while allowing reads everywhere else is not possible with Landlock v1-v3.
//! - We therefore apply the practical safe default:
//!   • Read-only (ReadFile | ReadDir | Execute) on `/` — whole filesystem readable
//!   • Read+write on cwd, ~/.local/share (MCP server state/logs) and
//!   <config_dir>/octolib (provider auth state, e.g. ChatGPT token refresh)
//!   • All writes outside those paths are denied
//! - Sensitive credential dirs (~/.ssh, ~/.aws, etc.) are protected from
//!   *writes* but remain readable. This matches the macOS write-only baseline.
//!   True read isolation on Linux requires namespaces/seccomp — out of scope here.

use anyhow::Result;
use landlock::{
	make_bitflags, path_beneath_rules, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr,
	RulesetStatus, ABI,
};
use std::env;

pub fn apply(dir: &std::path::Path) -> Result<()> {
	let abi = ABI::V3;
	let all_access = AccessFs::from_all(abi);
	// Read-only rights: read files, list dirs, execute — intersected with ABI support.
	let read_only = make_bitflags!(AccessFs::{ReadFile | ReadDir | Execute}) & all_access;

	let home = dirs::home_dir().unwrap_or_default();
	let xdg_data_home = home.join(".local").join("share");
	// octolib keeps provider OAuth state here (ChatGPT rewrites its token file and
	// takes a refresh lock on every token refresh).
	let octolib_config = dirs::config_dir()
		.ok_or_else(|| anyhow::anyhow!("sandbox: could not determine config directory"))?
		.join("octolib");

	let mut ruleset = Ruleset::default().handle_access(all_access)?.create()?;

	// Read-only on the whole filesystem so MCP servers can read system libs,
	// configs, and project files outside cwd.
	ruleset = ruleset.add_rules(path_beneath_rules(["/"], read_only))?;

	// Read+write on cwd, XDG data home (MCP server logs/state) and octolib auth state.
	let tmp_dir = env::temp_dir();
	let mut rw_paths: Vec<&std::path::Path> = vec![dir, tmp_dir.as_path()];
	if xdg_data_home.exists() {
		rw_paths.push(xdg_data_home.as_path());
	}
	if octolib_config.exists() {
		rw_paths.push(octolib_config.as_path());
	}
	// Common device nodes needed for standard shell operations.
	for dev in [
		"/dev/null",
		"/dev/zero",
		"/dev/full",
		"/dev/tty",
		"/dev/pts",
	] {
		if let Ok(path) = std::path::Path::new(dev).canonicalize() {
			ruleset = ruleset.add_rules(path_beneath_rules([path], all_access))?;
		} else if std::path::Path::new(dev).exists() {
			ruleset = ruleset.add_rules(path_beneath_rules([dev], all_access))?;
		}
	}
	ruleset = ruleset.add_rules(path_beneath_rules(rw_paths, all_access))?;

	let status = ruleset.restrict_self()?;

	match status.ruleset {
		RulesetStatus::FullyEnforced => {
			crate::log_info!(
				"Sandbox active (Landlock fully enforced): writes locked to {}",
				dir.display()
			);
		}
		RulesetStatus::PartiallyEnforced => {
			crate::log_info!(
				"Sandbox active (Landlock partially enforced — older kernel): writes locked to {}",
				dir.display()
			);
		}
		RulesetStatus::NotEnforced => {
			crate::log_info!(
				"Sandbox requested but Landlock is not enforced on this kernel — running without restrictions."
			);
		}
	}

	Ok(())
}

#[cfg(test)]
#[path = "linux_tests.rs"]
mod tests;
