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
use colored::Colorize;
use glob::glob;
use octomind::config::Config;
use octomind::session::chat::markdown::{is_markdown_content, MarkdownRenderer};
use octomind::session::{chat_completion_with_provider, Message, ProviderResponse};
use rustyline::error::ReadlineError;
use rustyline::{CompletionType, Config as RustylineConfig, EditMode, Editor};
use std::fs;
use std::io::IsTerminal;
use std::io::{self, Read};

#[derive(Args, Debug)]
pub struct AskArgs {
	/// Question or input to ask the AI
	#[arg(value_name = "INPUT")]
	pub input: Option<String>,

	/// Include files as context (supports glob patterns, can be used multiple times)
	#[arg(short = 'f', long = "file", value_name = "FILE_PATTERN")]
	pub files: Vec<String>,

	/// Use a specific model instead of the default (runtime only, not saved)
	#[arg(long)]
	pub model: Option<String>,

	/// Temperature for the AI response (0.0 to 1.0, runtime only, not saved)
	#[arg(long, default_value = "0.7")]
	pub temperature: f32,

	/// Output raw text without markdown rendering
	#[arg(long)]
	pub raw: bool,
}

// Helper function to print content with optional markdown rendering for ask command
fn print_response(content: &str, use_raw: bool, config: &Config) {
	if use_raw {
		// Use plain text output
		println!("{}", content);
	} else if is_markdown_content(content) {
		// Use markdown rendering with theme from config
		let theme = config.markdown_theme.parse().unwrap_or_default();
		let renderer = MarkdownRenderer::with_theme(theme);
		match renderer.render_and_print(content) {
			Ok(_) => {
				// Successfully rendered as markdown
			}
			Err(_) => {
				// Fallback to plain text if markdown rendering fails
				println!("{}", content);
			}
		}
	} else {
		// Use plain text with color for non-markdown content
		println!("{}", content.bright_green());
	}
}

// Helper function to validate file patterns and check if they exist
fn validate_file_patterns(file_patterns: &[String]) -> Result<()> {
	if file_patterns.is_empty() {
		return Ok(());
	}

	let mut has_errors = false;
	let mut total_files = 0;

	for pattern in file_patterns {
		match glob(pattern) {
			Ok(paths) => {
				let mut found_any = false;
				for path_result in paths {
					match path_result {
						Ok(path) => {
							found_any = true;
							total_files += 1;
							if !path.exists() {
								eprintln!("Error: File does not exist: {}", path.display());
								has_errors = true;
							} else if !path.is_file() {
								eprintln!("Error: Path is not a file: {}", path.display());
								has_errors = true;
							} else if let Err(e) = fs::metadata(&path) {
								eprintln!("Error: Cannot access file {}: {}", path.display(), e);
								has_errors = true;
							}
						}
						Err(e) => {
							eprintln!("Error: Invalid path in pattern '{}': {}", pattern, e);
							has_errors = true;
						}
					}
				}
				if !found_any {
					eprintln!("Error: No files found matching pattern '{}'", pattern);
					has_errors = true;
				}
			}
			Err(e) => {
				eprintln!("Error: Invalid glob pattern '{}': {}", pattern, e);
				has_errors = true;
			}
		}
	}

	if has_errors {
		return Err(anyhow::anyhow!(
			"File validation failed. Please check the file patterns and try again."
		));
	}

	if total_files > 50 {
		eprintln!(
			"Warning: Including {} files as context. This may result in a very large prompt.",
			total_files
		);
	}

	Ok(())
}

// Helper function to read files from glob patterns and format them as context
fn read_files_as_context(file_patterns: &[String]) -> Result<String> {
	if file_patterns.is_empty() {
		return Ok(String::new());
	}

	let mut context = String::new();
	context.push_str("## File Context\n\n");

	for pattern in file_patterns {
		match glob(pattern) {
			Ok(paths) => {
				for path_result in paths {
					match path_result {
						Ok(path) => {
							if let Ok(content) = fs::read_to_string(&path) {
								context.push_str(&format!("### File: {}\n\n", path.display()));
								context.push_str("```\n");
								context.push_str(&content);
								if !content.ends_with('\n') {
									context.push('\n');
								}
								context.push_str("```\n\n");
							} else {
								// This shouldn't happen as we validated earlier, but handle gracefully
								context.push_str(&format!(
									"### File: {} (could not read)\n\n",
									path.display()
								));
							}
						}
						Err(_) => {
							// Skip errors as we already validated
						}
					}
				}
			}
			Err(_) => {
				// Skip errors as we already validated
			}
		}
	}

	Ok(context)
}

// Helper function to get multi-line input interactively using rustyline
fn get_interactive_input() -> Result<String> {
	println!(
		"{}",
		"Enter your question (multi-line input supported):".bright_blue()
	);
	println!(
		"{}",
		"- Press Enter on empty line to finish and send".dimmed()
	);
	println!(
		"{}",
		"- Type '/exit' or '/quit' to cancel, or press Ctrl+D".dimmed()
	);
	println!();

	let config = RustylineConfig::builder()
		.completion_type(CompletionType::List)
		.edit_mode(EditMode::Emacs)
		.auto_add_history(false) // Don't save to history for this
		.build();

	let mut editor: Editor<(), rustyline::history::FileHistory> = Editor::with_config(config)?;
	let mut lines = Vec::new();
	let mut line_num = 1;

	loop {
		let prompt = if line_num == 1 {
			"❯ ".to_string()
		} else {
			format!("{} ", "┆".dimmed())
		};

		match editor.readline(&prompt) {
			Ok(line) => {
				// Check for exit commands
				let trimmed = line.trim();
				if trimmed == "/exit" || trimmed == "/quit" {
					return Err(anyhow::anyhow!("User cancelled input"));
				}

				// If line is empty and we have content, finish
				if trimmed.is_empty() && !lines.is_empty() {
					break;
				}

				// If first line is empty, continue waiting
				if trimmed.is_empty() && lines.is_empty() {
					continue;
				}

				lines.push(line);
				line_num += 1;
			}
			Err(ReadlineError::Interrupted) => {
				return Err(anyhow::anyhow!("User cancelled input"));
			}
			Err(ReadlineError::Eof) => {
				return Err(anyhow::anyhow!("User cancelled input"));
			}
			Err(err) => {
				return Err(anyhow::anyhow!("Error reading input: {}", err));
			}
		}
	}

	if lines.is_empty() {
		return Err(anyhow::anyhow!("No input provided"));
	}

	Ok(lines.join("\n"))
}

pub async fn execute(args: &AskArgs, config: &Config) -> Result<()> {
	// Validate file patterns first, before any other processing
	if let Err(e) = validate_file_patterns(&args.files) {
		eprintln!("{}", e);
		std::process::exit(1);
	}

	// Determine model to use: either from --model flag or effective config model
	let model = args
		.model
		.clone()
		.unwrap_or_else(|| config.get_effective_model());

	// Simple system prompt for ask command - no mode complexity needed
	let system_prompt = "You are a helpful assistant.".to_string();

	// Create a clean config with no MCP servers for ask command
	// This ensures no tools are sent to the API
	let mut clean_config = config.clone();
	clean_config.mcp.servers.clear();

	// Read file context once (validation already done)
	let file_context = read_files_as_context(&args.files)?;

	// Get input from argument, stdin, or interactive mode
	if let Some(input) = &args.input {
		// Single execution mode - input provided via argument
		let full_input = if file_context.is_empty() {
			input.clone()
		} else {
			format!("{}\n\n{}", file_context, input)
		};

		// Execute once and return
		let response = execute_single_query(
			&full_input,
			&model,
			args.temperature,
			&system_prompt,
			&clean_config,
		)
		.await?;
		print_response(&response.content, args.raw, config);
		Ok(())
	} else if !std::io::stdin().is_terminal() {
		// Read from stdin if it's being piped
		let mut buffer = String::new();
		io::stdin().read_to_string(&mut buffer)?;
		let input = buffer.trim().to_string();

		if input.is_empty() {
			eprintln!("Error: No input provided.");
			std::process::exit(1);
		}

		let full_input = if file_context.is_empty() {
			input
		} else {
			format!("{}\n\n{}", file_context, input)
		};

		// Execute once and return
		let response = execute_single_query(
			&full_input,
			&model,
			args.temperature,
			&system_prompt,
			&clean_config,
		)
		.await?;
		print_response(&response.content, args.raw, config);
		return Ok(());
	} else {
		// Interactive multimode - no argument provided and stdin is a terminal
		println!(
			"{}",
			"Entering multimode - ask questions continuously (no context preserved)".bright_green()
		);
		println!();

		loop {
			match get_interactive_input() {
				Ok(input) => {
					if input.is_empty() {
						eprintln!("Error: No input provided.");
						continue;
					}

					// Combine input with file context for this query
					let full_input = if file_context.is_empty() {
						input
					} else {
						format!("{}\n\n{}", file_context, input)
					};

					// Execute the query
					match execute_single_query(
						&full_input,
						&model,
						args.temperature,
						&system_prompt,
						&clean_config,
					)
					.await
					{
						Ok(response) => {
							print_response(&response.content, args.raw, config);
							println!(); // Add spacing between responses
						}
						Err(e) => {
							eprintln!("Error: {}", e);
						}
					}
				}
				Err(e) => {
					if e.to_string().contains("User cancelled") {
						println!("Exiting multimode.");
						break;
					} else {
						eprintln!("Error: {}", e);
						continue;
					}
				}
			}
		}

		return Ok(());
	}
}

// Helper function to execute a single query
async fn execute_single_query(
	input: &str,
	model: &str,
	temperature: f32,
	system_prompt: &str,
	config: &Config,
) -> Result<ProviderResponse> {
	// Create messages
	let messages = vec![
		Message {
			role: "system".to_string(),
			content: system_prompt.to_string(),
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
		},
		Message {
			role: "user".to_string(),
			content: input.to_string(),
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
		},
	];

	// Call the AI provider
	chat_completion_with_provider(&messages, model, temperature, config).await
}
