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
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

/// The built-in default tap — always present as the last fallback.
pub const DEFAULT_TAP: &str = "muvon/tap";

/// How often a GitHub tap is refreshed automatically. Every agent start reads the
/// taps, and a delegated specialist has seconds to answer its parent's
/// handshake, so a refresh starts at most once per interval for every process
/// sharing the data dir: the newer of git's `FETCH_HEAD` and `REFRESH_STAMP`
/// marks the last one.
const TAP_REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// File under `.git` touched when a background refresh is started. `FETCH_HEAD`
/// only changes when the pull finishes, which on a slow mount is long after
/// other processes have seen the same due refresh and started their own.
const REFRESH_STAMP: &str = "octomind-refresh";

/// Age past which a `.git/*.lock` has no live owner. A refresh that took 4.5
/// minutes on a degraded network mount is the slowest seen; an hour is far past it.
const STALE_LOCK_AGE: Duration = Duration::from_secs(60 * 60);

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

	/// Returns the workflows directory path for this tap.
	pub fn workflows_dir(&self) -> Result<PathBuf> {
		Ok(self.local_dir()?.join("workflows"))
	}

	/// Returns the plugins directory path for this tap (Agent Plugins 1.0.0 format).
	pub fn plugins_dir(&self) -> Result<PathBuf> {
		Ok(self.local_dir()?.join("plugins"))
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
	let content = fs::read_to_string(&path).context(format!("Failed to read taps file: {}", path.display()))?;
	toml::from_str(&content).context("Failed to parse taps.toml")
}

fn write_taps_file(taps: &TapsFile) -> Result<()> {
	let path = taps_file_path()?;
	let content = toml::to_string_pretty(taps).context("Failed to serialize taps")?;
	fs::write(&path, content).context(format!("Failed to write taps file: {}", path.display()))?;
	crate::agent::registry::invalidate_capability_catalog();
	Ok(())
}

/// Whether a GitHub tap is due for its automatic refresh. A clone that has
/// never fetched nor started a refresh is due at once.
fn refresh_due(tap_dir: &Path) -> bool {
	let git_dir = tap_dir.join(".git");
	let last_refresh = ["FETCH_HEAD", REFRESH_STAMP]
		.iter()
		.filter_map(|name| fs::metadata(git_dir.join(name)).and_then(|meta| meta.modified()).ok())
		.max();
	match last_refresh {
		// A stamp from the future (clock skew on a network mount) cannot prove freshness.
		Some(refreshed_at) => SystemTime::now()
			.duration_since(refreshed_at)
			.map_or(true, |age| age >= TAP_REFRESH_INTERVAL),
		None => true,
	}
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

	// Same `user/repo` rule as `tap init`, so the two commands accept the same names.
	super::tap_scaffold::split_tap_id(&name)?;

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
/// Also starts a background refresh of due GitHub taps (Homebrew-style); the
/// files it brings in are visible to later calls, never to this one.
pub fn load_taps() -> Result<Vec<Tap>> {
	let mut file = read_taps_file()?;

	// Ensure default tap is cloned (seamless first-time setup)
	ensure_default_tap()?;

	// Refresh GitHub taps only (local taps are symlinks — always live)
	for tap in &file.taps {
		if tap.local_path.is_none() {
			if let Ok(tap_dir) = tap.local_dir() {
				if tap_dir.exists() {
					spawn_refresh(&tap_dir);
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

/// Ensure the default tap is cloned (seamless first-time setup) and refreshed
/// in the background. Only the first clone blocks: nothing exists to read without it.
pub fn ensure_default_tap() -> Result<()> {
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
		spawn_refresh(&tap_dir);
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

/// Fetch a public workflow definition by name from taps.
///
/// Reads `<tap>/workflows/<name>.toml`, first tap wins (user taps first,
/// built-in default last). Starts a background refresh of GitHub taps
/// (Homebrew-style) before reading, so workflows fetched later stay fresh —
/// mirrors agent manifest fetch.
///
/// Returns `(raw_toml, source_tap_name)`.
pub fn fetch_workflow(name: &str) -> Result<(String, String)> {
	let taps = load_taps().context("Failed to load taps")?;
	let mut errors: Vec<String> = Vec::new();
	for tap in &taps {
		let path = match tap.workflows_dir() {
			Ok(d) => d.join(format!("{name}.toml")),
			Err(_) => continue,
		};
		match fs::read_to_string(&path) {
			Ok(content) => return Ok((content, tap.name.clone())),
			Err(e) => errors.push(format!("  - {}: {}", tap.name, e)),
		}
	}
	let detail = if errors.is_empty() {
		"No taps configured".to_string()
	} else {
		errors.join("\n")
	};
	anyhow::bail!(
		"Workflow '{}' not found in any tap (looked for workflows/{}.toml):\n{}",
		name,
		name,
		detail
	)
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
			fs::create_dir_all(parent).context(format!("Failed to create tap parent dir: {}", parent.display()))?;
		}
		// Remove stale symlink/dir if it already exists at the target path
		if tap_dir.exists() || tap_dir.symlink_metadata().is_ok() {
			remove_local_tap_link(&tap_dir)
				.context(format!("Failed to remove existing tap path: {}", tap_dir.display()))?;
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
		anyhow::bail!("'{}' is the built-in default tap and cannot be removed", name);
	}

	let mut file = read_taps_file()?;
	let before = file.taps.len();
	let removed: Vec<Tap> = file.taps.iter().filter(|t| t.name == name).cloned().collect();
	file.taps.retain(|t| t.name != name);
	if file.taps.len() == before {
		anyhow::bail!("Tap '{}' is not in your tap list", name);
	}

	// Remove the symlink for local taps (GitHub clones are left on disk intentionally)
	for tap in &removed {
		if tap.local_path.is_some() {
			if let Ok(tap_dir) = tap.local_dir() {
				if tap_dir.symlink_metadata().is_ok() {
					let _ = remove_local_tap_link(&tap_dir);
				}
			}
		}
	}

	write_taps_file(&file)?;
	Ok(())
}

/// Remove the directory symlink used for a local tap without touching its
/// target. Windows requires `remove_dir` for directory symlinks; Unix treats
/// the same link as a file-system entry removed with `remove_file`.
fn remove_local_tap_link(path: &std::path::Path) -> std::io::Result<()> {
	#[cfg(windows)]
	{
		fs::remove_dir(path)
	}
	#[cfg(not(windows))]
	{
		fs::remove_file(path)
	}
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
	crate::agent::registry::invalidate_capability_catalog();
	Ok(())
}

/// Pull latest changes for a tap. Output is suppressed; only shown in debug mode.
fn git_pull(dir: &PathBuf) -> Result<()> {
	let output = std::process::Command::new("git")
		.args(["pull"])
		.current_dir(dir)
		// Automatic tap refresh is best-effort and silent; it must never inherit
		// stdin and stop the entire session on a credential prompt.
		.env("GIT_TERMINAL_PROMPT", "0")
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.output()
		.context("Failed to run git pull")?;

	crate::agent::registry::invalidate_capability_catalog();
	if !output.status.success() {
		crate::log_debug!(
			"Failed to update tap at {}: {}",
			dir.display(),
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	Ok(())
}

/// Start a detached `git pull --ff-only` for a due tap and return at once.
///
/// The pull is never awaited on the caller's path: on a slow network mount it
/// takes minutes, and a delegated specialist must answer its parent's ACP
/// handshake first. It runs in its own process group so that the parent
/// killing a timed-out child's group cannot stop git mid-merge and leave a
/// stale `.git/*.lock` that fails every later pull. `--ff-only` never creates
/// a merge commit or leaves a half-merge in a clone shared between processes.
fn spawn_refresh(tap_dir: &Path) {
	if !refresh_due(tap_dir) {
		return;
	}
	remove_stale_locks(&tap_dir.join(".git"));
	// Stamp before spawning so concurrent processes see the refresh as taken.
	// Without a `.git` dir this is not a clone and there is nothing to pull.
	let stamp = tap_dir.join(".git").join(REFRESH_STAMP);
	if fs::File::create(&stamp)
		.and_then(|file| file.set_modified(SystemTime::now()))
		.is_err()
	{
		return;
	}

	let mut command = std::process::Command::new("git");
	command
		.args(["pull", "--ff-only"])
		.current_dir(tap_dir)
		// Best-effort and silent; it must never wait on a credential prompt.
		.env("GIT_TERMINAL_PROMPT", "0")
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null());
	#[cfg(unix)]
	{
		use std::os::unix::process::CommandExt;
		command.process_group(0);
	}
	#[cfg(windows)]
	{
		use std::os::windows::process::CommandExt;
		const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
		command.creation_flags(CREATE_NEW_PROCESS_GROUP);
	}

	let mut child = match command.spawn() {
		Ok(child) => child,
		Err(err) => {
			crate::log_debug!("Failed to start tap refresh at {}: {}", tap_dir.display(), err);
			return;
		}
	};
	let tap_dir = tap_dir.to_path_buf();
	// Reaps the child (no zombie in a long-lived server) and lets that server
	// pick up the new files once the pull lands.
	std::thread::spawn(move || {
		match child.wait() {
			Ok(status) if !status.success() => {
				crate::log_debug!("Failed to update tap at {}: {}", tap_dir.display(), status);
			}
			Ok(_) => {}
			Err(err) => {
				crate::log_debug!("Failed to wait for tap refresh at {}: {}", tap_dir.display(), err);
			}
		}
		crate::agent::registry::invalidate_capability_catalog();
	});
}

/// Remove `.git/*.lock` files older than [`STALE_LOCK_AGE`]. A container stop
/// or idle suspend kills git together with everything else, and the lock it
/// leaves makes every later pull fail — one such lock froze a tap for a week.
fn remove_stale_locks(git_dir: &Path) {
	let Ok(entries) = fs::read_dir(git_dir) else {
		return;
	};
	for entry in entries.flatten() {
		let path = entry.path();
		let stale = path.extension().is_some_and(|ext| ext == "lock")
			&& entry
				.metadata()
				.and_then(|meta| meta.modified())
				.ok()
				.and_then(|modified| SystemTime::now().duration_since(modified).ok())
				.is_some_and(|age| age >= STALE_LOCK_AGE);
		if stale {
			if let Err(err) = fs::remove_file(&path) {
				crate::log_debug!("Failed to remove stale tap lock {}: {}", path.display(), err);
			}
		}
	}
}

#[cfg(test)]
#[path = "taps_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "taps_additional_tests.rs"]
mod additional_tests;
