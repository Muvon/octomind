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
//! - Manifests expected at: `<tap>/agents/<ns>/default.toml` (the agent `<ns>`) and
//!   `<tap>/agents/<ns>/<variant>.toml` (the agent `<ns>:<variant>`)
//! - Skills: `<tap>/agents/<ns>/skills/<skill>/` (owned by that namespace) and `<tap>/skills/<skill>/` (shared)
//!
//! ## Priority
//! User taps (in order added) → built-in tap (`[registry] tap` in the config, `muvon/tap` by default)
//!
//! The built-in tap serves the branch, tag or commit named by `[registry] ref`; taps added by
//! the user follow their default branch.

use crate::agent::registry::{
	canonical_tag, manifest_parts, namespace_of, parse_agent_header, AgentHeader, DEFAULT_VARIANT,
};
use crate::config::registry::RegistryConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, PoisonError, RwLock};
use std::time::{Duration, SystemTime};

/// The built-in tap and the ref of it to serve, as the config names them. Process-wide because
/// the tap lookups below take no config and run on any thread; `Config` loading sets it
/// ([`set_built_in_tap`]). The config template is the only source of the values, so a lookup
/// before any config was loaded is an error, not a guess.
static BUILT_IN_TAP: RwLock<Option<BuiltInTap>> = RwLock::new(None);

#[derive(Clone)]
struct BuiltInTap {
	name: String,
	git_ref: String,
}

/// Install the built-in tap from `[registry]`. Called whenever a config is loaded.
pub fn set_built_in_tap(registry: &RegistryConfig) {
	*BUILT_IN_TAP.write().unwrap_or_else(PoisonError::into_inner) = Some(BuiltInTap {
		name: registry.tap.clone(),
		git_ref: registry.git_ref.clone(),
	});
}

fn configured_built_in_tap() -> Result<BuiltInTap> {
	if let Some(built_in) = BUILT_IN_TAP.read().unwrap_or_else(PoisonError::into_inner).clone() {
		return Ok(built_in);
	}
	not_loaded()
}

/// Unit tests run without a loaded config and use the values the template ships.
#[cfg(test)]
fn not_loaded() -> Result<BuiltInTap> {
	let registry = RegistryConfig::default();
	Ok(BuiltInTap {
		name: registry.tap,
		git_ref: registry.git_ref,
	})
}

#[cfg(not(test))]
fn not_loaded() -> Result<BuiltInTap> {
	anyhow::bail!("The built-in tap is not configured: no configuration was loaded")
}

/// The built-in tap — always present as the last fallback.
pub fn built_in_tap() -> Result<Tap> {
	Ok(Tap {
		name: configured_built_in_tap()?.name,
		local_path: None,
	})
}

/// The ref of the built-in tap that is served.
pub fn built_in_tap_ref() -> Result<String> {
	Ok(configured_built_in_tap()?.git_ref)
}

/// How often a GitHub tap is refreshed automatically. Every agent start reads the
/// taps, and a delegated agent has seconds to answer its parent's
/// handshake, so a refresh starts at most once per interval for every process
/// sharing the data dir: the newer of git's `FETCH_HEAD` and `REFRESH_STAMP`
/// marks the last one.
const TAP_REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// File under `.git` written when a background refresh is started, holding the
/// octomind version that started it and, for the built-in tap, the ref it serves
/// (see [`refresh_stamp`]). `FETCH_HEAD` only changes when the fetch
/// finishes, which on a slow mount is long after other processes have seen the
/// same due refresh and started their own.
const REFRESH_STAMP: &str = "octomind-refresh";

/// Age past which a `.git/*.lock` has no live owner. A refresh that took 4.5
/// minutes on a degraded network mount is the slowest seen; an hour is far past it.
const STALE_LOCK_AGE: Duration = Duration::from_secs(60 * 60);

/// Longest a lookup that found nothing waits for the refreshes this process started.
/// A pull takes a second or two on a local disk; past this limit the lookup reports
/// its miss instead of hanging on a stalled network.
const REFRESH_WAIT_LIMIT: Duration = Duration::from_secs(30);

/// Refreshes this process started: how many are still running, and whether any has ended.
/// Every waiter reads the same state, so one waiter never hides a refresh from another.
static REFRESH_STATE: Mutex<RefreshState> = Mutex::new(RefreshState {
	in_flight: 0,
	any_ended: false,
});
static REFRESH_ENDED: Condvar = Condvar::new();

struct RefreshState {
	in_flight: usize,
	any_ended: bool,
}

/// A tap entry: either a GitHub repo or a local path.
const AGENTS_DIR: &str = "agents";
const SKILLS_DIR: &str = "skills";

/// A skill directory of a tap and the agent it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapSkillDir {
	/// The namespace that owns the skill by location (`agents/<ns>/skills/`): its base agent
	/// and every variant. `None` for a shared skill (`skills/`), bound through its `domains:`.
	pub owner: Option<String>,
	pub dir: PathBuf,
}

/// The subdirectories of `dir` by name; none when `dir` is missing or unreadable.
fn sorted_subdirs(dir: &Path) -> Vec<PathBuf> {
	let Ok(entries) = fs::read_dir(dir) else {
		return Vec::new();
	};
	let mut dirs: Vec<PathBuf> = entries
		.flatten()
		.map(|entry| entry.path())
		.filter(|path| path.is_dir())
		.collect();
	dirs.sort();
	dirs
}

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
		Ok(self.local_dir()?.join(AGENTS_DIR))
	}

	/// Where the manifest of the agent `tag` lives: `agents/<ns>/default.toml` for `<ns>`,
	/// `agents/<ns>/<variant>.toml` for `<ns>:<variant>`.
	pub fn agent_manifest(&self, tag: &str) -> Result<PathBuf> {
		let (namespace, variant) = manifest_parts(canonical_tag(tag));
		Ok(self.agents_dir()?.join(namespace).join(format!("{variant}.toml")))
	}

	/// The old flat manifest file, `<ns>.toml` directly in `agents/`. It is no longer read; it
	/// is only looked up to tell a user whose tap still has it where the file must move to.
	fn flat_agent_manifest(&self, namespace: &str) -> Result<PathBuf> {
		Ok(self.agents_dir()?.join(namespace).with_extension("toml"))
	}

	/// The agents of this tap as `(tag, manifest path)`, sorted by tag. Each directory of
	/// `agents/` is a namespace: its `default.toml` is the agent `<ns>` and every other
	/// `*.toml` is the agent `<ns>:<variant>`. A namespace may have variants and no
	/// `default.toml`. A symlink is not an agent.
	pub fn agent_manifests(&self) -> Result<Vec<(String, PathBuf)>> {
		let agents_dir = self.agents_dir()?;
		if !agents_dir.is_dir() {
			return Ok(Vec::new());
		}
		let mut manifests = Vec::new();
		for namespace_entry in fs::read_dir(&agents_dir)?.flatten() {
			// `DirEntry::file_type` does not follow links, so a tap directory that is itself
			// a symlink (a local tap) still lists its real agents.
			if !namespace_entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
				continue;
			}
			let Ok(namespace) = namespace_entry.file_name().into_string() else {
				continue;
			};
			let Ok(files) = fs::read_dir(namespace_entry.path()) else {
				continue;
			};
			for file in files.flatten() {
				let path = file.path();
				if !file.file_type().is_ok_and(|file_type| file_type.is_file())
					|| path.extension().and_then(|extension| extension.to_str()) != Some("toml")
				{
					continue;
				}
				let Some(variant) = path.file_stem().and_then(|stem| stem.to_str()) else {
					continue;
				};
				let tag = variant_tag(&namespace, variant);
				manifests.push((tag, path));
			}
		}
		manifests.sort_by(|a, b| a.0.cmp(&b.0));
		Ok(manifests)
	}

	/// Every skill directory of this tap with the namespace that owns it: `Some(ns)` for
	/// `agents/<ns>/skills/<skill>/`, `None` for the shared `skills/<skill>/`. Order is
	/// fixed: shared skills first, then namespaces by name, each namespace's skills by name.
	pub fn skill_dirs(&self) -> Result<Vec<TapSkillDir>> {
		let root = self.local_dir()?;
		let mut skills: Vec<TapSkillDir> = sorted_subdirs(&root.join(SKILLS_DIR))
			.into_iter()
			.map(|dir| TapSkillDir { owner: None, dir })
			.collect();
		let mut namespaces: Vec<String> = self
			.agent_manifests()?
			.iter()
			.map(|(tag, _)| namespace_of(tag).to_string())
			.collect();
		namespaces.dedup();
		for namespace in namespaces {
			let owned = sorted_subdirs(&self.agents_dir()?.join(&namespace).join(SKILLS_DIR));
			skills.extend(owned.into_iter().map(|dir| TapSkillDir {
				owner: Some(namespace.clone()),
				dir,
			}));
		}
		Ok(skills)
	}

	/// Returns the deps directory path for this tap.
	pub fn deps_dir(&self) -> Result<PathBuf> {
		Ok(self.local_dir()?.join("deps"))
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

/// What a refresh writes into [`REFRESH_STAMP`]: the octomind version, and for the built-in
/// tap (`git_ref` is `Some`) the ref it is served at. A changed ref makes the next refresh due
/// at once, since the clone still holds the old one. A changed built-in tap name needs no
/// part here: it has a clone directory of its own, which has no stamp yet.
fn refresh_stamp(git_ref: Option<&str>) -> String {
	match git_ref {
		Some(git_ref) => format!("{} {git_ref}", env!("CARGO_PKG_VERSION")),
		None => env!("CARGO_PKG_VERSION").to_string(),
	}
}

/// Whether a GitHub tap is due for its automatic refresh. A clone that has
/// never fetched nor started a refresh is due at once, and so is one whose
/// last refresh wrote another `stamp`: a new octomind version may need agents
/// published since (the flat layout on the first run after an upgrade), and a
/// new `[registry] ref` names other files.
fn refresh_due(tap_dir: &Path, stamp: &str) -> bool {
	let git_dir = tap_dir.join(".git");
	let stamped = fs::read_to_string(git_dir.join(REFRESH_STAMP)).unwrap_or_default();
	if stamped != stamp {
		return true;
	}
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
	file.taps.push(built_in_tap()?);
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
					spawn_refresh(&tap_dir, &tap.name, None);
				}
			}
		}
	}

	// Built-in tap is always last
	file.taps.push(built_in_tap()?);
	Ok(file.taps)
}

/// Ensure the built-in tap is cloned (seamless first-time setup) and refreshed
/// in the background. Only the first clone blocks: nothing exists to read without it.
/// The clone ends on the configured `[registry] ref`.
pub fn ensure_default_tap() -> Result<()> {
	let default_tap = built_in_tap()?;
	let git_ref = built_in_tap_ref()?;
	let tap_dir = default_tap.local_dir()?;
	if !tap_dir.exists() {
		let url = default_tap.github_url();
		crate::log_info!("Cloning built-in tap {} at {}...", default_tap.name, git_ref);
		git_clone(&url, &tap_dir, Some(&git_ref))?;
	} else {
		spawn_refresh(&tap_dir, &default_tap.name, Some(&git_ref));
	}
	Ok(())
}

/// Wait for the tap refreshes this process started. `true` once any of them has ended, so a
/// lookup that found nothing is worth one more try. Gives up after [`REFRESH_WAIT_LIMIT`].
///
/// A refresh runs in the background and a lookup reads the clone as it is. An agent published
/// after the last refresh (a renamed one on the first run after an upgrade) is on disk only
/// once the pull has ended. Only a lookup that found nothing pays this wait. A refresh that
/// ended before this call still counts: the lookup may have read the clone just before the
/// pull landed. Concurrent callers all see the same refreshes, so parallel lookups all retry.
pub fn wait_for_started_refreshes() -> bool {
	let state = REFRESH_STATE.lock().unwrap_or_else(PoisonError::into_inner);
	let (state, _timed_out) = REFRESH_ENDED
		.wait_timeout_while(state, REFRESH_WAIT_LIMIT, |state| state.in_flight > 0)
		.unwrap_or_else(PoisonError::into_inner);
	state.any_ended
}

/// Returns only user-added taps (excludes the built-in default).
pub fn list_taps() -> Result<Vec<Tap>> {
	Ok(read_taps_file()?.taps)
}

/// A manifest file found in a tap, not read yet.
struct AgentManifestFile {
	tag: String,
	path: PathBuf,
}

/// Walk every active tap's agent manifests (see [`Tap::agent_manifests`]). A symlink is not an
/// agent, and neither is the old flat `<ns>.toml`.
///
/// Uses only locally cached tap data — no network calls. First tap wins on duplicates
/// (same priority as `fetch_manifest`). Result is sorted by tag.
fn walk_agent_manifests() -> Result<Vec<AgentManifestFile>> {
	let taps = get_taps()?;
	let mut manifests: Vec<AgentManifestFile> = Vec::new();
	let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

	for tap in &taps {
		for (tag, path) in tap.agent_manifests().unwrap_or_default() {
			if seen.insert(tag.clone()) {
				manifests.push(AgentManifestFile { tag, path });
			}
		}
	}

	manifests.sort_by(|a, b| a.tag.cmp(&b.tag));
	Ok(manifests)
}

/// Explain why `tag` is not an agent, for a lookup that found none: a tap that still has the
/// flat `<ns>.toml` file, or a namespace that has other agents but not this one. Returns `Ok`
/// when nothing in the taps explains it, so the caller reports the plain not-found.
pub fn explain_missing_agent(tag: &str) -> Result<()> {
	let tag = canonical_tag(tag);
	let namespace = namespace_of(tag);
	let taps = get_taps()?;
	for tap in &taps {
		let flat = tap.flat_agent_manifest(namespace)?;
		if flat.is_file() {
			anyhow::bail!(
				"Tap '{}' has the agent '{namespace}' as {}, which is no longer read. Move it to {}",
				tap.name,
				flat.display(),
				tap.agent_manifest(namespace)?.display()
			);
		}
	}
	let mut known: Vec<String> = Vec::new();
	for tap in &taps {
		for (existing, _) in tap.agent_manifests().unwrap_or_default() {
			if namespace_of(&existing) == namespace && !known.contains(&existing) {
				known.push(existing);
			}
		}
	}
	if !known.is_empty() {
		known.sort();
		anyhow::bail!(
			"There is no agent '{tag}': namespace '{namespace}' has the agents {}",
			known.join(", ")
		);
	}
	Ok(())
}

/// Returns all available agent tags (the agent names) from all active taps.
///
/// Uses only locally cached tap data — no network calls. First tap wins on duplicates
/// (same priority as `fetch_manifest`). Result is sorted alphabetically.
pub fn list_agent_tags() -> Result<Vec<String>> {
	Ok(walk_agent_manifests()?.into_iter().map(|m| m.tag).collect())
}

/// An agent found in a tap with the fields read from its header comments.
#[derive(Debug, Clone)]
pub struct TapAgentHeader {
	/// The agent's name, which is also its tag.
	pub tag: String,
	pub header: AgentHeader,
}

/// Returns every agent from all active taps with its header fields.
///
/// Same rules as [`list_agent_tags`]: local data only, first tap wins on a
/// duplicate tag, sorted by tag. An unreadable manifest is an error.
pub fn list_agent_headers() -> Result<Vec<TapAgentHeader>> {
	walk_agent_manifests()?
		.into_iter()
		.map(|manifest| {
			let raw = fs::read_to_string(&manifest.path)
				.with_context(|| format!("Failed to read agent manifest {}", manifest.path.display()))?;
			Ok(TapAgentHeader {
				tag: manifest.tag,
				header: parse_agent_header(&raw),
			})
		})
		.collect()
}

/// Whether an active tap has the agent `name`. Local tap data only, no network.
pub fn has_agent(name: &str) -> Result<bool> {
	Ok(walk_agent_manifests()?.iter().any(|manifest| manifest.tag == name))
}

/// The tag of `variant` in `namespace`: the namespace itself for its base agent.
fn variant_tag(namespace: &str, variant: &str) -> String {
	if variant == DEFAULT_VARIANT {
		namespace.to_string()
	} else {
		format!("{namespace}:{variant}")
	}
}

/// Variant an alias manifest points to: `default` for `chief.toml -> default.toml`.
///
/// `None` for a real file, a missing one, or a link that leaves its namespace directory.
fn alias_target_variant(manifest: &Path) -> Option<String> {
	let target = fs::read_link(manifest).ok()?;
	if target.parent() != Some(Path::new("")) || target.extension()? != "toml" {
		return None;
	}
	Some(target.file_stem()?.to_str()?.to_string())
}

/// Map an alias tag to the tag of the agent it points to; any other tag comes back unchanged.
///
/// A tap renames an agent by leaving a symlink under the old name: `chief.toml -> default.toml`
/// keeps `<ns>:chief` working as `<ns>`. Agent listings skip the link, so the old name is never
/// offered. The first tap that has the name decides, as in `fetch_manifest`: a real manifest
/// there is an agent of its own and is never redirected. Local tap data only, no network. Logs
/// one notice per mapping.
pub fn resolve_alias_tag(tag: &str) -> Result<String> {
	let tag = canonical_tag(tag);
	let (namespace, _) = manifest_parts(tag);
	for tap in get_taps()? {
		let manifest = tap.agent_manifest(tag)?;
		if manifest.symlink_metadata().is_err() {
			continue;
		}
		let Some(target_variant) = alias_target_variant(&manifest) else {
			return Ok(tag.to_string());
		};
		let target_tag = variant_tag(namespace, &target_variant);
		crate::log_info!("`{}` is an alias of `{}`; using `{}`", tag, target_tag, target_tag);
		return Ok(target_tag);
	}
	Ok(tag.to_string())
}

/// Tags that are aliases of `tag`: the symlinks in its namespace that point to its manifest.
/// Sorted, each once. Local tap data only, no network.
pub fn alias_tags_of(tag: &str) -> Result<Vec<String>> {
	let (namespace, variant) = manifest_parts(canonical_tag(tag));
	let mut aliases: Vec<String> = Vec::new();
	for tap in get_taps()? {
		let Ok(entries) = fs::read_dir(tap.agents_dir()?.join(namespace)) else {
			continue;
		};
		for entry in entries.flatten() {
			let path = entry.path();
			if alias_target_variant(&path).as_deref() != Some(variant) {
				continue;
			}
			if let Some(alias) = path.file_stem().and_then(|stem| stem.to_str()) {
				aliases.push(variant_tag(namespace, alias));
			}
		}
	}
	aliases.sort();
	aliases.dedup();
	Ok(aliases)
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

	if tap.name == built_in_tap()?.name {
		anyhow::bail!(
			"'{}' is the built-in tap — it's always active and cannot be re-added",
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
			git_clone(&url, &tap_dir, None)?;
		} else {
			crate::log_info!("Tap {} already cloned, updating...", tap.name);
			if let Err(err) = sync_clone(&tap_dir, None) {
				crate::log_error!("Failed to update tap {}: {:#}", tap.name, err);
			}
		}
	}

	file.taps.push(tap);
	write_taps_file(&file)?;
	Ok(())
}

/// Remove a tap by name. Also removes the symlink for local taps.
pub fn remove_tap(name: &str) -> Result<()> {
	let name = name.trim().to_string();

	if name == built_in_tap()?.name {
		anyhow::bail!("'{}' is the built-in tap and cannot be removed", name);
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
///
/// With a `git_ref` (the built-in tap) the clone ends on that ref, not on the default branch.
/// A clone that cannot reach it is removed: left in place it would be served on the wrong files.
fn git_clone(url: &str, dir: &std::path::Path, git_ref: Option<&str>) -> Result<()> {
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
	if let Some(git_ref) = git_ref {
		if let Err(err) = sync_clone(dir, Some(git_ref)) {
			let _ = fs::remove_dir_all(dir);
			return Err(err.context(format!("Failed to check out '{git_ref}' of {url}")));
		}
	}
	crate::agent::registry::invalidate_capability_catalog();
	Ok(())
}

/// Make a GitHub tap's clone match its upstream exactly: `git_ref` when given (the built-in
/// tap), the default branch otherwise.
///
/// The clone is a mirror, not a workspace (Homebrew-style): local edits, local
/// commits and new files are discarded, so nothing written into it can block a
/// later refresh. Customise a tap through a local tap or upstream instead. A
/// symlinked tap dir points at someone's working copy and is never touched.
/// Each step is its own git process; a refresh that stops between them leaves
/// a usable clone, and the next refresh completes it.
fn sync_clone(dir: &Path, git_ref: Option<&str>) -> Result<()> {
	if dir.symlink_metadata()?.file_type().is_symlink() {
		return Ok(());
	}
	// Without its own `.git`, git would find an enclosing repository (a dotfiles
	// repo over the data dir) and the reset and clean below would wipe it.
	if !dir.join(".git").is_dir() {
		anyhow::bail!("{} is not a git clone", dir.display());
	}
	run_git(dir, &["fetch", "--depth", "1", "origin", git_ref.unwrap_or("HEAD")])?;
	run_git(dir, &["reset", "--hard", "FETCH_HEAD"])?;
	run_git(dir, &["clean", "-fd"])
}

/// Run one git command in `dir`, returning its stderr as the error on failure.
///
/// It runs in its own process group so that the parent killing a timed-out
/// child's group cannot stop git mid-write and leave a stale `.git/*.lock`
/// that fails every later refresh. It never waits on a credential prompt.
fn run_git(dir: &Path, args: &[&str]) -> Result<()> {
	let mut command = std::process::Command::new("git");
	command
		.args(args)
		.current_dir(dir)
		.env("GIT_TERMINAL_PROMPT", "0")
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::piped());
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

	let output = command
		.output()
		.with_context(|| format!("Failed to run git {}", args[0]))?;
	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
		anyhow::bail!(
			"git {} failed: {}",
			args[0],
			if stderr.is_empty() {
				output.status.to_string()
			} else {
				stderr
			}
		);
	}
	Ok(())
}

/// Start a background [`sync_clone`] for a due tap and return at once.
///
/// The sync is never awaited on the caller's path: on a slow network mount it
/// takes minutes, and a delegated agent must answer its parent's ACP
/// handshake first. Only a lookup that found nothing waits for it, through
/// [`wait_for_started_refreshes`].
///
/// `git_ref` is the ref the built-in tap serves; a user tap passes `None` and follows `HEAD`.
/// The stamp claims the refresh for other processes while it runs; a failed sync takes the
/// claim back, so the next start tries again instead of waiting out the interval.
fn spawn_refresh(tap_dir: &Path, tap_name: &str, git_ref: Option<&str>) {
	let stamp_content = refresh_stamp(git_ref);
	if !refresh_due(tap_dir, &stamp_content) {
		return;
	}
	remove_stale_locks(&tap_dir.join(".git"));
	// Stamp before starting so concurrent processes see the refresh as taken.
	// Without a `.git` dir this is not a clone and there is nothing to sync.
	let stamp = tap_dir.join(".git").join(REFRESH_STAMP);
	if fs::File::create(&stamp)
		.and_then(|mut file| {
			file.write_all(stamp_content.as_bytes())?;
			file.set_modified(SystemTime::now())
		})
		.is_err()
	{
		return;
	}

	let tap_dir = tap_dir.to_path_buf();
	let git_ref = git_ref.map(str::to_string);
	let tap_name = tap_name.to_string();
	// Counted before the thread starts: a lookup that misses right after this call
	// must already see the refresh as running.
	let refresh = RefreshEndGuard::start();
	// Lets a long-lived server pick up the new files once the sync lands. The guard
	// drops last, after the catalog is invalidated, on every exit path, panic included.
	std::thread::spawn(move || {
		let _refresh = refresh;
		sync_or_release_stamp(&tap_dir, &tap_name, git_ref.as_deref());
		crate::agent::registry::invalidate_capability_catalog();
	});
}

/// Set once the first failed refresh has been shown to the user.
static REFRESH_FAILURE_SHOWN: AtomicBool = AtomicBool::new(false);

/// Sync a clone; on failure remove the stamp (so the refresh is due again at the next start)
/// and tell the user once per process that the previous checkout is still in use.
fn sync_or_release_stamp(tap_dir: &Path, tap_name: &str, git_ref: Option<&str>) {
	let Err(err) = sync_clone(tap_dir, git_ref) else {
		return;
	};
	if let Err(remove_err) = fs::remove_file(tap_dir.join(".git").join(REFRESH_STAMP)) {
		crate::log_debug!(
			"Failed to release the refresh stamp of tap {}: {}",
			tap_name,
			remove_err
		);
	}
	if REFRESH_FAILURE_SHOWN.swap(true, Ordering::SeqCst) {
		return;
	}
	let target = git_ref.map_or_else(|| "its default branch".to_string(), |git_ref| format!("'{git_ref}'"));
	eprintln!("Could not update tap {tap_name} to {target}: {err:#}. The previous checkout is still in use.");
}

/// Counts one refresh as running from start to drop, so a refresh thread that
/// panics still releases its waiters.
struct RefreshEndGuard;

impl RefreshEndGuard {
	fn start() -> Self {
		REFRESH_STATE.lock().unwrap_or_else(PoisonError::into_inner).in_flight += 1;
		Self
	}
}

impl Drop for RefreshEndGuard {
	fn drop(&mut self) {
		let mut state = REFRESH_STATE.lock().unwrap_or_else(PoisonError::into_inner);
		state.in_flight -= 1;
		state.any_ended = true;
		drop(state);
		REFRESH_ENDED.notify_all();
	}
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
