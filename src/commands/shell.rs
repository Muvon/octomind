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

use anyhow::Result;
use clap::Args;
use octomind::config::Config;
use octomind::session::{chat_completion_with_provider, Message};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{self, Read, Write};

// Function to add command to shell history
fn add_to_shell_history(command: &str) -> Result<()> {
	// Get the shell and history file path
	let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
	let home = std::env::var("HOME")?;

	// Try to get HISTFILE environment variable first, fallback to default locations
	let history_file = if let Ok(histfile) = std::env::var("HISTFILE") {
		histfile
	} else if shell.contains("zsh") {
		format!("{}/.zsh_history", home)
	} else if shell.contains("bash") {
		format!("{}/.bash_history", home)
	} else if shell.contains("fish") {
		format!("{}/.local/share/fish/fish_history", home)
	} else {
		// Default to bash history
		format!("{}/.bash_history", home)
	};

	// For zsh, we need to add timestamp and format correctly
	let history_entry = if shell.contains("zsh") {
		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();
		format!(": {}:0;{}\n", timestamp, command)
	} else if shell.contains("fish") {
		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();
		format!("- cmd: {}\n  when: {}\n", command, timestamp)
	} else {
		// Bash format
		format!("{}\n", command)
	};

	// Append to history file
	match OpenOptions::new()
		.create(true)
		.append(true)
		.open(&history_file)
	{
		Ok(mut file) => {
			let _ = file.write_all(history_entry.as_bytes());
			let _ = file.flush();
		}
		Err(_) => {
			// If we can't write to history file, just continue silently
			// This prevents the tool from failing if history file is not writable
		}
	}

	Ok(())
}

#[derive(Args, Debug)]
pub struct ShellArgs {
	/// Description of the shell command you want to execute
	#[arg(value_name = "DESCRIPTION")]
	pub description: Option<String>,

	/// Use a specific model instead of the one configured in config (runtime only, not saved)
	#[arg(long)]
	pub model: Option<String>,

	/// Skip confirmation and execute command directly
	#[arg(long, short)]
	pub yes: bool,

	/// Temperature for the AI response (0.0 to 1.0, runtime only, not saved)
	#[arg(long, default_value = "0.3")]
	pub temperature: f32,
}

#[derive(Serialize, Deserialize, Debug)]
struct ShellResponse {
	command: String,
	explanation: String,
	safety_notes: Option<String>,
}

pub async fn execute(args: &ShellArgs, config: &Config) -> Result<()> {
	// Get input from argument or stdin
	let description = if let Some(desc) = &args.description {
		desc.clone()
	} else {
		// Read from stdin
		let mut buffer = String::new();
		io::stdin().read_to_string(&mut buffer)?;
		buffer.trim().to_string()
	};

	if description.is_empty() {
		eprintln!("Error: No description provided. Use argument or pipe description to stdin.");
		std::process::exit(1);
	}

	// Determine model to use: either from --model flag or effective config model
	let model = args
		.model
		.clone()
		.unwrap_or_else(|| config.get_effective_model());

	// Create specialized system prompt for shell commands
	let system_prompt = create_shell_system_prompt();

	// Create user prompt that asks for structured response
	let user_prompt = format!(
		"Generate a shell command for: {}\n\n\
			Please respond with a JSON object containing:\n\
			- \"command\": the exact shell command to execute\n\
			- \"explanation\": brief explanation of what the command does\n\
			- \"safety_notes\": optional warnings if the command is potentially dangerous\n\n\
			Only respond with the JSON object, no other text.",
		description
	);

	// Create messages
	let messages = vec![
		Message {
			role: "system".to_string(),
			content: system_prompt,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
		},
		Message {
			role: "user".to_string(),
			content: user_prompt,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
		},
	];

	// Call the AI provider
	let response =
		chat_completion_with_provider(&messages, &model, args.temperature, config).await?;

	// Parse the JSON response
	let shell_response: ShellResponse = match serde_json::from_str(&response.content) {
		Ok(resp) => resp,
		Err(_) => {
			// If JSON parsing fails, try to extract command from markdown code blocks
			let content = response.content.trim();
			if let Some(json_start) = content.find('{') {
				if let Some(json_end) = content.rfind('}') {
					let json_part = &content[json_start..=json_end];
					match serde_json::from_str::<ShellResponse>(json_part) {
						Ok(resp) => resp,
						Err(_) => {
							eprintln!("Error: Could not parse AI response as structured command.");
							eprintln!("Raw response: {}", response.content);
							std::process::exit(1);
						}
					}
				} else {
					eprintln!("Error: Could not parse AI response as structured command.");
					eprintln!("Raw response: {}", response.content);
					std::process::exit(1);
				}
			} else {
				eprintln!("Error: Could not parse AI response as structured command.");
				eprintln!("Raw response: {}", response.content);
				std::process::exit(1);
			}
		}
	};

	// Display the command and explanation
	println!("📝 Command: {}", shell_response.command);
	println!("💡 Explanation: {}", shell_response.explanation);

	if let Some(safety_notes) = &shell_response.safety_notes {
		use colored::*;
		println!("⚠️  Safety notes: {}", safety_notes.yellow());
	}

	// Ask for confirmation unless --yes flag is used
	if !args.yes {
		print!("\n❓ Execute this command? [y/N]: ");
		io::Write::flush(&mut io::stdout())?;

		let mut input = String::new();
		io::stdin().read_line(&mut input)?;
		let input = input.trim().to_lowercase();

		if input != "y" && input != "yes" {
			println!("❌ Command execution cancelled.");
			return Ok(());
		}
	}

	// Execute the command by passing control to the shell
	println!("\n🚀 Executing: {}", shell_response.command);

	// Add command to shell history before execution
	let _ = add_to_shell_history(&shell_response.command);

	let status = std::process::Command::new("sh")
		.arg("-c")
		.arg(&shell_response.command)
		.status()?;

	// Show exit status only if command failed
	if !status.success() {
		use colored::Colorize;
		println!(
			"❌ Command failed with exit code: {}",
			status.code().unwrap_or(-1).to_string().red()
		);
		std::process::exit(status.code().unwrap_or(1));
	}

	Ok(())
}

fn create_shell_system_prompt() -> String {
	format!(
		"You are a shell command generator. Your task is to convert natural language descriptions into appropriate shell commands.\n\n\
			INSTRUCTIONS:\n\
			1. Generate safe, correct shell commands for the given description\n\
			2. Prefer commonly available tools and standard Unix commands\n\
			3. Always respond with properly formatted JSON\n\
			4. Include safety warnings for potentially dangerous commands\n\
			5. Make commands as specific as possible while being safe\n\
			6. Consider the current working directory: {}\n\n\
			SAFETY GUIDELINES:\n\
			- Avoid destructive operations without explicit user request\n\
			- Warn about commands that modify system files\n\
			- Prefer read-only operations when possible\n\
			- Include safety flags where appropriate (e.g., -i for interactive)\n\n\
			RESPONSE FORMAT:\n\
			Always respond with a JSON object containing exactly these fields:\n\
			- \"command\": string with the exact shell command\n\
			- \"explanation\": string explaining what the command does\n\
			- \"safety_notes\": optional string with warnings (null if no warnings needed)",
		std::env::current_dir()
			.map(|p| p.display().to_string())
			.unwrap_or_else(|_| "unknown".to_string())
	)
}
