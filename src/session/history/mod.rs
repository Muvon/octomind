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

// Role-based history management system

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;

lazy_static::lazy_static! {
	// Per-role mutexes for thread-safe history operations
	static ref HISTORY_MUTEXES: Mutex<HashMap<String, Mutex<()>>> = Mutex::new(HashMap::new());
}

/// Get the history directory path
pub fn get_history_dir() -> Result<PathBuf> {
	let data_dir = crate::directories::get_octomind_data_dir()?;
	let history_dir = data_dir.join("history");

	// Check if legacy history file exists BEFORE creating directory
	// This handles the case where old version had "history" as a file
	let legacy_history_file = data_dir.join("history");
	if legacy_history_file.exists() && legacy_history_file.is_file() {
		// Legacy file exists - we'll handle migration later, but first rename it
		// to avoid conflicts when creating the directory
		let backup_path = data_dir.join("history.legacy");
		fs::rename(&legacy_history_file, &backup_path).with_context(|| {
			format!(
				"Failed to rename legacy history file from {:?} to {:?}",
				legacy_history_file, backup_path
			)
		})?;
		crate::log_debug!(
			"Renamed legacy history file to: {:?} (will be migrated on first use)",
			backup_path
		);
	}

	// Now ensure history directory exists
	if !history_dir.exists() {
		fs::create_dir_all(&history_dir)
			.with_context(|| format!("Failed to create history directory: {:?}", history_dir))?;
	}

	Ok(history_dir)
}

/// Get the session history file path for a specific role
pub fn get_session_history_file_path(role: &str) -> Result<PathBuf> {
	let history_dir = get_history_dir()?;
	Ok(history_dir.join(format!("session_{}.history", role)))
}

/// Get or create a mutex for a specific role's history file
fn get_role_mutex(role: &str) -> std::sync::MutexGuard<'static, ()> {
	let mut mutexes = HISTORY_MUTEXES.lock().unwrap();

	// Create mutex for this role if it doesn't exist
	if !mutexes.contains_key(role) {
		mutexes.insert(role.to_string(), Mutex::new(()));
	}

	// Get reference to the mutex (this is safe because we never remove mutexes)
	let mutex_ref = mutexes.get(role).unwrap() as *const Mutex<()>;
	drop(mutexes); // Release the outer mutex

	// Safety: The mutex reference is valid because we never remove mutexes from the HashMap
	unsafe { (*mutex_ref).lock().unwrap() }
}

/// Migrate old global history file to role-based system
fn migrate_legacy_history_if_needed(role: &str) -> Result<()> {
	// Only migrate for developer role to avoid duplicating history across roles
	if role != "developer" {
		return Ok(());
	}

	let data_dir = crate::directories::get_octomind_data_dir()?;
	let legacy_backup = data_dir.join("history.legacy");
	let role_history_path = get_session_history_file_path(role)?;

	// Check if legacy backup exists and role-specific file doesn't exist yet
	if legacy_backup.exists() && legacy_backup.is_file() && !role_history_path.exists() {
		crate::log_debug!(
			"Migrating legacy global history to role-based system for role: {}",
			role
		);

		// Read legacy history
		let legacy_content = fs::read_to_string(&legacy_backup)
			.with_context(|| format!("Failed to read legacy history file: {:?}", legacy_backup))?;

		// Create new role-specific history file with version marker
		let mut role_file = OpenOptions::new()
			.create(true)
			.truncate(true)
			.write(true)
			.open(&role_history_path)
			.with_context(|| {
				format!(
					"Failed to create role history file: {:?}",
					role_history_path
				)
			})?;

		writeln!(role_file, "# OCTOMIND_HISTORY_VERSION=1")?;

		// Process and migrate each line from legacy file
		for line in legacy_content.lines() {
			if !line.trim().is_empty() {
				// Encode newlines to preserve multiline entries as single history records
				let encoded_line = line.replace("\\", "\\\\").replace("\n", "\\n");
				writeln!(role_file, "{}", encoded_line)?;
			}
		}

		role_file.flush()?;

		// Rename legacy backup to mark as migrated
		let migrated_path = data_dir.join("history.migrated");
		fs::rename(&legacy_backup, &migrated_path).with_context(|| {
			format!(
				"Failed to rename legacy backup file to: {:?}",
				migrated_path
			)
		})?;

		crate::log_debug!(
			"Successfully migrated legacy history to: {:?}",
			role_history_path
		);
		crate::log_debug!("Legacy file marked as migrated: {:?}", migrated_path);
	}

	Ok(())
}

/// Load history from a role-specific file
pub fn load_session_history_from_file(role: &str) -> Result<Vec<String>> {
	let _lock = get_role_mutex(role);

	// Migrate legacy history if needed (only for developer role)
	migrate_legacy_history_if_needed(role)?;

	let history_path = get_session_history_file_path(role)?;

	if !history_path.exists() {
		return Ok(Vec::new());
	}

	let file = std::fs::File::open(&history_path)
		.with_context(|| format!("Failed to open history file: {:?}", history_path))?;

	let reader = BufReader::new(file);
	let mut history_lines = Vec::new();

	for line in reader.lines() {
		let line = line.with_context(|| "Failed to read line from history file")?;

		// Skip version marker and empty lines
		if line.starts_with("# OCTOMIND_HISTORY_VERSION") || line.trim().is_empty() {
			continue;
		}

		// Decode newlines to restore multiline entries
		let decoded_line = line.replace("\\n", "\n").replace("\\\\", "\\");
		history_lines.push(decoded_line);
	}

	Ok(history_lines)
}

/// Append a line to role-specific history file
pub fn append_to_session_history_file(role: &str, line: &str) -> Result<()> {
	let _lock = get_role_mutex(role);
	let history_path = get_session_history_file_path(role)?;

	// Ensure file exists with version marker
	if !history_path.exists() {
		let mut file = OpenOptions::new()
			.create(true)
			.truncate(true)
			.write(true)
			.open(&history_path)
			.with_context(|| format!("Failed to create history file: {:?}", history_path))?;

		writeln!(file, "# OCTOMIND_HISTORY_VERSION=1")?;
		file.flush()?;
	}

	// Append the new line
	let mut file = OpenOptions::new()
		.create(true)
		.append(true)
		.open(&history_path)
		.with_context(|| format!("Failed to open history file for append: {:?}", history_path))?;

	// Encode newlines to preserve multiline entries as single history records
	let encoded_line = line.replace("\\", "\\\\").replace("\n", "\\n");
	writeln!(file, "{}", encoded_line)?;
	file.flush()?;

	Ok(())
}
