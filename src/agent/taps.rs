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
//! - `octomind tap user/repo /path/to/local` — symlinks local directory into taps dir
//! - `octomind tap` — lists all taps
//! - `octomind untap user/repo` — removes a tap
//!
//! ## Directory structure
//! - GitHub taps cloned to: `~/.local/share/octomind/taps/user/octomind-repo/`
//! - Local taps symlinked to: `~/.local/share/octomind/taps/user/octomind-repo/ -> /your/path`
//! - Manifests expected at: `<tap>/agents/<category>/<variant>.toml`
//!
//! ## Priority
//! User taps (in order added) → built-in default (muvon/tap)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;

/// The built-in default tap — always present as the last fallback.
pub const DEFAULT_TAP: &str = "muvon/tap";

/// A tap entry: either a GitHub repo or a local path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tap {
	/// Tap name in `user/repo` format.
	pub name: String,
	/// Original local path for local taps (None for GitHub taps). Stored for display only —
	/// the actual tap directory is always the standard symlink path under the taps dir.
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

	/// Returns the standard directory path for this tap.
	/// GitHub taps: `~/.local/share/octomind/taps/user/octomind-repo/` (git clone)
	/// Local taps:  `~/.local/share/octomind/taps/user/octomind-repo/` (symlink → local path)
	pub fn local_dir(&self) -> Result<PathBuf> {
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

	/// Returns the agents directory path for this tap.
	pub fn agents_dir(&self) -> Result<PathBuf> {
		Ok(self.local_dir()?.join("agents"))
	}

	/// Returns the deps directory path for this tap.
	pub fn deps_dir(&self) -> Result<PathBuf> {
		Ok(self.local_dir()?.join("deps"))
	}

	/// Returns the skills directory path for this tap.
	pub fn skills_dir(&self) -> Result<PathBuf> {
		Ok(self.local_dir()?.join("skills"))
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

/// Returns all active taps (local paths only, no network).
/// Use this for hot-path lookups (skill discovery, etc.) where taps are
/// already cloned/symlinked and git pull would add unnecessary latency.
pub fn get_taps() -> Result<Vec<Tap>> {
	let mut file = read_taps_file()?;
	file.taps.push(Tap {
		name: DEFAULT_TAP.to_string(),
		local_path: None,
	});
	Ok(file.taps)
}

/// Returns all active taps: user taps first, built-in default last.
/// Also auto-updates GitHub taps by running git pull (Homebrew-style).
pub fn load_taps() -> Result<Vec<Tap>> {
	let mut file = read_taps_file()?;

	// Ensure default tap is cloned (seamless first-time setup)
	ensure_default_tap()?;

	// Auto-update GitHub taps only (local taps are symlinks — always live)
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

/// Returns all available agent tags (`category:variant`) from all active taps.
///
/// Uses only locally cached tap data — no network calls. First tap wins on duplicates
/// (same priority as `fetch_manifest`). Result is sorted alphabetically.
pub fn list_agent_tags() -> Result<Vec<String>> {
	let taps = get_taps()?;
	let mut tags: Vec<String> = Vec::new();
	let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

	for tap in &taps {
		let agents_dir = match tap.agents_dir() {
			Ok(d) if d.exists() => d,
			_ => continue,
		};
		let category_entries = match fs::read_dir(&agents_dir) {
			Ok(e) => e,
			Err(_) => continue,
		};
		for category_entry in category_entries.flatten() {
			let category_path = category_entry.path();
			if !category_path.is_dir() {
				continue;
			}
			let category = match category_path.file_name().and_then(|n| n.to_str()) {
				Some(c) => c.to_string(),
				None => continue,
			};
			let variant_entries = match fs::read_dir(&category_path) {
				Ok(e) => e,
				Err(_) => continue,
			};
			for variant_entry in variant_entries.flatten() {
				let variant_path = variant_entry.path();
				if variant_path.extension().and_then(|e| e.to_str()) != Some("toml") {
					continue;
				}
				let variant = match variant_path.file_stem().and_then(|n| n.to_str()) {
					Some(v) => v.to_string(),
					None => continue,
				};
				let tag = format!("{category}:{variant}");
				if seen.insert(tag.clone()) {
					tags.push(tag);
				}
			}
		}
	}

	tags.sort();
	Ok(tags)
}

/// Add a tap. Clones from GitHub or creates a symlink for local taps.
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

	let tap_dir = tap.local_dir()?;

	if let Some(ref local_path) = tap.local_path {
		// Local tap: create a symlink so the tap dir always reflects the live local directory.
		let target = expand_path(local_path)?;
		if !target.exists() {
			anyhow::bail!("Local tap directory does not exist: {}", target.display());
		}
		// Create parent dirs (e.g. ~/.local/share/octomind/taps/user/)
		if let Some(parent) = tap_dir.parent() {
			fs::create_dir_all(parent).context(format!(
				"Failed to create tap parent dir: {}",
				parent.display()
			))?;
		}
		// Remove stale symlink/dir if it already exists at the target path
		if tap_dir.exists() || tap_dir.symlink_metadata().is_ok() {
			fs::remove_file(&tap_dir).context(format!(
				"Failed to remove existing tap path: {}",
				tap_dir.display()
			))?;
		}
		#[cfg(unix)]
		std::os::unix::fs::symlink(&target, &tap_dir).context(format!(
			"Failed to create symlink {} -> {}",
			tap_dir.display(),
			target.display()
		))?;
		#[cfg(windows)]
		std::os::windows::fs::symlink_dir(&target, &tap_dir).context(format!(
			"Failed to create symlink {} -> {}",
			tap_dir.display(),
			target.display()
		))?;
		crate::log_info!("Symlinked tap {} -> {}", tap.name, target.display());
	} else {
		// GitHub tap: clone or update
		if !tap_dir.exists() {
			let url = tap.github_url();
			crate::log_info!("Cloning tap {}...", tap.name);
			git_clone(&url, &tap_dir)?;
		} else {
			crate::log_info!("Tap {} already cloned, updating...", tap.name);
			git_pull(&tap_dir)?;
		}
	}

	file.taps.push(tap);
	write_taps_file(&file)?;
	Ok(())
}

/// Remove a tap by name. Also removes the symlink for local taps.
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
	let removed: Vec<Tap> = file
		.taps
		.iter()
		.filter(|t| t.name == name)
		.cloned()
		.collect();
	file.taps.retain(|t| t.name != name);
	if file.taps.len() == before {
		anyhow::bail!("Tap '{}' is not in your tap list", name);
	}

	// Remove the symlink for local taps (GitHub clones are left on disk intentionally)
	for tap in &removed {
		if tap.local_path.is_some() {
			if let Ok(tap_dir) = tap.local_dir() {
				if tap_dir.symlink_metadata().is_ok() {
					let _ = fs::remove_file(&tap_dir);
				}
			}
		}
	}

	write_taps_file(&file)?;
	Ok(())
}

/// Clone a Git repository. Stderr is captured and included in the error on failure.
fn git_clone(url: &str, dir: &std::path::Path) -> Result<()> {
	let output = std::process::Command::new("git")
		.args(["clone", "--depth", "1", url, &dir.to_string_lossy()])
		.stdout(Stdio::null())
		.stderr(Stdio::piped())
		.output()
		.context("Failed to run git clone")?;

	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
		anyhow::bail!(
			"Failed to clone tap from {}: {}",
			url,
			if stderr.is_empty() {
				"unknown git error".to_string()
			} else {
				stderr
			}
		);
	}
	Ok(())
}

/// Pull latest changes for a tap. Output is suppressed; only shown in debug mode.
fn git_pull(dir: &PathBuf) -> Result<()> {
	let output = std::process::Command::new("git")
		.args(["pull"])
		.current_dir(dir)
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.output()
		.context("Failed to run git pull")?;

	if !output.status.success() {
		crate::log_debug!(
			"Failed to update tap at {}: {}",
			dir.display(),
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	Ok(())
}
