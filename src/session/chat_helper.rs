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

// Implementation of a command completer for reedline
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use lazy_static::lazy_static;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct Pair {
	pub display: String,
	pub replacement: String,
}

lazy_static! {
	static ref FILE_CACHE: Mutex<Option<Vec<String>>> = Mutex::new(None);
}

pub(crate) struct CommandCompleter<'a> {
	commands: Vec<String>,
	config: &'a crate::config::Config,
	role: &'a str,
}

impl<'a> CommandCompleter<'a> {
	pub(crate) fn new(config: &'a crate::config::Config, role: &'a str) -> Self {
		let commands = crate::session::chat::COMMANDS
			.iter()
			.map(|&s| s.to_string())
			.collect();
		Self {
			commands,
			config,
			role,
		}
	}

	/// Get available context filters for /context command
	fn get_context_filters() -> Vec<&'static str> {
		vec!["all", "assistant", "user", "tool", "large"]
	}

	/// Get available MCP subcommands for /mcp command
	fn get_mcp_subcommands() -> Vec<&'static str> {
		vec!["list", "info", "full", "health", "dump", "validate"]
	}

	/// Get available cache subcommands for /cache command
	fn get_cache_subcommands() -> Vec<&'static str> {
		vec!["stats", "clear", "threshold"]
	}

	/// Get available log levels for /loglevel command
	fn get_log_levels() -> Vec<&'static str> {
		vec!["none", "info", "debug"]
	}

	/// Get available roles for /role command
	fn get_available_roles(&self) -> Vec<String> {
		self.config.roles.iter().map(|r| r.name.clone()).collect()
	}

	/// Get available workflows for /workflow command
	fn get_available_workflows(&self) -> Vec<String> {
		self.config
			.workflows
			.iter()
			.map(|w| w.name.clone())
			.collect()
	}

	/// Check if the given file extension is a supported image format
	fn is_image_file(path: &str) -> bool {
		let supported_extensions = [
			".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp", ".tiff", ".tif", ".ico", ".svg",
			".avif", ".heic", ".heif",
		];

		let path_lower = path.to_lowercase();
		supported_extensions
			.iter()
			.any(|ext| path_lower.ends_with(ext))
	}

	/// Expand tilde (~) to home directory
	fn expand_tilde(path: &str) -> PathBuf {
		if let Some(stripped) = path.strip_prefix("~/") {
			if let Some(home) = dirs::home_dir() {
				home.join(stripped)
			} else {
				PathBuf::from(path)
			}
		} else if path == "~" {
			dirs::home_dir().unwrap_or_else(|| PathBuf::from(path))
		} else {
			PathBuf::from(path)
		}
	}

	fn find_at_query(line: &str, pos: usize) -> Option<(usize, &str)> {
		let before_cursor = &line[..pos.min(line.len())];
		let at_pos = before_cursor.rfind('@')?;
		let before_at = before_cursor[..at_pos].chars().last();
		if at_pos == 0 || before_at.map(char::is_whitespace).unwrap_or(true) {
			let query = &before_cursor[at_pos + 1..];
			if query.chars().any(char::is_whitespace) {
				return None;
			}
			return Some((at_pos, query));
		}
		None
	}

	fn fuzzy_match_files(query: &str, max_results: usize) -> Vec<Pair> {
		let files = Self::get_all_files();
		if files.is_empty() {
			return Vec::new();
		}

		let matcher = SkimMatcherV2::default();
		let mut scored: Vec<(i64, &str)> = files
			.iter()
			.filter_map(|path| {
				matcher
					.fuzzy_match(path, query)
					.map(|score| (score, path.as_str()))
			})
			.collect();

		scored.sort_by(|a, b| b.0.cmp(&a.0));
		scored.truncate(max_results);

		scored
			.into_iter()
			.map(|(_, path)| Pair {
				display: path.to_string(),
				replacement: path.to_string(),
			})
			.collect()
	}

	fn get_all_files() -> Vec<String> {
		let mut cache = FILE_CACHE.lock().expect("file cache lock");
		if let Some(files) = cache.as_ref() {
			return files.clone();
		}

		let output = Command::new("rg").args(["--files", "--hidden"]).output();

		let files = match output {
			Ok(output) if output.status.success() => {
				let stdout = String::from_utf8_lossy(&output.stdout);
				stdout
					.lines()
					.map(|line| line.trim().to_string())
					.filter(|line| !line.is_empty())
					.collect()
			}
			_ => Vec::new(),
		};

		*cache = Some(files.clone());
		files
	}

	/// Custom file completion that handles absolute paths and tilde expansion
	fn complete_file_path(file_part: &str) -> Vec<Pair> {
		if file_part.is_empty() {
			// Show current directory contents
			return Self::list_directory_contents(".");
		}

		// Expand tilde if present
		let expanded_path = Self::expand_tilde(file_part);
		let expanded_str = expanded_path.to_string_lossy();

		// If the path ends with a separator, list contents of that directory
		if file_part.ends_with('/') || file_part.ends_with('\\') {
			return Self::list_directory_contents(&expanded_str);
		}

		// Determine the parent directory and filename part
		let (parent_dir, filename_part) = if let Some(parent) = expanded_path.parent() {
			let parent_str = parent.to_str().unwrap_or(".");
			// If parent is empty string (for relative paths without ./), use current directory
			let actual_parent = if parent_str.is_empty() {
				"."
			} else {
				parent_str
			};

			let filename_part = expanded_path
				.file_name()
				.and_then(|n| n.to_str())
				.unwrap_or("");

			(actual_parent, filename_part)
		} else {
			// No parent found, treat as current directory search
			(".", file_part)
		};

		let mut candidates = Self::list_directory_contents(parent_dir);

		// Filter candidates that start with the filename part
		if !filename_part.is_empty() {
			let filename_lower = filename_part.to_lowercase();
			candidates.retain(|candidate| {
				let name = Path::new(&candidate.replacement)
					.file_name()
					.and_then(|n| n.to_str())
					.unwrap_or("")
					.to_lowercase();
				name.starts_with(&filename_lower)
			});
		}

		// Adjust the replacement paths to be relative to the original input
		for candidate in &mut candidates {
			if file_part.starts_with("~/") {
				// Convert back to tilde notation
				if let Some(home) = dirs::home_dir() {
					if let Ok(relative) = PathBuf::from(&candidate.replacement).strip_prefix(&home)
					{
						candidate.replacement = format!("~/{}", relative.to_string_lossy());
					}
				}
			} else if file_part.starts_with('/') {
				// Keep absolute path
				// candidate.replacement is already correct
			} else {
				// For relative paths, make sure we maintain the relative nature
				if let Ok(relative) = PathBuf::from(&candidate.replacement)
					.strip_prefix(std::env::current_dir().unwrap_or_default())
				{
					candidate.replacement = relative.to_string_lossy().to_string();
				}
			}
		}

		candidates
	}

	/// List contents of a directory, returning both directories and image files
	fn list_directory_contents(dir_path: &str) -> Vec<Pair> {
		let mut candidates = Vec::new();

		if let Ok(entries) = fs::read_dir(dir_path) {
			for entry in entries.flatten() {
				let path = entry.path();
				let path_str = path.to_string_lossy().to_string();

				if path.is_dir() {
					// Add directory with trailing slash
					let display = format!(
						"{}/",
						path.file_name().and_then(|n| n.to_str()).unwrap_or("")
					);
					candidates.push(Pair {
						display: display.clone(),
						replacement: format!("{}/", path_str),
					});
				} else if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
					if Self::is_image_file(filename) {
						candidates.push(Pair {
							display: filename.to_string(),
							replacement: path_str,
						});
					}
				}
			}
		}

		// Sort: directories first, then files
		candidates.sort_by(|a, b| {
			let a_is_dir = a.replacement.ends_with('/');
			let b_is_dir = b.replacement.ends_with('/');

			match (a_is_dir, b_is_dir) {
				(true, false) => std::cmp::Ordering::Less,
				(false, true) => std::cmp::Ordering::Greater,
				_ => a.replacement.cmp(&b.replacement),
			}
		});

		candidates
	}

	/// Filter and prepare completion candidates for better UX with bash-like completion
	fn filter_and_limit_candidates(candidates: Vec<Pair>, file_part: &str) -> Vec<Pair> {
		// For bash-like completion, find common prefix for partial completion
		if candidates.is_empty() {
			return candidates;
		}

		// If there's a common prefix longer than current input, add it as first candidate
		let common_prefix = Self::find_common_prefix(&candidates);
		let mut result = candidates;

		// If common prefix is longer than current input, add it for partial completion
		if common_prefix.len() > file_part.len() {
			let partial_completion = Pair {
				display: format!("{} (partial)", common_prefix),
				replacement: common_prefix,
			};
			result.insert(0, partial_completion);
		}

		// Limit total candidates for better UX
		const MAX_TOTAL: usize = 10; // More options for list mode
		result.truncate(MAX_TOTAL);
		result
	}

	/// Find common prefix among all candidates for partial completion
	fn find_common_prefix(candidates: &[Pair]) -> String {
		if candidates.is_empty() {
			return String::new();
		}

		let first = &candidates[0].replacement;
		let mut common_len = first.len();

		for candidate in candidates.iter().skip(1) {
			let replacement = &candidate.replacement;
			let mut len = 0;
			for (a, b) in first.chars().zip(replacement.chars()) {
				if a == b {
					len += a.len_utf8();
				} else {
					break;
				}
			}
			common_len = common_len.min(len);
		}

		first[..common_len].to_string()
	}
}

impl<'a> CommandCompleter<'a> {
	pub(crate) fn complete(&self, line: &str, pos: usize) -> (usize, Vec<Pair>) {
		if let Some((start, query)) = Self::find_at_query(line, pos) {
			let candidates = Self::fuzzy_match_files(query, 10);
			return (start, candidates);
		}

		// Handle /image command with file completion
		if line.starts_with("/image ") {
			let image_prefix = "/image ";
			let prefix_len = image_prefix.len();

			// Extract the file path part up to cursor position
			let file_part = if pos > prefix_len {
				&line[prefix_len..pos]
			} else {
				""
			};

			// Use our custom file completion that handles absolute paths and tilde expansion
			let candidates = Self::complete_file_path(file_part);
			let filtered_candidates = Self::filter_and_limit_candidates(candidates, file_part);

			// For file completion, we want to replace from the start of the file part
			(prefix_len, filtered_candidates)
		} else if line.starts_with("/prompt ") {
			// Handle /prompt command with template name completion
			let prompt_prefix = "/prompt ";
			let prefix_len = prompt_prefix.len();

			// Extract the template name part up to cursor position
			let template_part = if pos > prefix_len {
				&line[prefix_len..pos]
			} else {
				""
			};

			// Get available prompt templates from config
			let candidates: Vec<Pair> = self
				.config
				.prompts
				.iter()
				.filter(|prompt| prompt.name.starts_with(template_part))
				.map(|prompt| Pair {
					display: if let Some(ref description) = prompt.description {
						format!("{} - {}", prompt.name, description)
					} else {
						prompt.name.clone()
					},
					replacement: prompt.name.clone(),
				})
				.collect();

			(prefix_len, candidates)
		} else if line.starts_with("/run ") {
			// Handle /run command with command name completion
			let run_prefix = "/run ";
			let prefix_len = run_prefix.len();

			// Extract the command name part up to cursor position
			let command_part = if pos > prefix_len {
				&line[prefix_len..pos]
			} else {
				""
			};

			// Get available commands for this role
			let available_commands =
				crate::session::chat::list_available_commands(self.config, self.role);
			let candidates: Vec<Pair> = available_commands
				.iter()
				.filter(|cmd| cmd.starts_with(command_part))
				.map(|cmd| Pair {
					display: cmd.clone(),
					replacement: cmd.clone(),
				})
				.collect();

			(prefix_len, candidates)
		} else if line.starts_with("/workflow ") {
			// Handle /workflow command with workflow name completion
			let workflow_prefix = "/workflow ";
			let prefix_len = workflow_prefix.len();

			// Extract the workflow name part up to cursor position
			let workflow_part = if pos > prefix_len {
				&line[prefix_len..pos]
			} else {
				""
			};

			// Get available workflows
			let candidates: Vec<Pair> = self
				.get_available_workflows()
				.iter()
				.filter(|workflow| workflow.starts_with(workflow_part))
				.map(|workflow| Pair {
					display: workflow.clone(),
					replacement: workflow.clone(),
				})
				.collect();

			(prefix_len, candidates)
		} else if line.starts_with("/context ") {
			// Handle /context command with filter completion
			let context_prefix = "/context ";
			let prefix_len = context_prefix.len();

			// Extract the filter part up to cursor position
			let filter_part = if pos > prefix_len {
				&line[prefix_len..pos]
			} else {
				""
			};

			let candidates: Vec<Pair> = Self::get_context_filters()
				.iter()
				.filter(|filter| filter.starts_with(filter_part))
				.map(|filter| Pair {
					display: filter.to_string(),
					replacement: filter.to_string(),
				})
				.collect();

			(prefix_len, candidates)
		} else if line.starts_with("/mcp ") {
			// Handle /mcp command with subcommand completion
			let mcp_prefix = "/mcp ";
			let prefix_len = mcp_prefix.len();

			// Extract the subcommand part up to cursor position
			let subcommand_part = if pos > prefix_len {
				&line[prefix_len..pos]
			} else {
				""
			};

			let candidates: Vec<Pair> = Self::get_mcp_subcommands()
				.iter()
				.filter(|subcommand| subcommand.starts_with(subcommand_part))
				.map(|subcommand| Pair {
					display: subcommand.to_string(),
					replacement: subcommand.to_string(),
				})
				.collect();

			(prefix_len, candidates)
		} else if line.starts_with("/cache ") {
			// Handle /cache command with subcommand completion
			let cache_prefix = "/cache ";
			let prefix_len = cache_prefix.len();

			// Extract the subcommand part up to cursor position
			let subcommand_part = if pos > prefix_len {
				&line[prefix_len..pos]
			} else {
				""
			};

			let candidates: Vec<Pair> = Self::get_cache_subcommands()
				.iter()
				.filter(|subcommand| subcommand.starts_with(subcommand_part))
				.map(|subcommand| Pair {
					display: subcommand.to_string(),
					replacement: subcommand.to_string(),
				})
				.collect();

			(prefix_len, candidates)
		} else if line.starts_with("/loglevel ") {
			// Handle /loglevel command with level completion
			let loglevel_prefix = "/loglevel ";
			let prefix_len = loglevel_prefix.len();

			// Extract the level part up to cursor position
			let level_part = if pos > prefix_len {
				&line[prefix_len..pos]
			} else {
				""
			};

			let candidates: Vec<Pair> = Self::get_log_levels()
				.iter()
				.filter(|level| level.starts_with(level_part))
				.map(|level| Pair {
					display: level.to_string(),
					replacement: level.to_string(),
				})
				.collect();

			(prefix_len, candidates)
		} else if line.starts_with("/role ") {
			// Handle /role command with role name completion
			let role_prefix = "/role ";
			let prefix_len = role_prefix.len();

			// Extract the role part up to cursor position
			let role_part = if pos > prefix_len {
				&line[prefix_len..pos]
			} else {
				""
			};

			let candidates: Vec<Pair> = self
				.get_available_roles()
				.iter()
				.filter(|role| role.starts_with(role_part))
				.map(|role| Pair {
					display: role.clone(),
					replacement: role.clone(),
				})
				.collect();

			(prefix_len, candidates)
		} else if !line.starts_with('/') {
			// No completion for non-commands
			(0, vec![])
		} else {
			// Handle regular command completion with cursor position awareness
			let command_part = &line[..pos.min(line.len())];
			let candidates: Vec<Pair> = self
				.commands
				.iter()
				.filter(|cmd| cmd.starts_with(command_part))
				.map(|cmd| Pair {
					display: cmd.clone(),
					replacement: cmd.clone(),
				})
				.collect();

			// Find common prefix for partial completion
			let common_prefix = Self::find_common_prefix(&candidates);
			let mut result = candidates;

			// If there's a longer common prefix and multiple candidates, add it as first option
			if common_prefix.len() > command_part.len() && result.len() > 1 {
				let partial = Pair {
					display: format!("{} (partial)", common_prefix),
					replacement: common_prefix,
				};
				result.insert(0, partial);
			}

			(0, result)
		}
	}
}

impl<'a> CommandCompleter<'a> {
	pub(crate) fn hint(&self, line: &str) -> Option<String> {
		if line.is_empty() || !line.starts_with('/') {
			return None;
		}

		// Special hint for /image command
		if line == "/image" {
			return Some(" <path_to_image>".to_string());
		}

		// Special hint for /prompt command
		if line == "/prompt" {
			return Some(" <template_name>".to_string());
		}

		// Special hint for /run command
		if line == "/run" {
			return Some(" <command_name>".to_string());
		}

		// Special hint for /workflow command
		if line == "/workflow" {
			return Some(" <workflow_name>".to_string());
		}

		// Special hint for /context command
		if line == "/context" {
			return Some(" [all|assistant|user|tool|large]".to_string());
		}

		// Special hint for /mcp command
		if line == "/mcp" {
			return Some(" [list|info|full|health|dump|validate]".to_string());
		}

		// Special hint for /cache command
		if line == "/cache" {
			return Some(" [stats|clear|threshold]".to_string());
		}

		// Special hint for /loglevel command
		if line == "/loglevel" {
			return Some(" [none|info|debug]".to_string());
		}

		// Special hint for /role command
		if line == "/role" {
			return Some(" <role_name>".to_string());
		}

		// Special hint for /model command
		if line == "/model" {
			return Some(" <model_name>".to_string());
		}

		if line.starts_with("/image ") && line.len() > 7 {
			let file_part = &line[7..]; // "/image ".len() = 7
			if file_part.is_empty() {
				return Some("Start typing image file path...".to_string());
			}
			return None; // Let filename completer handle this
		}

		if line.starts_with("/prompt ") && line.len() > 8 {
			let template_part = &line[8..]; // "/prompt ".len() = 8
			if template_part.is_empty() {
				return Some("Start typing prompt template name...".to_string());
			}
			return None; // Let template completer handle this
		}

		if line.starts_with("/run ") && line.len() > 5 {
			let command_part = &line[5..]; // "/run ".len() = 5
			if command_part.is_empty() {
				return Some("Start typing command name...".to_string());
			}
			return None; // Let command completer handle this
		}

		if line.starts_with("/workflow ") && line.len() > 10 {
			let workflow_part = &line[10..]; // "/workflow ".len() = 10
			if workflow_part.is_empty() {
				return Some("Start typing workflow name...".to_string());
			}
			return None; // Let workflow completer handle this
		}

		if line.starts_with("/context ") && line.len() > 9 {
			let filter_part = &line[9..]; // "/context ".len() = 9
			if filter_part.is_empty() {
				return Some("all|assistant|user|tool|large".to_string());
			}
			return None; // Let completer handle this
		}

		if line.starts_with("/mcp ") && line.len() > 5 {
			let subcommand_part = &line[5..]; // "/mcp ".len() = 5
			if subcommand_part.is_empty() {
				return Some("list|info|full|health|dump|validate".to_string());
			}
			return None; // Let completer handle this
		}

		if line.starts_with("/cache ") && line.len() > 7 {
			let subcommand_part = &line[7..]; // "/cache ".len() = 7
			if subcommand_part.is_empty() {
				return Some("stats|clear|threshold".to_string());
			}
			return None; // Let completer handle this
		}

		if line.starts_with("/loglevel ") && line.len() > 10 {
			let level_part = &line[10..]; // "/loglevel ".len() = 10
			if level_part.is_empty() {
				return Some("none|info|debug".to_string());
			}
			return None; // Let completer handle this
		}

		if line.starts_with("/role ") && line.len() > 6 {
			let role_part = &line[6..]; // "/role ".len() = 6
			if role_part.is_empty() {
				let roles = self.get_available_roles();
				if !roles.is_empty() {
					return Some(roles.join("|"));
				}
				return Some("Start typing role name...".to_string());
			}
			return None; // Let completer handle this
		}

		if line.starts_with("/model ") && line.len() > 7 {
			let model_part = &line[7..]; // "/model ".len() = 7
			if model_part.is_empty() {
				return Some("Start typing model name...".to_string());
			}
			return None; // Let completer handle this
		}

		// Look for a command that starts with the current input
		self.commands
			.iter()
			.find(|cmd| cmd.starts_with(line))
			.map(|cmd| cmd[line.len()..].to_string())
	}
}
