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

//! Tap management — Homebrew-style registry source list.
//!
//! Taps are Git repositories containing agent manifests.
//!
//! ## Usage
//! - `octomind tap user/repo` — clones https://github.com/user/octomind-repo
//! - `octomind tap user/repo /path/to/local` — uses local directory (no clone)
//! - `octomind tap` — lists all taps
//! - `octomind untap user/repo` — removes a tap
//!
//! ## Directory structure
//! - Taps cloned to: `~/.local/share/octomind/taps/user/octomind-repo/`
//! - Manifests expected at: `<tap>/agents/<category>/<variant>.toml`
//!
//! ## Priority
//! User taps (in order added) → built-in default (muvon/tap)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// The built-in default tap — always present as the last fallback.
pub const DEFAULT_TAP: &str = "muvon/tap";

/// A tap entry: either a GitHub repo or a local path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tap {
	/// Tap name in `user/repo` format.
	pub name: String,
	/// Local path if using a local directory (None for GitHub taps).
	pub local_path: Option<String>,
}

impl Tap {
	/// Returns the GitHub URL for this tap.
	pub fn github_url(&self) -> String {
		// user/repo → https://github.com/user/octomind-repo
		let parts: Vec<&str> = self.name.split('/').collect();
		if parts.len() == 2 {
			format!("https://github.com/{}/octomind-{}", parts[0], parts[1])
		} else {
			// Fallback: treat as full URL
			self.name.clone()
		}
	}

	/// Returns the local directory path for this tap.
	/// For GitHub taps: `~/.local/share/octomind/taps/user/octomind-repo/`
	/// For local taps: the specified local_path.
	pub fn local_dir(&self) -> Result<PathBuf> {
		if let Some(ref path) = self.local_path {
			expand_path(path)
		} else {
			let parts: Vec<&str> = self.name.split('/').collect();
			if parts.len() == 2 {
				let tap_dir = crate::directories::get_octomind_data_dir()?
					.join("taps")
					.join(parts[0])
					.join(format!("octomind-{}", parts[1]));
				Ok(tap_dir)
			} else {
				anyhow::bail!("Invalid tap name format: {}", self.name);
			}
		}
	}

	/// Returns the agents directory path for this tap.
	pub fn agents_dir(&self) -> Result<PathBuf> {
		Ok(self.local_dir()?.join("agents"))
	}
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TapsFile {
	#[serde(default)]
	taps: Vec<Tap>,
}

fn taps_file_path() -> Result<PathBuf> {
	Ok(crate::directories::get_octomind_data_dir()?.join("taps.toml"))
}

fn read_taps_file() -> Result<TapsFile> {
	let path = taps_file_path()?;
	if !path.exists() {
		return Ok(TapsFile::default());
	}
	let content = fs::read_to_string(&path)
		.context(format!("Failed to read taps file: {}", path.display()))?;
	toml::from_str(&content).context("Failed to parse taps.toml")
}

fn write_taps_file(taps: &TapsFile) -> Result<()> {
	let path = taps_file_path()?;
	let content = toml::to_string_pretty(taps).context("Failed to serialize taps")?;
	fs::write(&path, content).context(format!("Failed to write taps file: {}", path.display()))
}

/// Expand a path that may contain `~` or `./`.
fn expand_path(path: &str) -> Result<PathBuf> {
	if let Some(stripped) = path.strip_prefix("~/") {
		let home = dirs::home_dir().context("Cannot determine home directory")?;
		Ok(home.join(stripped))
	} else if let Some(stripped) = path.strip_prefix("./") {
		let cwd = std::env::current_dir().context("Cannot determine current directory")?;
		Ok(cwd.join(stripped))
	} else {
		Ok(PathBuf::from(path))
	}
}

/// Parse a tap argument in one of these formats:
/// - `user/repo` — GitHub tap
/// - `user/repo /path/to/local` — local tap
fn parse_tap_arg(arg: &str) -> Result<Tap> {
	let parts: Vec<&str> = arg.splitn(2, ' ').collect();
	let name = parts[0].trim().to_string();

	// Validate name format
	if !name.contains('/') || name.split('/').count() != 2 {
		anyhow::bail!("Tap name must be in 'user/repo' format, got: {}", name);
	}

	let local_path = if parts.len() == 2 {
		Some(parts[1].trim().to_string())
	} else {
		None
	};

	Ok(Tap { name, local_path })
}

/// Returns all active taps: user taps first, built-in default last.
/// Also auto-updates GitHub taps by running git pull (Homebrew-style).
pub fn load_taps() -> Result<Vec<Tap>> {
	let mut file = read_taps_file()?;

	// Ensure default tap is cloned (seamless first-time setup)
	ensure_default_tap()?;

	// Auto-update GitHub taps (seamless, like Homebrew)
	for tap in &file.taps {
		if tap.local_path.is_none() {
			if let Ok(tap_dir) = tap.local_dir() {
				if tap_dir.exists() {
					// Silently pull updates — don't block on failure
					let _ = git_pull(&tap_dir);
				}
			}
		}
	}

	// Built-in default is always last
	file.taps.push(Tap {
		name: DEFAULT_TAP.to_string(),
		local_path: None,
	});
	Ok(file.taps)
}

/// Ensure the default tap is cloned and updated (seamless first-time setup).
fn ensure_default_tap() -> Result<()> {
	let default_tap = Tap {
		name: DEFAULT_TAP.to_string(),
		local_path: None,
	};
	let tap_dir = default_tap.local_dir()?;
	if !tap_dir.exists() {
		let url = default_tap.github_url();
		crate::log_info!("Cloning default tap {}...", DEFAULT_TAP);
		git_clone(&url, &tap_dir)?;
	} else {
		// Silently pull updates — don't block on failure
		let _ = git_pull(&tap_dir);
	}
	Ok(())
}

/// Returns only user-added taps (excludes the built-in default).
pub fn list_taps() -> Result<Vec<Tap>> {
	Ok(read_taps_file()?.taps)
}

/// Add a tap. Clones from GitHub if not a local tap.
///
/// Format: `user/repo` or `user/repo /path/to/local`
pub fn add_tap(arg: &str) -> Result<()> {
	let tap = parse_tap_arg(arg)?;

	if tap.name == DEFAULT_TAP {
		anyhow::bail!(
			"'{}' is the built-in default tap — it's always active and cannot be re-added",
			tap.name
		);
	}

	let mut file = read_taps_file()?;
	if file.taps.iter().any(|t| t.name == tap.name) {
		anyhow::bail!("Tap '{}' is already added", tap.name);
	}

	// Clone from GitHub if not a local tap
	if tap.local_path.is_none() {
		let tap_dir = tap.local_dir()?;
		if !tap_dir.exists() {
			let url = tap.github_url();
			crate::log_info!("Cloning tap {}...", tap.name);
			git_clone(&url, &tap_dir)?;
		} else {
			crate::log_info!("Tap {} already cloned, updating...", tap.name);
			git_pull(&tap_dir)?;
		}
	} else {
		// Verify local path exists
		let local_dir = tap.local_dir()?;
		if !local_dir.exists() {
			anyhow::bail!(
				"Local tap directory does not exist: {}",
				local_dir.display()
			);
		}
	}

	file.taps.push(tap);
	write_taps_file(&file)?;
	Ok(())
}

/// Remove a tap by name.
pub fn remove_tap(name: &str) -> Result<()> {
	let name = name.trim().to_string();

	if name == DEFAULT_TAP {
		anyhow::bail!(
			"'{}' is the built-in default tap and cannot be removed",
			name
		);
	}

	let mut file = read_taps_file()?;
	let before = file.taps.len();
	file.taps.retain(|t| t.name != name);
	if file.taps.len() == before {
		anyhow::bail!("Tap '{}' is not in your tap list", name);
	}
	write_taps_file(&file)?;
	Ok(())
}

/// Clone a Git repository.
fn git_clone(url: &str, dir: &PathBuf) -> Result<()> {
	let status = std::process::Command::new("git")
		.args(["clone", "--depth", "1", url, &dir.to_string_lossy()])
		.status()
		.context("Failed to run git clone")?;

	if !status.success() {
		anyhow::bail!("Failed to clone tap from {}", url);
	}
	Ok(())
}

/// Pull latest changes for a tap.
fn git_pull(dir: &PathBuf) -> Result<()> {
	let status = std::process::Command::new("git")
		.args(["pull"])
		.current_dir(dir)
		.status()
		.context("Failed to run git pull")?;

	if !status.success() {
		crate::log_info!("Failed to update tap at {}", dir.display());
	}
	Ok(())
}
