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

//! `octomind tap init` — bootstrap a new user tap from the default tap's scaffold.
//!
//! The canonical template lives in the default tap under `scaffolds/tap/`:
//! `scaffold.toml` is the render contract and everything below its `root/`
//! directory is copied into the destination after `__TOKEN__` substitution in
//! both file paths and file contents. Rendering fails if any token remains
//! unresolved. After rendering, the scaffold's own validation command runs,
//! the directory is git-initialized, and the tap is registered as a local tap
//! so `octomind run <domain>:<spec>` works immediately.

use anyhow::{bail, Context, Result};
use regex::Regex;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::taps::{self, Tap, DEFAULT_TAP};

const SCAFFOLD_SUBDIR: &str = "scaffolds/tap";
const SUPPORTED_SCHEMA: u32 = 1;
/// Same pattern `scripts/check.py` in the scaffold enforces post-render.
const TOKEN_PATTERN: &str = "__[A-Z][A-Z0-9_]*__";

#[derive(Debug, serde::Deserialize)]
struct ScaffoldManifest {
	schema: u32,
	root: String,
	#[serde(default)]
	required_tokens: Vec<String>,
	#[serde(default)]
	defaults: ScaffoldDefaults,
	#[serde(default)]
	post_create: PostCreate,
}

#[derive(Debug, Default, serde::Deserialize)]
struct ScaffoldDefaults {
	agent_spec: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PostCreate {
	#[serde(default)]
	executable: Vec<String>,
	validate: Option<String>,
}

/// Result of a successful `tap init`, consumed by the command layer for display.
pub struct InitOutcome {
	pub tap_id: String,
	pub dest: PathBuf,
	pub agent_tag: String,
}

/// Create a new tap repository from the default tap's scaffold.
///
/// `agent` overrides the starter agent tag as `domain:spec` (domain defaults
/// to the repo name, spec to the scaffold's default). `dir` overrides the
/// destination (defaults to `./octomind-<repo>` in the current directory).
pub fn init_tap(tap_id: &str, agent: Option<&str>, dir: Option<&Path>) -> Result<InitOutcome> {
	let (owner, name) = split_tap_id(tap_id)?;

	if tap_id == DEFAULT_TAP {
		bail!(
			"'{}' is the built-in default tap — pick your own owner/name",
			tap_id
		);
	}
	if taps::list_taps()?.iter().any(|t| t.name == tap_id) {
		bail!(
			"Tap '{}' is already registered — remove it first with `octomind untap {}`",
			tap_id,
			tap_id
		);
	}

	taps::ensure_default_tap()?;
	let default_tap = Tap {
		name: DEFAULT_TAP.to_string(),
		local_path: None,
	};
	let scaffold_dir = default_tap.local_dir()?.join(SCAFFOLD_SUBDIR);
	let manifest_path = scaffold_dir.join("scaffold.toml");
	if !manifest_path.is_file() {
		bail!(
			"Default tap has no scaffold at {} — update it with `git -C {} pull`",
			manifest_path.display(),
			default_tap.local_dir()?.display()
		);
	}

	let manifest: ScaffoldManifest = toml::from_str(
		&fs::read_to_string(&manifest_path)
			.context(format!("Failed to read {}", manifest_path.display()))?,
	)
	.context("Failed to parse scaffold.toml")?;
	if manifest.schema != SUPPORTED_SCHEMA {
		bail!(
			"Scaffold schema {} is not supported by this octomind build (expected {}) — update octomind",
			manifest.schema,
			SUPPORTED_SCHEMA
		);
	}
	let scaffold_root = scaffold_dir.join(&manifest.root);
	if !scaffold_root.is_dir() {
		bail!(
			"Scaffold root directory missing: {}",
			scaffold_root.display()
		);
	}

	let tokens = build_tokens(owner, name, agent, manifest.defaults.agent_spec.as_deref())?;
	let missing: Vec<&str> = manifest
		.required_tokens
		.iter()
		.filter(|t| !tokens.contains_key(t.as_str()))
		.map(|t| t.as_str())
		.collect();
	if !missing.is_empty() {
		bail!(
			"Scaffold requires tokens this octomind build does not provide: {} — update octomind",
			missing.join(", ")
		);
	}

	let dest = match dir {
		Some(d) => d.to_path_buf(),
		None => std::env::current_dir()?.join(&tokens["__TAP_REPOSITORY__"]),
	};
	let created_dest = !dest.exists();
	ensure_empty_dest(&dest)?;

	let agent_tag = format!(
		"{}:{}",
		tokens["__AGENT_DOMAIN__"], tokens["__AGENT_SPEC__"]
	);
	let result = populate(&scaffold_root, &dest, &tokens, &manifest, tap_id);
	if let Err(e) = result {
		// Only remove what this run created; never touch a pre-existing directory.
		if created_dest {
			let _ = fs::remove_dir_all(&dest);
		}
		return Err(e);
	}

	Ok(InitOutcome {
		tap_id: tap_id.to_string(),
		dest,
		agent_tag,
	})
}

/// Render, validate, git-init, and register — everything that writes into `dest`.
fn populate(
	scaffold_root: &Path,
	dest: &Path,
	tokens: &BTreeMap<String, String>,
	manifest: &ScaffoldManifest,
	tap_id: &str,
) -> Result<()> {
	let leftover = Regex::new(TOKEN_PATTERN).expect("valid token regex");
	render_tree(scaffold_root, dest, tokens, &leftover)?;

	#[cfg(unix)]
	for rel in &manifest.post_create.executable {
		use std::os::unix::fs::PermissionsExt;
		let path = dest.join(rel);
		if !path.is_file() {
			bail!("post_create.executable entry missing after render: {}", rel);
		}
		let mut perms = fs::metadata(&path)?.permissions();
		perms.set_mode(perms.mode() | 0o111);
		fs::set_permissions(&path, perms)?;
	}

	if let Some(validate) = &manifest.post_create.validate {
		let output = Command::new("sh")
			.args(["-c", validate])
			.current_dir(dest)
			.output()
			.context(format!("Failed to run scaffold validation: {}", validate))?;
		if !output.status.success() {
			bail!(
				"Scaffold validation failed ({}):\n{}{}",
				validate,
				String::from_utf8_lossy(&output.stdout),
				String::from_utf8_lossy(&output.stderr)
			);
		}
	}

	let output = Command::new("git")
		.args(["init", "--quiet"])
		.current_dir(dest)
		.output()
		.context("Failed to run git init")?;
	if !output.status.success() {
		bail!(
			"git init failed in {}: {}",
			dest.display(),
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}

	let abs = fs::canonicalize(dest)?;
	taps::add_tap(&format!("{} {}", tap_id, abs.display()))?;
	Ok(())
}

/// Split `owner/name`, rejecting anything that is not exactly two non-empty parts.
fn split_tap_id(tap_id: &str) -> Result<(&str, &str)> {
	let mut parts = tap_id.split('/');
	match (parts.next(), parts.next(), parts.next()) {
		(Some(owner), Some(name), None) if !owner.is_empty() && !name.is_empty() => {
			Ok((owner, name))
		}
		_ => bail!("Tap name must be in 'user/repo' format, got: {}", tap_id),
	}
}

/// Split a `domain:spec` starter-agent override.
fn split_agent_tag(arg: &str) -> Result<(&str, &str)> {
	let mut parts = arg.split(':');
	match (parts.next(), parts.next(), parts.next()) {
		(Some(domain), Some(spec), None) if !domain.is_empty() && !spec.is_empty() => {
			Ok((domain, spec))
		}
		_ => bail!("Agent must be in 'domain:spec' format, got: {}", arg),
	}
}

fn build_tokens(
	owner: &str,
	name: &str,
	agent: Option<&str>,
	default_spec: Option<&str>,
) -> Result<BTreeMap<String, String>> {
	let (domain, spec) = match agent {
		Some(arg) => split_agent_tag(arg)?,
		None => {
			let spec = default_spec.context(
				"scaffold.toml defines no [defaults] agent_spec — pass --agent domain:spec",
			)?;
			(name, spec)
		}
	};

	let mut tokens = BTreeMap::new();
	tokens.insert("__TAP_ID__".to_string(), format!("{}/{}", owner, name));
	tokens.insert("__TAP_OWNER__".to_string(), owner.to_string());
	tokens.insert("__TAP_NAME__".to_string(), name.to_string());
	tokens.insert(
		"__TAP_REPOSITORY__".to_string(),
		format!("octomind-{}", name),
	);
	tokens.insert("__AGENT_DOMAIN__".to_string(), domain.to_string());
	tokens.insert("__AGENT_SPEC__".to_string(), spec.to_string());
	tokens.insert(
		"__YEAR__".to_string(),
		chrono::Utc::now().format("%Y").to_string(),
	);
	Ok(tokens)
}

/// Refuse anything but a missing path or an existing empty directory.
fn ensure_empty_dest(dest: &Path) -> Result<()> {
	if dest.exists() {
		if !dest.is_dir() {
			bail!("Destination is not a directory: {}", dest.display());
		}
		if fs::read_dir(dest)?.next().is_some() {
			bail!(
				"Destination is not empty: {} — refusing to overwrite",
				dest.display()
			);
		}
	} else {
		fs::create_dir_all(dest)
			.context(format!("Failed to create destination: {}", dest.display()))?;
	}
	Ok(())
}

fn render(input: &str, tokens: &BTreeMap<String, String>) -> String {
	let mut out = input.to_string();
	for (token, value) in tokens {
		out = out.replace(token.as_str(), value);
	}
	out
}

/// Recursively copy `src` into `dest`, rendering tokens in paths and UTF-8
/// contents. Symlinks are read through (`fs::metadata`/`fs::read` follow them),
/// so the scaffold may deduplicate files via relative symlinks — the generated
/// tap always gets real files. Binary files are copied verbatim.
fn render_tree(
	src: &Path,
	dest: &Path,
	tokens: &BTreeMap<String, String>,
	leftover: &Regex,
) -> Result<()> {
	for entry in fs::read_dir(src).context(format!("Failed to read {}", src.display()))? {
		let entry = entry?;
		let file_name = entry
			.file_name()
			.into_string()
			.map_err(|n| anyhow::anyhow!("Non-UTF8 file name in scaffold: {:?}", n))?;
		let rendered_name = render(&file_name, tokens);
		if let Some(m) = leftover.find(&rendered_name) {
			bail!(
				"Unresolved token {} in scaffold path: {}",
				m.as_str(),
				entry.path().display()
			);
		}
		let src_path = entry.path();
		let dest_path = dest.join(&rendered_name);
		let metadata =
			fs::metadata(&src_path).context(format!("Failed to stat {}", src_path.display()))?;

		if metadata.is_dir() {
			fs::create_dir_all(&dest_path)?;
			render_tree(&src_path, &dest_path, tokens, leftover)?;
			continue;
		}

		let bytes =
			fs::read(&src_path).context(format!("Failed to read {}", src_path.display()))?;
		match String::from_utf8(bytes) {
			Ok(text) => {
				let rendered = render(&text, tokens);
				if let Some(m) = leftover.find(&rendered) {
					bail!(
						"Unresolved token {} in scaffold file: {}",
						m.as_str(),
						src_path.display()
					);
				}
				fs::write(&dest_path, rendered)?;
			}
			Err(e) => {
				fs::write(&dest_path, e.into_bytes())?;
			}
		}
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			fs::set_permissions(
				&dest_path,
				fs::Permissions::from_mode(metadata.permissions().mode()),
			)?;
		}
	}
	Ok(())
}

#[cfg(test)]
#[path = "tap_scaffold_tests.rs"]
mod tests;
