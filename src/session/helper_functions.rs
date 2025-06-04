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

use crate::session::project_context::ProjectContext;
use crate::session::Session;
use chrono::{DateTime, Local};
use futures::future::join_all;
use std::collections::HashMap;
use std::env;
use std::path::Path;
use tokio::process::Command;

// System prompts are now fully controlled by configuration files
// All hardcoded prompts have been moved to the config template

// Function to process placeholders in a system prompt (synchronous version for backward compatibility)
pub fn process_placeholders(prompt: &str, project_dir: &Path) -> String {
	let mut processed_prompt = prompt.to_string();

	// Check which placeholders are actually in the prompt to avoid unnecessary work
	let needs_cwd = prompt.contains("%{CWD}");
	let needs_context = prompt.contains("%{CONTEXT}");
	let needs_git_status = prompt.contains("%{GIT_STATUS}");
	let needs_git_tree = prompt.contains("%{GIT_TREE}");
	let needs_readme = prompt.contains("%{README}");

	// Early return if no supported placeholders are found (async placeholders are not supported in sync version)
	if !needs_cwd && !needs_context && !needs_git_status && !needs_git_tree && !needs_readme {
		return processed_prompt;
	}

	// Create a map of placeholder values (without async system info for backward compatibility)
	let mut placeholders = HashMap::new();

	// Collect project context only if needed
	let project_context = if needs_context || needs_git_status || needs_git_tree || needs_readme {
		Some(ProjectContext::collect(project_dir))
	} else {
		None
	};

	// Add CWD if needed
	if needs_cwd {
		placeholders.insert("%{CWD}", project_dir.to_string_lossy().to_string());
	}

	// Add project context placeholders only if needed
	if let Some(ref context) = project_context {
		if needs_context {
			let context_info = context.format_for_prompt();
			let context_section = if !context_info.is_empty() {
				format!(
					"\n\n==== PROJECT CONTEXT ====\n\n{}\n\n==== END PROJECT CONTEXT ====\n",
					context_info
				)
			} else {
				String::new()
			};
			placeholders.insert("%{CONTEXT}", context_section);
		}

		if needs_git_status {
			let git_status = if let Some(ref git_status) = context.git_status {
				format!(
					"\n\n==== GIT STATUS ====\n\n{}\n\n==== END GIT STATUS ====\n",
					git_status
				)
			} else {
				String::new()
			};
			placeholders.insert("%{GIT_STATUS}", git_status);
		}

		if needs_git_tree {
			let git_tree = if let Some(ref file_tree) = context.file_tree {
				format!(
					"\n\n==== FILE TREE ====\n\n{}\n\n==== END FILE TREE ====\n",
					file_tree
				)
			} else {
				String::new()
			};
			placeholders.insert("%{GIT_TREE}", git_tree);
		}

		if needs_readme {
			let readme = if let Some(ref readme) = context.readme_content {
				format!(
					"\n\n==== README ====\n\n{}\n\n==== END README ====\n",
					readme
				)
			} else {
				String::new()
			};
			placeholders.insert("%{README}", readme);
		}
	}

	// Replace all placeholders
	for (placeholder, value) in placeholders.iter() {
		processed_prompt = processed_prompt.replace(placeholder, value);
	}

	processed_prompt
}

// Function to get summarized context for layers using the Summary InputMode
pub fn summarize_context(session: &Session, input: &str) -> String {
	// This is a placeholder. In practice, you'd want to analyze the session history
	// and create a summary of the important points rather than just concatenating everything.
	let history = session
		.messages
		.iter()
		.filter(|m| m.role == "assistant")
		.map(|m| m.content.clone())
		.collect::<Vec<_>>()
		.join("\n\n");

	format!("Current input: {}\n\nSummary of previous context: {}\n\nPlease generate a concise summary of the above context.",
		input,
		if history.len() > 2000 {
			format!("{} (truncated)...", &history[..2000])
		} else {
			history
		}
	)
}

#[derive(Debug, Default)]
pub struct SystemInfo {
	pub date_with_timezone: String,
	pub shell_info: String,
	pub os_info: String,
	pub binaries: String,
}

// Async function to get the version of a command
async fn get_command_version(command: &str) -> String {
	let version_flags = match command {
		"bash" => vec!["--version"],
		"awk" => vec!["--version"],
		"rg" | "ripgrep" => vec!["--version"],
		"rustc" => vec!["--version"],
		"php" => vec!["--version"],
		"node" => vec!["--version"],
		"npm" => vec!["--version"],
		"python" | "python3" => vec!["--version"],
		"go" => vec!["version"],
		"java" => vec!["-version"],
		"gcc" => vec!["--version"],
		"clang" => vec!["--version"],
		"git" => vec!["--version"],
		"gh" => vec!["--version"],
		"docker" => vec!["--version"],
		"make" => vec!["--version"],
		"curl" => vec!["--version"],
		"wget" => vec!["--version"],
		"tar" => vec!["--version"],
		"zip" => vec!["--version"],
		"unzip" => vec!["--version"],
		_ => vec!["--version"],
	};

	// First, try to get version information
	for flag in version_flags {
		match Command::new(command).arg(flag).output().await {
			Ok(output) => {
				if output.status.success() {
					let stdout = String::from_utf8_lossy(&output.stdout);
					let stderr = String::from_utf8_lossy(&output.stderr);
					let version_output = if !stdout.trim().is_empty() {
						stdout.trim()
					} else {
						stderr.trim()
					};

					// Extract just the version number/info from the first line
					let first_line = version_output.lines().next().unwrap_or("").trim();
					if !first_line.is_empty() {
						return first_line.to_string();
					}
				}
			}
			Err(_) => continue,
		}
	}

	// If version detection failed, check if the command exists at all
	// by trying to run it with no arguments or a help flag
	let existence_checks = vec![
		vec!["--help"],
		vec!["-h"],
		vec![], // Some commands show help when run with no args
	];

	for args in existence_checks {
		match Command::new(command).args(&args).output().await {
			Ok(_) => {
				// If command runs (regardless of exit code), it exists
				return "version unknown".to_string();
			}
			Err(_) => continue,
		}
	}

	"missing".to_string()
}

// Async function to gather all system information
pub async fn gather_system_info() -> SystemInfo {
	let mut info = SystemInfo::default();

	// Get current date with timezone
	let now: DateTime<Local> = Local::now();
	info.date_with_timezone = now.format("%Y-%m-%d %H:%M:%S %Z").to_string();

	// Get shell information
	let shell_path = env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
	let shell_name = shell_path.split('/').next_back().unwrap_or("unknown");

	// Try to get shell version
	let shell_version = get_command_version(shell_name).await;
	info.shell_info = if shell_version != "missing" {
		format!("{} ({})", shell_name, shell_version)
	} else {
		shell_name.to_string()
	};

	// Get OS information
	info.os_info = get_os_info().await;

	// Get shell binaries versions in parallel
	let commands = vec![
		"awk", "sed", "rg", "rustc", "php", "node", "npm", "python3", "python", "go", "java",
		"gcc", "clang", "git", "docker", "make", "curl", "wget", "tar", "zip", "unzip",
	];

	let version_futures: Vec<_> = commands
		.iter()
		.map(|&cmd| async move {
			let version = get_command_version(cmd).await;
			(cmd, version)
		})
		.collect();

	let versions = join_all(version_futures).await;

	// Format binaries info - one line per binary
	let mut binaries = Vec::new();
	for (cmd, version) in versions {
		if version != "missing" {
			binaries.push(format!("{}: {}", cmd, version));
		} else {
			binaries.push(format!("{}: missing", cmd));
		}
	}

	info.binaries = binaries.join("\n");

	info
}

// Function to get detailed OS information
async fn get_os_info() -> String {
	let mut os_parts = Vec::new();

	// Get basic OS info

	os_parts.push(format!("os: {}", env::consts::OS));
	os_parts.push(format!("arch: {}", env::consts::ARCH));
	os_parts.push(format!("family: {}", env::consts::FAMILY));

	// Try to get more detailed system information
	if cfg!(target_os = "macos") {
		if let Ok(output) = Command::new("sw_vers").output().await {
			if output.status.success() {
				let sw_vers = String::from_utf8_lossy(&output.stdout);
				let mut version_info = Vec::new();
				for line in sw_vers.lines() {
					if let Some((key, value)) = line.split_once(':') {
						let key = key
							.trim()
							.replace("ProductName", "name")
							.replace("ProductVersion", "version")
							.replace("BuildVersion", "build");
						version_info.push(format!("{}: {}", key, value.trim()));
					}
				}
				if !version_info.is_empty() {
					os_parts.extend(version_info);
				}
			}
		}
	} else if cfg!(target_os = "linux") {
		// Try to get Linux distribution info
		if let Ok(output) = Command::new("lsb_release").args(["-a"]).output().await {
			if output.status.success() {
				let lsb_info = String::from_utf8_lossy(&output.stdout);
				for line in lsb_info.lines() {
					if line.contains("Description:") {
						if let Some(desc) = line.split_once(':') {
							os_parts.push(format!("distribution: {}", desc.1.trim()));
						}
					}
				}
			}
		}

		// Try /etc/os-release as fallback
		if let Ok(output) = Command::new("cat").arg("/etc/os-release").output().await {
			if output.status.success() {
				let os_release = String::from_utf8_lossy(&output.stdout);
				for line in os_release.lines() {
					if line.starts_with("PRETTY_NAME=") {
						let name = line
							.replace("PRETTY_NAME=", "")
							.trim_matches('"')
							.to_string();
						os_parts.push(format!("distribution: {}", name));
						break;
					}
				}
			}
		}
	} else if cfg!(target_os = "windows") {
		if let Ok(output) = Command::new("wmic")
			.args(["os", "get", "Caption,Version", "/format:list"])
			.output()
			.await
		{
			if output.status.success() {
				let wmic_info = String::from_utf8_lossy(&output.stdout);
				for line in wmic_info.lines() {
					if let Some((key, value)) = line.split_once('=') {
						if !value.trim().is_empty() {
							let key = key.trim().to_lowercase();
							os_parts.push(format!("{}: {}", key, value.trim()));
						}
					}
				}
			}
		}
	}

	// Get kernel version if available
	if let Ok(output) = Command::new("uname").args(["-r"]).output().await {
		if output.status.success() {
			let kernel = String::from_utf8_lossy(&output.stdout).trim().to_string();
			os_parts.push(format!("kernel: {}", kernel));
		}
	}

	os_parts.join(", ")
}

// Smart async version of process_placeholders - only gathers data for placeholders that exist in the prompt
pub async fn process_placeholders_async(prompt: &str, project_dir: &Path) -> String {
	let mut processed_prompt = prompt.to_string();

	// Check which placeholders are actually in the prompt to avoid unnecessary work
	let needs_date = prompt.contains("%{DATE}");
	let needs_shell = prompt.contains("%{SHELL}");
	let needs_os = prompt.contains("%{OS}");
	let needs_binaries = prompt.contains("%{BINARIES}");
	let needs_cwd = prompt.contains("%{CWD}");
	let needs_system = prompt.contains("%{SYSTEM}"); // System info: date, shell, OS, binaries, CWD
	let needs_context = prompt.contains("%{CONTEXT}"); // Project info: README, git status, git tree
	let needs_git_status = prompt.contains("%{GIT_STATUS}");
	let needs_git_tree = prompt.contains("%{GIT_TREE}");
	let needs_readme = prompt.contains("%{README}");

	// Early return if no placeholders are found
	if !needs_date
		&& !needs_shell
		&& !needs_os
		&& !needs_binaries
		&& !needs_cwd
		&& !needs_system
		&& !needs_context
		&& !needs_git_status
		&& !needs_git_tree
		&& !needs_readme
	{
		return processed_prompt;
	}

	// Create a map of placeholder values
	let mut placeholders = HashMap::new();

	// Collect system information only if needed
	let system_info = if needs_date || needs_shell || needs_os || needs_binaries || needs_system {
		Some(gather_system_info().await)
	} else {
		None
	};

	// Collect project context only if needed
	let project_context = if needs_context || needs_git_status || needs_git_tree || needs_readme {
		Some(ProjectContext::collect(project_dir))
	} else {
		None
	};

	// Add system info placeholders only if needed
	if let Some(ref info) = system_info {
		if needs_date {
			placeholders.insert("%{DATE}", info.date_with_timezone.clone());
		}
		if needs_shell {
			placeholders.insert("%{SHELL}", info.shell_info.clone());
		}
		if needs_os {
			placeholders.insert("%{OS}", info.os_info.clone());
		}
		if needs_binaries {
			placeholders.insert("%{BINARIES}", info.binaries.clone());
		}
		if needs_system {
			// Build comprehensive system information
			let mut system_context = String::new();
			system_context.push_str("# System Information\n\n");
			system_context.push_str(&format!("**Date**: {}\n", info.date_with_timezone));
			system_context.push_str(&format!("**Shell**: {}\n", info.shell_info));
			system_context.push_str(&format!("**Operating System**: {}\n", info.os_info));
			system_context.push_str(&format!(
				"**Current Directory**: {}\n",
				project_dir.to_string_lossy()
			));
			system_context.push_str("\n## Available Development Tools\n\n");
			system_context.push_str(&info.binaries);

			let system_section = format!(
				"\n\n==== SYSTEM INFORMATION ====\n\n{}\n\n==== END SYSTEM INFORMATION ====\n",
				system_context
			);
			placeholders.insert("%{SYSTEM}", system_section);
		}
	}

	// Add CWD if needed
	if needs_cwd {
		placeholders.insert("%{CWD}", project_dir.to_string_lossy().to_string());
	}

	// Add project context placeholders only if needed
	if let Some(ref context) = project_context {
		if needs_context {
			let context_info = context.format_for_prompt();

			// Build project context (README, git status, git tree)
			let context_section = if !context_info.is_empty() {
				format!(
					"\n\n==== PROJECT CONTEXT ====\n\n{}\n\n==== END PROJECT CONTEXT ====\n",
					context_info
				)
			} else {
				String::new()
			};
			placeholders.insert("%{CONTEXT}", context_section);
		}

		if needs_git_status {
			let git_status = if let Some(ref git_status) = context.git_status {
				format!(
					"\n\n==== GIT STATUS ====\n\n{}\n\n==== END GIT STATUS ====\n",
					git_status
				)
			} else {
				String::new()
			};
			placeholders.insert("%{GIT_STATUS}", git_status);
		}

		if needs_git_tree {
			let git_tree = if let Some(ref file_tree) = context.file_tree {
				format!(
					"\n\n==== FILE TREE ====\n\n{}\n\n==== END FILE TREE ====\n",
					file_tree
				)
			} else {
				String::new()
			};
			placeholders.insert("%{GIT_TREE}", git_tree);
		}

		if needs_readme {
			let readme = if let Some(ref readme) = context.readme_content {
				format!(
					"\n\n==== README ====\n\n{}\n\n==== END README ====\n",
					readme
				)
			} else {
				String::new()
			};
			placeholders.insert("%{README}", readme);
		}
	}

	// Replace all placeholders
	for (placeholder, value) in placeholders.iter() {
		processed_prompt = processed_prompt.replace(placeholder, value);
	}

	processed_prompt
}

// Function to get all available placeholders with their current values
pub async fn get_all_placeholders(project_dir: &Path) -> HashMap<String, String> {
	let mut placeholders = HashMap::new();

	// Collect context information
	let project_context = ProjectContext::collect(project_dir);

	// Gather system information asynchronously
	let system_info = gather_system_info().await;

	// Build system information section
	let mut system_context = String::new();
	system_context.push_str("# System Information\n\n");
	system_context.push_str(&format!("**Date**: {}\n", system_info.date_with_timezone));
	system_context.push_str(&format!("**Shell**: {}\n", system_info.shell_info));
	system_context.push_str(&format!("**Operating System**: {}\n", system_info.os_info));
	system_context.push_str(&format!(
		"**Current Directory**: {}\n",
		project_dir.to_string_lossy()
	));
	system_context.push_str("\n## Available Development Tools\n\n");
	system_context.push_str(&system_info.binaries);

	let system_section = format!(
		"\n\n==== SYSTEM INFORMATION ====\n\n{}\n\n==== END SYSTEM INFORMATION ====\n",
		system_context
	);

	// Build project context section (README, git status, git tree)
	let context_info = project_context.format_for_prompt();
	let context_section = if !context_info.is_empty() {
		format!(
			"\n\n==== PROJECT CONTEXT ====\n\n{}\n\n==== END PROJECT CONTEXT ====\n",
			context_info
		)
	} else {
		String::new()
	};

	// Add all placeholders
	placeholders.insert("%{SYSTEM}".to_string(), system_section); // System info: date, shell, OS, binaries, CWD
	placeholders.insert("%{CONTEXT}".to_string(), context_section); // Project info: README, git status, git tree
	placeholders.insert(
		"%{CWD}".to_string(),
		project_dir.to_string_lossy().to_string(),
	);
	placeholders.insert("%{DATE}".to_string(), system_info.date_with_timezone);
	placeholders.insert("%{SHELL}".to_string(), system_info.shell_info);
	placeholders.insert("%{OS}".to_string(), system_info.os_info);
	placeholders.insert("%{BINARIES}".to_string(), system_info.binaries);

	// Add specific parts of the context as individual placeholders
	placeholders.insert(
		"%{GIT_STATUS}".to_string(),
		if let Some(git_status) = &project_context.git_status {
			format!(
				"\n\n==== GIT STATUS ====\n\n{}\n\n==== END GIT STATUS ====\n",
				git_status
			)
		} else {
			String::new()
		},
	);

	placeholders.insert(
		"%{GIT_TREE}".to_string(),
		if let Some(file_tree) = &project_context.file_tree {
			format!(
				"\n\n==== FILE TREE ====\n\n{}\n\n==== END FILE TREE ====\n",
				file_tree
			)
		} else {
			String::new()
		},
	);

	placeholders.insert(
		"%{README}".to_string(),
		if let Some(readme) = &project_context.readme_content {
			format!(
				"\n\n==== README ====\n\n{}\n\n==== END README ====\n",
				readme
			)
		} else {
			String::new()
		},
	);

	placeholders
}
