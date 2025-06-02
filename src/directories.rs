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

// Directory utilities for cross-platform data directory management

use anyhow::{Result, Context};
use std::path::PathBuf;
use std::fs;

/// Get the system-wide data directory for octomind
///
/// This function returns the appropriate data directory based on the OS:
/// - macOS: ~/.local/share/octomind
/// - Linux: ~/.local/share/octomind (following XDG Base Directory specification)
/// - Windows: %LOCALAPPDATA%/octomind
pub fn get_octomind_data_dir() -> Result<PathBuf> {
	let data_dir = match dirs::home_dir() {
		Some(home) => {
			#[cfg(target_os = "windows")]
			let path = {
				// On Windows, use %LOCALAPPDATA%/octomind
				match dirs::data_local_dir() {
					Some(dir) => dir.join("octomind"),
					None => home.join("AppData").join("Local").join("octomind"),
				}
			};

			#[cfg(not(target_os = "windows"))]
			let path = home.join(".local").join("share").join("octomind");

			path
		},
		None => {
			return Err(anyhow::anyhow!("Unable to determine home directory"));
		}
	};

	// Ensure the directory exists
	if !data_dir.exists() {
		fs::create_dir_all(&data_dir)
			.context(format!("Failed to create octomind data directory: {}", data_dir.display()))?;
	}

	Ok(data_dir)
}

/// Get the configuration directory path
pub fn get_config_dir() -> Result<PathBuf> {
	let data_dir = get_octomind_data_dir()?;
	let config_dir = data_dir.join("config");

	if !config_dir.exists() {
		fs::create_dir_all(&config_dir)?;
	}

	Ok(config_dir)
}

/// Get the sessions directory path
pub fn get_sessions_dir() -> Result<PathBuf> {
	let data_dir = get_octomind_data_dir()?;
	let sessions_dir = data_dir.join("sessions");

	if !sessions_dir.exists() {
		fs::create_dir_all(&sessions_dir)?;
	}

	Ok(sessions_dir)
}

/// Get the logs directory path
pub fn get_logs_dir() -> Result<PathBuf> {
	let data_dir = get_octomind_data_dir()?;
	let logs_dir = data_dir.join("logs");

	if !logs_dir.exists() {
		fs::create_dir_all(&logs_dir)?;
	}

	Ok(logs_dir)
}

/// Get the cache directory path
pub fn get_cache_dir() -> Result<PathBuf> {
	let data_dir = get_octomind_data_dir()?;
	let cache_dir = data_dir.join("cache");

	if !cache_dir.exists() {
		fs::create_dir_all(&cache_dir)?;
	}

	Ok(cache_dir)
}

/// Get the default configuration file path
pub fn get_config_file_path() -> Result<PathBuf> {
	let config_dir = get_config_dir()?;
	Ok(config_dir.join("config.toml"))
}

/// Display information about the data directory locations
pub fn print_directory_info() -> Result<()> {
	println!("Octomind Data Directories:");
	println!("  Data Dir:     {}", get_octomind_data_dir()?.display());
	println!("  Config Dir:   {}", get_config_dir()?.display());
	println!("  Sessions Dir: {}", get_sessions_dir()?.display());
	println!("  Logs Dir:     {}", get_logs_dir()?.display());
	println!("  Cache Dir:    {}", get_cache_dir()?.display());

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_get_octomind_data_dir() {
		let result = get_octomind_data_dir();
		assert!(result.is_ok());

		let path = result.unwrap();
		assert!(path.to_string_lossy().contains("octomind"));

		// The directory should exist after calling the function
		assert!(path.exists());
	}

	#[test]
	fn test_subdirectories() {
		// Test that all subdirectory functions work
		assert!(get_config_dir().is_ok());
		assert!(get_sessions_dir().is_ok());
		assert!(get_logs_dir().is_ok());
		assert!(get_cache_dir().is_ok());
	}

	#[test]
	fn test_config_file_path() {
		let config_path = get_config_file_path().unwrap();
		assert!(config_path.to_string_lossy().ends_with("config.toml"));
	}
}
