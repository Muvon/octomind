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

//! Registry client: fetch and cache agent manifests.
//!
//! Tag format: `category:variant` or `category:variant@version`
//! Example:    `developer:rust`, `developer:rust@1.2`
//!
//! Manifests are cached at `~/.local/share/octomind/agents/<category>/<variant>.toml`.
//! If the cached file is older than `cache_ttl_hours`, it is refreshed in the background
//! while the cached copy is returned immediately.
//!
//! Sources are resolved from user taps (see `agent::taps`) — user taps first, built-in last.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::config::registry::RegistryConfig;

/// Parse a tag string into `(category, variant, version)`.
///
/// - `developer:rust`      → `("developer", "rust", None)`
/// - `developer:rust@1.2`  → `("developer", "rust", Some("1.2"))`
pub fn parse_tag(tag: &str) -> Result<(String, String, Option<String>)> {
	let (name_part, version) = if let Some((n, v)) = tag.split_once('@') {
		(n, Some(v.to_string()))
	} else {
		(tag, None)
	};

	let (category, variant) = name_part.split_once(':').context(format!(
		"Invalid agent tag '{tag}': expected 'category:variant'"
	))?;

	if category.is_empty() || variant.is_empty() {
		anyhow::bail!("Invalid agent tag '{tag}': category and variant must be non-empty");
	}

	Ok((category.to_string(), variant.to_string(), version))
}

/// Local cache path for a manifest.
fn cache_path(category: &str, variant: &str) -> Result<PathBuf> {
	let dir = crate::directories::get_octomind_data_dir()?
		.join("agents")
		.join(category);
	fs::create_dir_all(&dir).context(format!(
		"Failed to create agent cache dir: {}",
		dir.display()
	))?;
	Ok(dir.join(format!("{variant}.toml")))
}

/// Check whether a cached file is stale (older than ttl).
fn is_stale(path: &PathBuf, ttl_hours: u64) -> bool {
	let Ok(meta) = fs::metadata(path) else {
		return true;
	};
	let Ok(modified) = meta.modified() else {
		return true;
	};
	let age = SystemTime::now()
		.duration_since(modified)
		.unwrap_or(Duration::MAX);
	age > Duration::from_secs(ttl_hours * 3600)
}

/// Fetch raw TOML from a tap for the given category/variant.
///
/// For GitHub taps: reads from cloned repo at `~/.local/share/octomind/taps/user/octomind-repo/`
/// For local taps: reads from the specified local path
async fn fetch_from_tap(
	tap: &crate::agent::taps::Tap,
	category: &str,
	variant: &str,
) -> Result<String> {
	let agents_dir = tap.agents_dir()?;
	let manifest_path = agents_dir.join(category).join(format!("{variant}.toml"));
	fs::read_to_string(&manifest_path).context(format!(
		"Failed to read manifest from tap '{}': {}",
		tap.name,
		manifest_path.display()
	))
}

/// Fetch a manifest for `tag` from the registry, using cache when fresh.
///
/// Sources are loaded from user taps (user taps first, built-in default last).
/// If the same manifest is found in multiple taps, the first one wins and a
/// warning is printed.
///
/// Returns `(raw_toml, tap_root)` — the TOML string and the root directory of
/// the tap that provides it (used to locate dep scripts at `<tap_root>/deps/`).
pub async fn fetch_manifest(tag: &str, registry: &RegistryConfig) -> Result<(String, PathBuf)> {
	let (category, variant, _version) = parse_tag(tag)?;
	let cache = cache_path(&category, &variant)?;

	let taps = crate::agent::taps::load_taps().unwrap_or_else(|_| {
		vec![crate::agent::taps::Tap {
			name: crate::agent::taps::DEFAULT_TAP.to_string(),
			local_path: None,
		}]
	});

	// Find the first tap that provides this manifest — this is always the tap root
	// used for dep scripts, regardless of whether we serve TOML from cache.
	let providing_tap = taps
		.iter()
		.find(|tap| {
			tap.agents_dir()
				.map(|d| d.join(&category).join(format!("{variant}.toml")).exists())
				.unwrap_or(false)
		})
		.cloned();

	// Warn if multiple taps provide this manifest (first wins, like Homebrew)
	let providing_count = taps
		.iter()
		.filter(|tap| {
			tap.agents_dir()
				.map(|d| d.join(&category).join(format!("{variant}.toml")).exists())
				.unwrap_or(false)
		})
		.count();
	if providing_count > 1 {
		if let Some(ref tap) = providing_tap {
			crate::log_debug!(
				"'{}' found in multiple taps — using first match: {}",
				tag,
				tap.name
			);
		}
	}

	// Resolve tap root — fall back to default tap dir if no live tap found
	let tap_root = providing_tap
		.as_ref()
		.and_then(|t| t.local_dir().ok())
		.unwrap_or_else(|| {
			crate::agent::taps::Tap {
				name: crate::agent::taps::DEFAULT_TAP.to_string(),
				local_path: None,
			}
			.local_dir()
			.unwrap_or_default()
		});

	// Return cached copy if fresh
	if cache.exists() && !is_stale(&cache, registry.cache_ttl_hours) {
		let toml = fs::read_to_string(&cache).context(format!(
			"Failed to read cached manifest: {}",
			cache.display()
		))?;
		return Ok((toml, tap_root));
	}

	// If stale but exists, return cached and refresh in background
	if cache.exists() {
		let cached = fs::read_to_string(&cache).context(format!(
			"Failed to read cached manifest: {}",
			cache.display()
		))?;

		let cat = category.clone();
		let var = variant.clone();
		let cache_path_bg = cache.clone();
		let taps_bg = taps.clone();
		tokio::spawn(async move {
			for tap in &taps_bg {
				if let Ok(content) = fetch_from_tap(tap, &cat, &var).await {
					let _ = fs::write(&cache_path_bg, content);
					return;
				}
			}
		});

		return Ok((cached, tap_root));
	}

	// No cache — fetch synchronously from taps in order
	let mut last_err = anyhow::anyhow!("No taps configured");
	for tap in &taps {
		match fetch_from_tap(tap, &category, &variant).await {
			Ok(content) => {
				let _ = fs::write(&cache, &content);
				let root = tap.local_dir().unwrap_or_default();
				return Ok((content, root));
			}
			Err(e) => {
				last_err = e;
			}
		}
	}

	Err(last_err).context(format!(
		"Failed to fetch agent manifest for '{}' from all taps",
		tag
	))
}
