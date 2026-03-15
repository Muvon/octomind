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

// Session setup and initialization utilities

use super::core::{ChatSession, SessionInitParams};
use super::params::extract_session_params;
use crate::config::Config;
use crate::log_info;
use crate::providers::ProviderFactory;
use anyhow::Result;
use colored::*;
use std::io::IsTerminal;
use std::time::Duration;

/// Display a random helpful tip for new sessions
fn display_random_tip() -> String {
	use std::time::{SystemTime, UNIX_EPOCH};

	let tips = [
		"Use ↑/↓ arrows or Ctrl+R for command history search",
		"Press Ctrl+G to add a message to context without sending to AI",
		"Press Tab for command or file completion",
		"Type @ followed by a filename for fuzzy file search and insertion",
		"Start a line with space to skip saving it to history",
		"Press Ctrl+J for multi-line input",
		"Press Ctrl+E to accept a hint when available",
		"Use /context [filter] to view session messages",
		"Use /model <name> to switch AI model mid-session",
		"Use /role <name> to switch role configuration",
		"Use /mcp list to see available MCP tools",
		"Use /run [command] to run a command",
		"Use /prompt [text] to send some predefined prompt",
		"Use /info to see current session costs and token usage",
		"Use /workflow to execute multi-step automation tasks",
	];

	// Generate deterministic but randomized tip based on session start time
	let now = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();
	let index = (now as usize) % tips.len();

	format!("💡 Tip: {}", tips[index])
}

// Helper function to setup session parameters and initialize chat session
pub async fn setup_and_initialize_session<T: std::fmt::Debug>(
	args: &T,
	config: &Config,
) -> Result<(ChatSession, Config, String, bool)> {
	use indicatif::{ProgressBar, ProgressStyle};

	// Show loading spinner in interactive mode
	let spinner = if std::io::stdin().is_terminal() {
		let sp = ProgressBar::new_spinner();
		sp.set_style(
			ProgressStyle::default_spinner()
				.template(" {spinner:.cyan} {msg:.cyan}")
				.unwrap()
				.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
		);
		sp.set_message("Starting session...");
		sp.enable_steady_tick(Duration::from_millis(80));
		Some(sp)
	} else {
		None
	};

	// Extract session parameters
	let (
		name,
		resume,
		resume_recent,
		model,
		max_tokens,
		temperature,
		role,
		max_retries,
		output_mode,
		system_file,
		instructions_file,
		schema_file,
	) = extract_session_params(args, config);

	// Validate role exists before doing anything — give a clean error instead of a panic
	if !config.has_role(&role) {
		let available: Vec<&str> = config.role_map.keys().map(|s| s.as_str()).collect();
		return Err(anyhow::anyhow!(
			"Role '{}' not found. Available roles: {}",
			role,
			available.join(", ")
		));
	}

	// Get role config for defaults
	let (role_config, _, _, _, _) = config.get_role_config(&role);

	// Validate provider credentials before starting — fail fast with a clear error
	// Priority: CLI --model > role.model > config.model
	let effective_model = model
		.as_deref()
		.or(role_config.model.as_deref())
		.unwrap_or(&config.model);
	if let Err(e) = validate_provider_credentials(effective_model) {
		if let Some(sp) = spinner {
			sp.finish_and_clear();
			print!("\x1B[2K\r");
			std::io::Write::flush(&mut std::io::stdout()).ok();
		}
		return Err(e);
	}

	// Get current directory - use thread-local if set (ACP sessions), otherwise process cwd
	let current_dir = crate::mcp::get_thread_working_directory();

	// Get the merged configuration for the specified role
	let mut config_for_role = config.get_merged_config_for_role(&role);

	// Apply CLI overrides directly into config_for_role — single injection point, no downstream changes
	if let Some(ref path) = system_file {
		match std::fs::read_to_string(path) {
			Ok(content) => {
				log_info!("Overriding system prompt from file: {}", path);
				// Mutate the role entry so create_system_prompt picks it up via get_role_config()
				if let Some(role_entry) = config_for_role.role_map.get_mut(&role) {
					role_entry.config.system = content;
				}
			}
			Err(e) => log_info!("Failed to read --system file {}: {}", path, e),
		}
	}
	if let Some(ref path) = instructions_file {
		// Use absolute path — Path::join with absolute path ignores the base, so existing logic works
		config_for_role.custom_instructions_file_name = path.clone();
	}

	// Store output_mode in config for later use in main loop
	config_for_role.runtime_output_mode = Some(output_mode.clone());
	if config_for_role.max_session_tokens_threshold > 0 {
		if let Err(e) =
			crate::session::validate_session_token_threshold(&config_for_role, &role, &current_dir)
				.await
		{
			return Err(anyhow::anyhow!(
				"Session initialization failed: {}\nTo fix this issue\n1. Increase max_session_tokens_threshold in your config\n2. Or disable compression by setting max_session_tokens_threshold = 0\n3. Or reduce the number of MCP servers to lower tool overhead",
				e
			));
		}
	}

	// Create or load session
	let mut session_params = SessionInitParams::new(&config_for_role, &role);

	if let Some(name) = name {
		session_params = session_params.with_name(name);
	}
	if let Some(resume) = resume {
		session_params = session_params.with_resume(resume);
	}
	if resume_recent {
		session_params = session_params.with_resume_recent(true);
	}
	if let Some(model) = model.clone() {
		session_params = session_params.with_model(model);
	}

	// Use CLI temperature if provided, otherwise use role config temperature
	let effective_temperature = temperature.unwrap_or(role_config.temperature);
	session_params = session_params.with_temperature(effective_temperature);

	// Use CLI max_tokens if provided, otherwise use config default
	let effective_max_tokens =
		max_tokens.unwrap_or_else(|| config_for_role.get_effective_max_tokens());
	session_params = session_params.with_max_tokens(effective_max_tokens);

	// Use CLI max_retries if provided, otherwise use root config max_retries
	let effective_max_retries = max_retries.unwrap_or(config_for_role.max_retries);
	session_params = session_params.with_max_retries(effective_max_retries);

	// Set output mode for CLI output suppression in JSONL mode
	let output_mode_for_check = output_mode.clone();
	let output_mode_clone = output_mode.clone();
	session_params = session_params.with_output_mode(output_mode_clone);

	// Clean up spinner BEFORE initializing session (which prints messages)
	if let Some(sp) = spinner {
		sp.finish_and_clear();
		// Clear entire line and move cursor to beginning
		print!("\x1B[2K\r");
		std::io::Write::flush(&mut std::io::stdout()).ok();
	}

	let mut chat_session = ChatSession::initialize(session_params).await?;

	// Display initial status line for new sessions (not resumed) - skip in structured output modes
	let suppress = crate::session::output::OutputMode::from_runtime_mode(&output_mode_for_check)
		.should_suppress_cli_output();
	if !chat_session.was_resumed && !suppress {
		// Show tip first, then shortcut help
		println!("{}", display_random_tip().bright_yellow());
		println!("{}", "? for shortcuts • /help for commands".bright_black());
		chat_session.initial_status_shown = true;
	}

	// Apply runtime overrides (these override the session initialization values)
	if let Some(runtime_model) = &model {
		chat_session.model = runtime_model.clone();
		log_info!("Using runtime model override: {}", runtime_model);
	}

	// Apply runtime temperature override if provided via CLI
	if let Some(runtime_temperature) = temperature {
		chat_session.temperature = runtime_temperature;
		log_info!(
			"Using runtime temperature override: {}",
			runtime_temperature
		);
	}

	// Apply runtime max_tokens override if provided via CLI
	if let Some(runtime_max_tokens) = max_tokens {
		chat_session.max_tokens = runtime_max_tokens;
		log_info!("Using runtime_max_tokens override: {}", runtime_max_tokens);
	}

	// Apply runtime max_retries override if provided via CLI
	if let Some(runtime_max_retries) = max_retries {
		chat_session.max_retries = runtime_max_retries;
		log_info!(
			"Using runtime max_retries override: {}",
			runtime_max_retries
		);
	}

	// Load and apply schema for structured output if provided via --schema
	if let Some(ref path) = schema_file {
		match std::fs::read_to_string(path) {
			Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
				Ok(schema) => {
					log_info!("Using structured output schema from: {}", path);
					chat_session.schema = Some(schema);
				}
				Err(e) => return Err(anyhow::anyhow!("Invalid JSON schema file {}: {}", path, e)),
			},
			Err(e) => {
				return Err(anyhow::anyhow!(
					"Failed to read --schema file {}: {}",
					path,
					e
				))
			}
		}
	}

	// Track if the first message has been processed through layers

	let first_message_processed = !chat_session.session.messages.is_empty();

	Ok((chat_session, config_for_role, role, first_message_processed))
}

/// Check that the provider for the given model string has its credentials set.
/// Fails fast before the session starts — avoids the confusing "first message fails" UX.
fn validate_provider_credentials(model: &str) -> Result<()> {
	let (provider, _) = ProviderFactory::parse_model(model)
		.map_err(|e| anyhow::anyhow!("Invalid model '{}': {}", model, e))?;
	let provider_instance = ProviderFactory::create_provider(&provider)
		.map_err(|e| anyhow::anyhow!("Unknown provider '{}': {}", provider, e))?;
	provider_instance
		.get_api_key()
		.map(|_| ())
		.map_err(|e| anyhow::anyhow!("Provider '{}' credentials missing: {}", provider, e))
}
