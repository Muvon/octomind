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

//! Environment variable source tracking
//!
//! This module provides functionality to track where environment variables
//! come from (system environment vs .env file) for better debugging and
//! status reporting.

use std::collections::HashMap;
use std::env;
use std::path::Path;

/// Represents the source of an environment variable
#[derive(Debug, Clone, PartialEq)]
pub enum EnvSource {
	/// Variable was set in the system environment
	System,
	/// Variable was loaded from .env file
	DotEnv,
	/// Variable was not found
	NotFound,
}

/// Environment variable tracker that can determine the source of variables
pub struct EnvTracker {
	/// Variables that existed before .env loading
	pre_dotenv_vars: HashMap<String, String>,
	/// Whether .env file exists and was loaded
	dotenv_loaded: bool,
}

impl EnvTracker {
	/// Create a new tracker and capture current environment state
	pub fn new() -> Self {
		let pre_dotenv_vars = env::vars().collect();
		Self {
			pre_dotenv_vars,
			dotenv_loaded: false,
		}
	}

	/// Load .env file with override and track that it was loaded
	pub fn load_dotenv_override(&mut self) -> Result<(), dotenvy::Error> {
		if Path::new(".env").exists() {
			dotenvy::from_filename_override(".env")?;
			self.dotenv_loaded = true;
			crate::log_debug!("Loaded .env file with override - tracking enabled");
		}

		// Check if extra variables are not set and set them if needed
		// This is a bit tricky but we set it to use in octolib to make it easier
		if std::env::var("OPENROUTER_APP_TITLE").is_err() {
			std::env::set_var("OPENROUTER_APP_TITLE", "Octomind");
		}

		if std::env::var("OPENROUTER_HTTP_REFERER").is_err() {
			std::env::set_var("OPENROUTER_HTTP_REFERER", "https://octomind.muvon.io");
		}

		Ok(())
	}

	/// Determine the source of an environment variable
	pub fn get_source(&self, var_name: &str) -> EnvSource {
		match env::var(var_name) {
			Ok(current_value) => {
				// Treat empty values as not found for API keys
				if current_value.trim().is_empty() {
					return EnvSource::NotFound;
				}

				if !self.dotenv_loaded {
					// No .env file loaded, must be from system
					return EnvSource::System;
				}

				// Check if variable existed before .env loading
				match self.pre_dotenv_vars.get(var_name) {
					Some(pre_value) => {
						if current_value == *pre_value {
							// Value unchanged, came from system
							EnvSource::System
						} else {
							// Value changed, came from .env override
							EnvSource::DotEnv
						}
					}
					None => {
						// Variable didn't exist before, came from .env
						EnvSource::DotEnv
					}
				}
			}
			Err(_) => EnvSource::NotFound,
		}
	}

	/// Get a formatted source description for display
	pub fn get_source_description(&self, var_name: &str) -> &'static str {
		match self.get_source(var_name) {
			EnvSource::System => "environment variable",
			EnvSource::DotEnv => "environment/.env file",
			EnvSource::NotFound => "not set",
		}
	}

	/// Check if .env file was loaded
	pub fn is_dotenv_loaded(&self) -> bool {
		self.dotenv_loaded
	}
}

impl Default for EnvTracker {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::env;

	#[test]
	fn test_env_tracker_new() {
		let tracker = EnvTracker::new();
		// Should capture current environment
		assert!(!tracker.pre_dotenv_vars.is_empty());
		assert!(!tracker.dotenv_loaded);
	}

	#[test]
	fn test_source_detection_system_only() {
		// Set a test variable
		env::set_var("TEST_SYSTEM_VAR", "system_value");

		let tracker = EnvTracker::new();

		// Should detect as system source
		assert_eq!(tracker.get_source("TEST_SYSTEM_VAR"), EnvSource::System);
		assert_eq!(
			tracker.get_source_description("TEST_SYSTEM_VAR"),
			"environment variable"
		);

		// Clean up
		env::remove_var("TEST_SYSTEM_VAR");
	}

	#[test]
	fn test_source_detection_not_found() {
		let tracker = EnvTracker::new();

		// Should detect as not found
		assert_eq!(tracker.get_source("NONEXISTENT_VAR"), EnvSource::NotFound);
		assert_eq!(tracker.get_source_description("NONEXISTENT_VAR"), "not set");
	}
}
