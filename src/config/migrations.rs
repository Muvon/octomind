// Copyright 2025 Muvon Un Limited
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

//! Configuration migration system
//!
//! This module handles automatic upgrades of configuration files when the
//! config version changes. Each version increment should have a corresponding
//! migration function.

use super::CURRENT_CONFIG_VERSION;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Check if config needs upgrading and perform automatic migration
pub fn check_and_upgrade_config(config_path: &Path) -> Result<bool> {
	// Try to load the config to check version
	let config_content =
		fs::read_to_string(config_path).context("Failed to read config file for version check")?;

	// Parse just to get the version field - use a more lenient approach
	let parsed_toml: toml::Value =
		toml::from_str(&config_content).context("Failed to parse config file for version check")?;

	// Extract version, defaulting to 0 if not present (for backward compatibility)
	let current_version = parsed_toml
		.get("version")
		.and_then(|v| v.as_integer())
		.unwrap_or(0) as u32;

	if current_version < CURRENT_CONFIG_VERSION {
		println!(
			"🔄 Config version {} detected, upgrading to version {}...",
			current_version, CURRENT_CONFIG_VERSION
		);

		// Perform the migration by modifying the TOML content directly
		let upgraded_content = migrate_config_content(&config_content, current_version)?;

		// Backup the old config
		let backup_path = config_path.with_extension("toml.backup");
		fs::copy(config_path, &backup_path).context("Failed to create config backup")?;

		// Write the upgraded config
		fs::write(config_path, upgraded_content).context("Failed to write upgraded config")?;

		println!(
			"✅ Config upgraded successfully! Backup saved to: {}",
			backup_path.display()
		);

		return Ok(true);
	}

	Ok(false)
}

/// Migrate config content by modifying TOML text directly (preserves formatting and comments)
fn migrate_config_content(content: &str, from_version: u32) -> Result<String> {
	let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
	let mut current_version = from_version;

	// Apply migrations incrementally
	while current_version < CURRENT_CONFIG_VERSION {
		match current_version {
			0 => {
				// Migration from v0 to v1: Update version field if it exists, or add it if missing
				let mut version_found = false;

				for line in lines.iter_mut() {
					if line.trim().starts_with("version = ") {
						*line = "version = 1".to_string();
						version_found = true;
						break;
					}
				}

				// If version field not found, add it at the beginning
				if !version_found {
					// Find the first non-comment line to insert version before it
					let mut insert_pos = 0;
					for (i, line) in lines.iter().enumerate() {
						let trimmed = line.trim();
						if !trimmed.is_empty() && !trimmed.starts_with('#') {
							insert_pos = i;
							break;
						}
					}

					// Insert version field with a comment
					lines.insert(
						insert_pos,
						"# Configuration version (DO NOT MODIFY - used for automatic upgrades)"
							.to_string(),
					);
					lines.insert(insert_pos + 1, "version = 1".to_string());
					lines.insert(insert_pos + 2, "".to_string());
				}

				current_version = 1;
			}
			// Future migrations will go here
			_ => {
				current_version += 1;
			}
		}
	}

	println!(
		"🔄 Applied migration from version {} to {}",
		from_version, current_version
	);
	Ok(lines.join("\n"))
}

/// Force upgrade config file (for manual --upgrade command)
pub fn force_upgrade_config(config_path: &Path) -> Result<()> {
	if !config_path.exists() {
		return Err(anyhow::anyhow!(
			"Config file not found: {}",
			config_path.display()
		));
	}

	let config_content = fs::read_to_string(config_path).context("Failed to read config file")?;

	// Parse just to get the version field
	let parsed_toml: toml::Value =
		toml::from_str(&config_content).context("Failed to parse config file")?;

	let current_version = parsed_toml
		.get("version")
		.and_then(|v| v.as_integer())
		.unwrap_or(0) as u32;

	if current_version >= CURRENT_CONFIG_VERSION {
		println!(
			"✅ Config is already at the latest version ({})",
			current_version
		);
		return Ok(());
	}

	println!(
		"🔄 Upgrading config from version {} to {}...",
		current_version, CURRENT_CONFIG_VERSION
	);

	// Perform the migration
	let upgraded_content = migrate_config_content(&config_content, current_version)?;

	// Backup the old config
	let backup_path = config_path.with_extension("toml.backup");
	fs::copy(config_path, &backup_path).context("Failed to create config backup")?;

	// Write the upgraded config
	fs::write(config_path, upgraded_content).context("Failed to write upgraded config")?;

	println!(
		"✅ Config upgraded successfully! Backup saved to: {}",
		backup_path.display()
	);

	Ok(())
}

// Future migration functions will be added here as needed
// Example:
// fn migrate_from_v1_to_v2(mut config: Config) -> Result<Config> {
//     // Perform specific v1 -> v2 migration logic
//     config.version = 2;
//     Ok(config)
// }
