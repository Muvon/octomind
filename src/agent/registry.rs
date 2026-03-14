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

/// Fetch raw TOML from a single source URL for the given category/variant.
async fn fetch_from_source(source: &str, category: &str, variant: &str) -> Result<String> {
	if let Some(local_path) = source.strip_prefix("file://") {
		// Local file source — expand ~ manually
		let expanded = if local_path.starts_with('~') {
			let home = dirs::home_dir().context("Cannot determine home directory")?;
			home.join(&local_path[2..])
		} else {
			PathBuf::from(local_path)
		};
		// Source path IS the agents root — look for <category>/<variant>.toml directly
		let manifest_path = expanded.join(category).join(format!("{variant}.toml"));
		return fs::read_to_string(&manifest_path).context(format!(
			"Failed to read local manifest: {}",
			manifest_path.display()
		));
	}

	// HTTP(S) source
	let url = format!("{source}/agents/{category}/{variant}.toml");
	let response = reqwest::get(&url)
		.await
		.context(format!("Failed to fetch manifest from {url}"))?;

	if !response.status().is_success() {
		anyhow::bail!("Registry returned {} for {url}", response.status());
	}

	response
		.text()
		.await
		.context("Failed to read manifest response body")
}

/// Fetch a manifest for `tag` from the registry, using cache when fresh.
///
/// Returns the raw TOML string of the manifest.
pub async fn fetch_manifest(tag: &str, registry: &RegistryConfig) -> Result<String> {
	let (category, variant, _version) = parse_tag(tag)?;
	let cache = cache_path(&category, &variant)?;

	// Return cached copy if fresh
	if cache.exists() && !is_stale(&cache, registry.cache_ttl_hours) {
		return fs::read_to_string(&cache).context(format!(
			"Failed to read cached manifest: {}",
			cache.display()
		));
	}

	// If stale but exists, return cached and refresh in background
	if cache.exists() {
		let cached = fs::read_to_string(&cache).context(format!(
			"Failed to read cached manifest: {}",
			cache.display()
		))?;

		let sources = registry.sources.clone();
		let cat = category.clone();
		let var = variant.clone();
		let cache_path_bg = cache.clone();
		tokio::spawn(async move {
			for source in &sources {
				if let Ok(content) = fetch_from_source(source, &cat, &var).await {
					let _ = fs::write(&cache_path_bg, content);
					return;
				}
			}
		});

		return Ok(cached);
	}

	// No cache — fetch synchronously from sources in order
	let mut last_err = anyhow::anyhow!("No registry sources configured");
	for source in &registry.sources {
		match fetch_from_source(source, &category, &variant).await {
			Ok(content) => {
				// Save to cache
				let _ = fs::write(&cache, &content);
				return Ok(content);
			}
			Err(e) => {
				last_err = e;
			}
		}
	}

	Err(last_err).context(format!(
		"Failed to fetch agent manifest for '{tag}' from all sources"
	))
}
