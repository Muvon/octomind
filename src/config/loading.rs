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

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use super::Config;

const MODEL_PROFILE_FIELDS: [&str; 8] = [
	"reasoning_effort",
	"max_tokens",
	"temperature",
	"top_p",
	"top_k",
	"max_retries",
	"retry_timeout",
	"request_timeout_seconds",
];

/// Accept the pre-v12 scalar/flat spelling at versionless merge boundaries.
/// The canonical serialized form is always a nested `model` table.
fn normalize_model_owner(table: &mut toml::map::Map<String, toml::Value>) {
	let mut profile = match table.remove("model") {
		Some(toml::Value::Table(profile)) => profile,
		Some(toml::Value::String(model)) => {
			let mut profile = toml::map::Map::new();
			profile.insert("name".to_string(), toml::Value::String(model));
			profile
		}
		Some(other) => {
			table.insert("model".to_string(), other);
			return;
		}
		None => toml::map::Map::new(),
	};
	for key in MODEL_PROFILE_FIELDS {
		if let Some(value) = table.remove(key) {
			// The nested v12 spelling is authoritative; flat keys only fill gaps.
			profile.entry(key.to_string()).or_insert(value);
		}
	}
	if !profile.is_empty() {
		table.insert("model".to_string(), toml::Value::Table(profile));
	}
}

fn normalize_model_profile_syntax(value: &mut toml::Value) {
	let Some(root) = value.as_table_mut() else {
		return;
	};
	normalize_model_owner(root);
	if let Some(roles) = root.get_mut("roles").and_then(toml::Value::as_array_mut) {
		for role in roles {
			if let Some(role) = role.as_table_mut() {
				normalize_model_owner(role);
			}
		}
	}
	if let Some(supervisor) = root
		.get_mut("supervisor")
		.and_then(toml::Value::as_table_mut)
	{
		normalize_model_owner(supervisor);
		if let Some(learning) = supervisor
			.get_mut("learning")
			.and_then(toml::Value::as_table_mut)
		{
			learning.remove("model");
			for key in MODEL_PROFILE_FIELDS {
				learning.remove(key);
			}
		}
	}
	if let Some(compression) = root
		.get_mut("compression")
		.and_then(toml::Value::as_table_mut)
	{
		if !compression.contains_key("model") {
			if let Some(decision) = compression.remove("decision") {
				compression.insert("model".to_string(), decision);
			}
		}
		if let Some(model) = compression.get_mut("model") {
			if let Some(profile) = model.as_table_mut() {
				if let Some(name) = profile.remove("model") {
					profile.insert("name".to_string(), name);
				}
			}
		}
	}
}

/// Merge multiple TOML values into one - later values override/add to earlier ones
/// Arrays of tables are concatenated (e.g. [[mcp.servers]]), tables merge deeply, scalars override
fn merge_toml_values(base: &toml::Value, override_: &toml::Value) -> toml::Value {
	match (base, override_) {
		// Merge tables (objects) deeply
		(toml::Value::Table(base_table), toml::Value::Table(override_table)) => {
			let mut result = base_table.clone();
			for (key, value) in override_table {
				if let Some(base_value) = result.get(key) {
					result.insert(key.clone(), merge_toml_values(base_value, value));
				} else {
					result.insert(key.clone(), value.clone());
				}
			}
			toml::Value::Table(result)
		}
		// Concatenate arrays of tables (TOML [[...]] style)
		// This allows split-file configs to ADD entries (e.g. mcp-*.toml adding [[mcp.servers]])
		// Last entry wins for same-name items (dedup by "name" field)
		(toml::Value::Array(base_arr), toml::Value::Array(override_arr))
			if is_array_of_tables(base_arr) || is_array_of_tables(override_arr) =>
		{
			let mut result = base_arr.clone();
			result.extend(override_arr.iter().cloned());
			// Deduplicate by "name" field — last entry wins (override files loaded after base)
			dedup_tables_by_name(result)
		}
		// Scalar arrays and other types: override replaces
		(_, override_) => override_.clone(),
	}
}

/// Check if a TOML array contains only tables (i.e. [[...]] style)
fn is_array_of_tables(arr: &[toml::Value]) -> bool {
	!arr.is_empty() && arr.iter().all(|v| v.is_table())
}

/// Deduplicate an array of tables by "name" field, keeping the last occurrence.
/// Items without a "name" field are always kept.
fn dedup_tables_by_name(arr: Vec<toml::Value>) -> toml::Value {
	let mut seen = std::collections::HashMap::new();
	let mut result = Vec::new();

	// First pass: record last index for each name
	for (i, item) in arr.iter().enumerate() {
		if let Some(name) = item
			.as_table()
			.and_then(|t| t.get("name"))
			.and_then(|v| v.as_str())
		{
			seen.insert(name.to_string(), i);
		}
	}

	// Second pass: keep only the last occurrence of each name (and all unnamed items)
	for (i, item) in arr.into_iter().enumerate() {
		let name = item
			.as_table()
			.and_then(|t| t.get("name"))
			.and_then(|v| v.as_str())
			.map(|s| s.to_string());

		match name {
			Some(n) if seen.get(&n) == Some(&i) => result.push(item),
			Some(_) => {} // Earlier duplicate — skip
			None => result.push(item),
		}
	}

	toml::Value::Array(result)
}

/// Load and merge all TOML files from a directory
/// Order: config.toml first, then other *.toml files in alphabetical order
/// `mcp-*.toml` files are treated as overrides and loaded last.
/// `mcp.toml` (no dash) is a regular file and loads in normal alphabetical order.
fn is_mcp_extension_file(path: &Path) -> bool {
	path.file_name()
		.and_then(|n| n.to_str())
		.map(|n| n.starts_with("mcp-") && n.ends_with(".toml"))
		.unwrap_or(false)
}

fn load_and_merge_toml_from_directory(dir: &Path) -> Result<toml::Value> {
	let mut merged: Option<toml::Value> = None;

	// Read directory and collect TOML files
	let mut files: Vec<_> = fs::read_dir(dir)?
		.filter_map(|entry| entry.ok())
		.map(|e| e.path())
		.filter(|p| p.is_file() && p.extension().map(|e| e == "toml").unwrap_or(false))
		.collect();

	// Load order (documented contract, doc/usage/03-configuration.md): `config.toml`
	// first as the base, then other regular files alphabetically, then `mcp-*.toml`
	// overrides last (so their fields win over a same-named server in `mcp.toml`).
	// Merge is last-wins, so `config.toml` MUST sort first — plain alphabetical
	// order would let e.g. `aliases.toml` load before it and invert the contract.
	fn load_rank(path: &Path) -> u8 {
		if path.file_name().and_then(|n| n.to_str()) == Some("config.toml") {
			0
		} else if is_mcp_extension_file(path) {
			2
		} else {
			1
		}
	}
	files.sort_by(|a, b| load_rank(a).cmp(&load_rank(b)).then_with(|| a.cmp(b)));

	for file in &files {
		let content = fs::read_to_string(file)
			.context(format!("Failed to read TOML file: {}", file.display()))?;

		let mut value: toml::Value = toml::from_str(&content)
			.context(format!("Failed to parse TOML file: {}", file.display()))?;
		normalize_model_profile_syntax(&mut value);

		merged = Some(if let Some(base) = merged {
			merge_toml_values(&base, &value)
		} else {
			value
		});
	}

	merged.ok_or_else(|| {
		anyhow::anyhow!("No TOML files found in config directory: {}", dir.display())
	})
}

impl Config {
	fn initialize_config(&mut self) {}

	pub fn ensure_octomind_dir() -> Result<std::path::PathBuf> {
		// Use the system-wide directory
		crate::directories::get_octomind_data_dir()
	}

	/// Copy the default configuration template when no config exists
	pub fn copy_default_config_template(config_path: &std::path::Path) -> Result<()> {
		// Default config template embedded in binary
		const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../../config-templates/default.toml");

		// Ensure the parent directory exists
		if let Some(parent) = config_path.parent() {
			fs::create_dir_all(parent).context(format!(
				"Failed to create config directory: {}",
				parent.display()
			))?;
		}

		// Write the default template
		fs::write(config_path, DEFAULT_CONFIG_TEMPLATE).context(format!(
			"Failed to write default config template to {}",
			config_path.display()
		))?;

		println!("Created default configuration at {}", config_path.display());
		println!("Please edit the configuration file to set your API keys and preferences.");

		Ok(())
	}

	/// Create default config at the standard location (public version for commands)
	pub fn create_default_config() -> Result<std::path::PathBuf> {
		let config_path = crate::directories::get_config_file_path()?;

		if !config_path.exists() {
			Self::copy_default_config_template(&config_path)?;
		}

		Ok(config_path)
	}

	/// Inject default configuration directly from embedded TOML template
	fn inject_default_config() -> Result<Self> {
		// Use the existing embedded template, but parse directly into memory
		const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../../config-templates/default.toml");

		let mut config: Config = toml::from_str(DEFAULT_CONFIG_TEMPLATE)
			.context("Failed to parse default configuration template")?;

		// Build role map from roles array
		config.build_role_map();

		Ok(config)
	}

	/// Load configuration from the system-wide config file with strict validation
	/// Supports multi-file configuration: reads config.toml and all other *.toml files
	/// in the same directory, merging them into a single configuration.
	///
	/// Environment variable OCTOMIND_CONFIG_PATH can be used to specify a custom config path.
	pub fn load() -> Result<Self> {
		// Check for custom config path from environment variable
		let config_path = if let Ok(custom_path) = std::env::var("OCTOMIND_CONFIG_PATH") {
			std::path::PathBuf::from(custom_path)
		} else {
			// Use the new system-wide config file path
			crate::directories::get_config_file_path()?
		};

		// Get the config directory (config file's parent). A relative single-
		// component path like "myconfig.toml" has a parent of Some("") — treat
		// that empty path as "." so we look in the current dir instead of
		// deciding the directory "doesn't exist" and overwriting the file with
		// the default template.
		let parent = config_path.parent();
		let config_dir = match parent {
			Some(p) if !p.as_os_str().is_empty() => p,
			_ => Path::new("."),
		};

		if !config_dir.exists() {
			// Directory doesn't exist, create default config
			let default_config = Self::inject_default_config()?;
			default_config.save_to_path(&config_path)?;
			return Ok(default_config);
		}

		// Check if config.toml exists
		if !config_path.exists() {
			// No config.toml, but check if there are other toml files
			let has_toml_files = config_dir.read_dir()?.any(|e| {
				e.ok()
					.map(|f| {
						f.file_type()
							.map(|t| {
								t.is_file()
									&& f.path().extension().map(|e| e == "toml").unwrap_or(false)
							})
							.unwrap_or(false)
					})
					.unwrap_or(false)
			});

			if !has_toml_files {
				// No config files at all, inject default
				let default_config = Self::inject_default_config()?;
				default_config.save_to_path(&config_path)?;
				return Ok(default_config);
			}
		}

		// Check for automatic config upgrades on config.toml
		if config_path.exists() {
			super::migrations::check_and_upgrade_config(&config_path)
				.context("Failed to check/upgrade config version")?;
		}

		// Load and merge all TOML files from the config directory
		let merged_value = load_and_merge_toml_from_directory(config_dir)?;

		// Convert to Config struct
		let mut config: Config = merged_value.try_into().context(
			"Failed to parse merged TOML configuration. All required fields must be present.",
		)?;

		// Store the config path for future saves
		config.config_path = Some(config_path);

		// Initialize the configuration
		config.initialize_config();

		// Build role map from roles array
		config.build_role_map();

		// STRICT validation - fail if configuration is invalid
		config.validate()?;

		Ok(config)
	}

	/// REMOVED: No more default_with_env - config must be complete and explicit
	/// All defaults are now in the template file
	///
	/// Save configuration to file
	pub fn save(&self) -> Result<()> {
		// Validate before saving
		self.validate()?;

		// Use the stored config path, or fallback to system-wide default
		let config_path = if let Some(path) = &self.config_path {
			path.clone()
		} else {
			crate::directories::get_config_file_path()?
		};

		// Ensure the parent directory exists
		if let Some(parent) = config_path.parent() {
			fs::create_dir_all(parent).context(format!(
				"Failed to create config directory: {}",
				parent.display()
			))?;
		}

		// Serialize to TOML
		let config_str =
			toml::to_string_pretty(self).context("Failed to serialize configuration to TOML")?;

		// Write to file
		fs::write(&config_path, config_str).context(format!(
			"Failed to write config to {}",
			config_path.display()
		))?;

		// stderr, never stdout: ACP and MCP stdio modes carry JSON-RPC on stdout,
		// and a first run creates this file before the protocol starts.
		eprintln!("Configuration saved to {}", config_path.display());
		Ok(())
	}

	/// Load configuration from a specific file path or directory
	/// If path is a directory: loads and merges all *.toml files (same as load())
	/// If path is a file: loads that single file
	pub fn load_from_path(path: &std::path::Path) -> Result<Self> {
		if path.is_dir() {
			// Load and merge all TOML files from directory
			let merged_value = load_and_merge_toml_from_directory(path)?;

			let mut config: Config = merged_value
				.try_into()
				.context("Failed to parse merged TOML configuration")?;

			config.config_path = Some(path.join("config.toml"));
			config.initialize_config();
			config.build_role_map();
			config.validate()?;

			Ok(config)
		} else {
			// Load single file
			let config_str = fs::read_to_string(path)
				.context(format!("Failed to read config from {}", path.display()))?;
			let mut value: toml::Value =
				toml::from_str(&config_str).context("Failed to parse TOML configuration")?;
			normalize_model_profile_syntax(&mut value);
			let mut config: Config = value
				.try_into()
				.context("Failed to parse TOML configuration")?;

			config.config_path = Some(path.to_path_buf());
			config.initialize_config();
			config.build_role_map();
			config.validate()?;

			Ok(config)
		}
	}

	/// Save configuration to a specific file path
	pub fn save_to_path(&self, path: &std::path::Path) -> Result<()> {
		// Validate before saving
		self.validate()?;

		// Ensure the parent directory exists
		if let Some(parent) = path.parent() {
			fs::create_dir_all(parent).context(format!(
				"Failed to create config directory: {}",
				parent.display()
			))?;
		}

		// Serialize to TOML
		let config_str =
			toml::to_string_pretty(self).context("Failed to serialize configuration to TOML")?;

		// Write to file
		fs::write(path, config_str)
			.context(format!("Failed to write config to {}", path.display()))?;

		eprintln!("Configuration saved to {}", path.display());
		Ok(())
	}

	/// Create a clean copy of the config for saving (removes runtime-only fields)
	pub fn create_clean_copy_for_saving(&self) -> Self {
		// Only remove servers that are marked as runtime-only or temporary
		// (Currently there are no runtime-only servers, so we keep all servers)

		// Keep the MCP section to show what's actually available

		self.clone()
	}

	/// Update configuration with a closure and save
	pub fn update_and_save<F>(&mut self, updater: F) -> Result<()>
	where
		F: FnOnce(&mut Self),
	{
		// Validate before saving
		self.validate()?;

		// Use the stored config path, or fallback to system-wide default
		let config_path = if let Some(path) = &self.config_path {
			path.clone()
		} else {
			crate::directories::get_config_file_path()?
		};

		// Ensure the parent directory exists
		if let Some(parent) = config_path.parent() {
			fs::create_dir_all(parent).context(format!(
				"Failed to create config directory: {}",
				parent.display()
			))?;
		}

		// Create clean config for saving (no internal servers)
		let clean_config = self.create_clean_copy_for_saving();
		let config_str =
			toml::to_string(&clean_config).context("Failed to serialize configuration to TOML")?;

		// Write to file
		fs::write(&config_path, config_str).context(format!(
			"Failed to write config to {}",
			config_path.display()
		))?;

		// Update self with the changes (but keep internal servers in memory)
		updater(self);

		Ok(())
	}

	/// REMOVED: create_default_config - use copy_default_config_template instead
	/// This ensures all defaults come from the template file, not code
	///
	/// Update a specific field in the configuration and save to disk
	/// STRICT MODE: Requires existing config file
	pub fn update_specific_field<F>(&mut self, updater: F) -> Result<()>
	where
		F: Fn(&mut Config),
	{
		// Load existing config from disk without initializing internal servers
		let config_path = if let Some(path) = &self.config_path {
			path.clone()
		} else {
			crate::directories::get_config_file_path()?
		};

		let mut disk_config = if config_path.exists() {
			let config_str = fs::read_to_string(&config_path).context(format!(
				"Failed to read config from {}",
				config_path.display()
			))?;
			let mut config: Config =
				toml::from_str(&config_str).context("Failed to parse TOML configuration")?;
			config.config_path = Some(config_path.clone());
			// SIMPLIFIED: Don't initialize internal servers
			config
		} else {
			// STRICT MODE: Fail if no config file exists
			return Err(anyhow::anyhow!(
				"No configuration file found at {}. Run with --init to create a default configuration.",
				config_path.display()
			));
		};

		// Apply the update to the disk config
		updater(&mut disk_config);

		// Validate the updated config
		disk_config.validate()?;

		// Ensure the parent directory exists
		if let Some(parent) = config_path.parent() {
			fs::create_dir_all(parent).context(format!(
				"Failed to create config directory: {}",
				parent.display()
			))?;
		}

		// Create clean config for saving (no internal servers)
		let clean_config = disk_config.create_clean_copy_for_saving();
		let config_str =
			toml::to_string(&clean_config).context("Failed to serialize configuration to TOML")?;

		// Write to file
		fs::write(&config_path, config_str).context(format!(
			"Failed to write config to {}",
			config_path.display()
		))?;

		// Update self with the changes (but keep internal servers in memory)
		updater(self);

		Ok(())
	}
}

/// Merge an agent manifest TOML string into an existing Config.
///
/// Unlike regular multi-file merging (where arrays replace), this function
/// **concatenates** `mcp.servers` and `roles` arrays so the agent's additions
/// stack on top of the user's base config. All other keys use override semantics.
pub fn merge_agent_toml(base: &Config, agent_toml: &str) -> Result<Config> {
	let mut agent_value: toml::Value =
		toml::from_str(agent_toml).context("Failed to parse agent manifest TOML")?;
	normalize_model_profile_syntax(&mut agent_value);

	// Serialize base config to toml::Value so we can manipulate it
	let base_str = toml::to_string(base).context("Failed to serialize base config")?;
	let mut base_value: toml::Value =
		toml::from_str(&base_str).context("Failed to re-parse base config")?;

	// Concatenate mcp.servers (additive, skip duplicates by name). The base may
	// have no [mcp] table or no servers array at all (common — many user configs
	// declare no MCP servers); create them so the agent's servers are still
	// merged instead of silently dropped.
	if let Some(toml::Value::Table(agent_mcp)) = agent_value.get("mcp") {
		if let Some(toml::Value::Array(agent_servers)) = agent_mcp.get("servers") {
			let base_mcp = base_value
				.as_table_mut()
				.expect("base config serializes to a TOML table")
				.entry("mcp")
				.or_insert_with(|| toml::Value::Table(Default::default()));
			let base_servers = base_mcp
				.as_table_mut()
				.expect("mcp is a table")
				.entry("servers")
				.or_insert_with(|| toml::Value::Array(Vec::new()));
			if let toml::Value::Array(base_servers) = base_servers {
				let existing_names: std::collections::HashSet<String> = base_servers
					.iter()
					.filter_map(|s| {
						s.get("name")
							.and_then(|n| n.as_str())
							.map(|n| n.to_string())
					})
					.collect();
				for server in agent_servers {
					let name = server.get("name").and_then(|n| n.as_str()).unwrap_or("");
					if !existing_names.contains(name) {
						base_servers.push(server.clone());
					}
				}
			}
		}
	}

	// Concatenate roles (additive, skip duplicates by name)
	if let (Some(toml::Value::Array(base_roles)), Some(toml::Value::Array(agent_roles))) =
		(base_value.get_mut("roles"), agent_value.get("roles"))
	{
		let existing_names: std::collections::HashSet<String> = base_roles
			.iter()
			.filter_map(|r| {
				r.get("name")
					.and_then(|n| n.as_str())
					.map(|n| n.to_string())
			})
			.collect();
		for role in agent_roles {
			let name = role.get("name").and_then(|n| n.as_str()).unwrap_or("");
			if !existing_names.contains(name) {
				base_roles.push(role.clone());
			}
		}
	}

	// Merge remaining keys with override semantics (tables deep-merge, scalars replace).
	// mcp and roles are already handled above — skip them here.
	if let toml::Value::Table(agent_table) = &agent_value {
		if let toml::Value::Table(base_table) = &mut base_value {
			for (key, value) in agent_table {
				if key == "mcp" || key == "roles" {
					continue;
				}
				if let Some(base_val) = base_table.get(key) {
					let merged = merge_toml_values(base_val, value);
					base_table.insert(key.clone(), merged);
				} else {
					base_table.insert(key.clone(), value.clone());
				}
			}
		}
	}

	let mut merged: Config = base_value
		.try_into()
		.context("Failed to deserialize merged agent config")?;
	merged.build_role_map();
	Ok(merged)
}

#[cfg(test)]
#[path = "loading_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "loading_tests.rs"]
mod path_tests;
