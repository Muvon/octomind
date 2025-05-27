use clap::Args;
use anyhow::Result;
use std::io::{self, Read};
use octodev::config::Config;
use octodev::session::{Message, create_system_prompt, chat_completion_with_provider};
use octodev::session::chat::markdown::{MarkdownRenderer, is_markdown_content};
use colored::Colorize;

#[derive(Args, Debug)]
pub struct AskArgs {
	/// Question or input to ask the AI
	#[arg(value_name = "INPUT")]
	pub input: Option<String>,

	/// Use a specific model instead of the one configured in config
	#[arg(long)]
	pub model: Option<String>,

	/// Mode to use for the AI response (developer or assistant)
	#[arg(long, default_value = "assistant")]
	pub mode: String,

	/// Temperature for the AI response (0.0 to 1.0)
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

pub async fn execute(args: &AskArgs, config: &Config) -> Result<()> {
	// Get input from argument or stdin
	let input = if let Some(input) = &args.input {
		input.clone()
	} else {
		// Read from stdin
		let mut buffer = String::new();
		io::stdin().read_to_string(&mut buffer)?;
		buffer.trim().to_string()
	};

	if input.is_empty() {
		eprintln!("Error: No input provided. Use argument or pipe input to stdin.");
		std::process::exit(1);
	}

	// Get mode configuration
	let mode_config = config.get_merged_config_for_mode(&args.mode);
	
	// Determine model to use
	let model = args.model.as_ref()
		.unwrap_or(&mode_config.openrouter.model)
		.clone();

	// Create system prompt for the mode
	let current_dir = std::env::current_dir()?;
	let system_prompt = create_system_prompt(&current_dir, config, &args.mode).await;

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
			content: input,
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