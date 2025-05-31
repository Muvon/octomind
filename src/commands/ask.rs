use clap::Args;
use anyhow::Result;
use std::io::{self, Read};
use std::fs;
use octodev::config::Config;
use octodev::session::{Message, chat_completion_with_provider};
use octodev::session::chat::markdown::{MarkdownRenderer, is_markdown_content};
use colored::Colorize;
use rustyline::{Editor, Config as RustylineConfig, CompletionType, EditMode};
use rustyline::error::ReadlineError;
use glob::glob;
use atty;

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
fn print_response(content: &str, use_raw: bool) {
	if use_raw {
		// Use plain text output
		println!("{}", content);
	} else if is_markdown_content(content) {
		// Use markdown rendering
		let renderer = MarkdownRenderer::new();
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
				let mut found_any = false;
				for path_result in paths {
					match path_result {
						Ok(path) => {
							found_any = true;
							if let Ok(content) = fs::read_to_string(&path) {
								context.push_str(&format!("### File: {}\n\n", path.display()));
								context.push_str("```\n");
								context.push_str(&content);
								if !content.ends_with('\n') {
									context.push('\n');
								}
								context.push_str("```\n\n");
							} else {
								context.push_str(&format!("### File: {} (could not read)\n\n", path.display()));
							}
						}
						Err(e) => {
							eprintln!("Warning: Error reading path in pattern '{}': {}", pattern, e);
						}
					}
				}
				if !found_any {
					eprintln!("Warning: No files found matching pattern '{}'", pattern);
				}
			}
			Err(e) => {
				eprintln!("Warning: Invalid glob pattern '{}': {}", pattern, e);
			}
		}
	}

	Ok(context)
}

// Helper function to get multi-line input interactively using rustyline
fn get_interactive_input() -> Result<String> {
	println!("{}", "Enter your question (multi-line input supported):".bright_blue());
	println!("{}","- Use Ctrl+J to add new lines".dimmed());
	println!("{}","- Press Enter on empty line to finish and send".dimmed());
	println!("{}","- Type '/exit' or '/quit' to cancel".dimmed());
	println!("{}","- Press Ctrl+D to cancel".dimmed());
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
	// Get input from argument, stdin, or interactive mode
	let input = if let Some(input) = &args.input {
		input.clone()
	} else if !atty::is(atty::Stream::Stdin) {
		// Read from stdin if it's being piped
		let mut buffer = String::new();
		io::stdin().read_to_string(&mut buffer)?;
		buffer.trim().to_string()
	} else {
		// Interactive mode - no argument provided and stdin is a terminal
		match get_interactive_input() {
			Ok(input) => input,
			Err(e) => {
				eprintln!("Cancelled: {}", e);
				std::process::exit(1);
			}
		}
	};

	if input.is_empty() {
		eprintln!("Error: No input provided.");
		std::process::exit(1);
	}

	// Read file context if any file patterns are provided
	let file_context = read_files_as_context(&args.files)?;
	
	// Combine input with file context
	let full_input = if file_context.is_empty() {
		input
	} else {
		format!("{}\n\n{}", file_context, input)
	};

	// Determine model to use: either from --model flag or effective config model
	let model = args.model.clone()
		.unwrap_or_else(|| config.get_effective_model());

	// Simple system prompt for ask command - no mode complexity needed
	let system_prompt = "You are a helpful assistant.".to_string();

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
			content: full_input,
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
	let response = chat_completion_with_provider(
		&messages,
		&model,
		args.temperature,
		config,
	).await?;

	// Print the response with optional markdown rendering
	print_response(&response.content, args.raw);

	Ok(())
}