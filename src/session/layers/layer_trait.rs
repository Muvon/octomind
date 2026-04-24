// Copyright 2026 Muvon Un Limited
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

use crate::config::Config;
use crate::session::Session;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

// Layer result returned from ACP execution
pub struct LayerResult {
	pub outputs: Vec<String>,
	pub total_time_ms: u64,
}

// Input mode determines what part of the previous layer's output will be used
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputMode {
	Last,    // Only the last assistant message from the session
	All,     // All messages/data from the previous layer
	Summary, // A summarized version of all data from the previous layer
}

impl InputMode {
	pub fn as_str(&self) -> &'static str {
		match self {
			InputMode::Last => "last",
			InputMode::All => "all",
			InputMode::Summary => "summary",
		}
	}
}

impl FromStr for InputMode {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().as_str() {
			"last" => Ok(InputMode::Last),
			"all" => Ok(InputMode::All),
			"summary" => Ok(InputMode::Summary),
			_ => Err(format!(
				"Unknown input mode: '{}'. Valid options: last, all, summary",
				s
			)),
		}
	}
}

// Custom deserializer for InputMode to handle string values from config
fn deserialize_input_mode<'de, D>(deserializer: D) -> Result<InputMode, D::Error>
where
	D: serde::Deserializer<'de>,
{
	use serde::de::Error;
	let s = String::deserialize(deserializer)?;
	InputMode::from_str(&s).map_err(D::Error::custom)
}

// Output mode determines how the layer's output affects the session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputMode {
	None,    // Don't modify session (intermediate layer like task_refiner)
	Append,  // Add output as new message to session
	Replace, // Replace entire session with output (reducer functionality)
	Last,    // Append only the last response to session (ignore multiple outputs)
	Restart, // Replace session with only the last response (fresh start with last message)
}

impl OutputMode {
	pub fn as_str(&self) -> &'static str {
		match self {
			OutputMode::None => "none",
			OutputMode::Append => "append",
			OutputMode::Replace => "replace",
			OutputMode::Last => "last",
			OutputMode::Restart => "restart",
		}
	}
}

impl FromStr for OutputMode {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().as_str() {
			"none" => Ok(OutputMode::None),
			"append" => Ok(OutputMode::Append),
			"replace" => Ok(OutputMode::Replace),
			"last" => Ok(OutputMode::Last),
			"restart" => Ok(OutputMode::Restart),
			_ => Err(format!(
				"Unknown output mode: '{}'. Valid options: none, append, replace, last, restart",
				s
			)),
		}
	}
}

// Output role determines the role used when adding messages to the session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputRole {
	Assistant, // Add output as assistant message (default)
	User,      // Add output as user message
}

impl OutputRole {
	pub fn as_str(&self) -> &'static str {
		match self {
			OutputRole::Assistant => "assistant",
			OutputRole::User => "user",
		}
	}
}

impl FromStr for OutputRole {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().as_str() {
			"assistant" => Ok(OutputRole::Assistant),
			"user" => Ok(OutputRole::User),
			_ => Err(format!(
				"Unknown output role: '{}'. Valid options: assistant, user",
				s
			)),
		}
	}
}

// Custom deserializer for OutputMode to handle string values from config
fn deserialize_output_mode<'de, D>(deserializer: D) -> Result<OutputMode, D::Error>
where
	D: serde::Deserializer<'de>,
{
	use serde::de::Error;
	let s = String::deserialize(deserializer)?;
	OutputMode::from_str(&s).map_err(D::Error::custom)
}

// Custom deserializer for OutputRole to handle string values from config
fn deserialize_output_role<'de, D>(deserializer: D) -> Result<OutputRole, D::Error>
where
	D: serde::Deserializer<'de>,
{
	use serde::de::Error;
	let s = String::deserialize(deserializer)?;
	OutputRole::from_str(&s).map_err(D::Error::custom)
}

// Common configuration for all layers.
// Layers execute via ACP protocol - model/prompt config lives in the role
// used by the command field, not here. Only orchestration fields remain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayerConfig {
	pub name: String,
	pub description: String,
	pub command: String,
	#[serde(default = "default_workdir")]
	pub workdir: String,
	#[serde(deserialize_with = "deserialize_input_mode")]
	pub input_mode: InputMode,
	#[serde(deserialize_with = "deserialize_output_mode")]
	pub output_mode: OutputMode,
	#[serde(deserialize_with = "deserialize_output_role")]
	pub output_role: OutputRole,
}

fn default_workdir() -> String {
	".".to_string()
}

impl LayerConfig {
	/// Get the resolved working directory as an absolute path.
	///
	/// If workdir is relative, it's resolved relative to the session's working directory.
	pub fn get_resolved_workdir(&self, session_workdir: &std::path::Path) -> std::path::PathBuf {
		let workdir_path = std::path::PathBuf::from(&self.workdir);
		if workdir_path.is_absolute() {
			workdir_path
		} else {
			session_workdir.join(&self.workdir)
		}
	}
}

// Trait that all layers must implement
#[async_trait]
pub trait Layer {
	fn name(&self) -> &str;
	fn config(&self) -> &LayerConfig;

	// Process the input through this layer
	// Each layer handles its own function calls with its own model
	// The process function is responsible for executing any function calls
	// and incorporating their results into the final output
	async fn process(
		&self,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<LayerResult>;

	// Helper function to prepare input based on input_mode
	fn prepare_input(&self, input: &str, session: &Session) -> String {
		// Each layer processes input in its own isolated context
		// The input mode determines what part of the previous context is used
		match self.config().input_mode {
			InputMode::Last => {
				// In Last mode, we get the last assistant response from the session
				// This is useful for commands that want to analyze or work with the last AI response
				if input.trim().is_empty() {
					// If no explicit input provided, get the last assistant message
					session
						.messages
						.iter()
						.rfind(|m| m.role == "assistant")
						.map(|m| m.content.clone())
						.unwrap_or_else(|| {
							// Fallback: if no assistant messages, get last user message
							session
								.messages
								.iter()
								.rfind(|m| m.role == "user")
								.map(|m| m.content.clone())
								.unwrap_or_else(|| "No previous messages found".to_string())
						})
				} else {
					// If explicit input provided, use it but also include last assistant context
					let last_assistant = session
						.messages
						.iter()
						.rfind(|m| m.role == "assistant")
						.map(|m| {
							format!(
								"Previous response:\n{}\n\nCurrent input:\n{}",
								m.content, input
							)
						})
						.unwrap_or_else(|| input.to_string());
					last_assistant
				}
			}
			InputMode::All => {
				// Build a chronological transcript of the session in natural reading order
				// (oldest → newest), followed by the current input as the task to act on.
				// Skips system messages — those are already in the layer's own system prompt.
				let transcript = session
					.messages
					.iter()
					.filter(|m| m.role != "system")
					.map(|m| {
						let label = match m.role.as_str() {
							"assistant" => "Assistant",
							"user" => "User",
							other => other,
						};
						format!("[{}]\n{}", label, m.content)
					})
					.collect::<Vec<_>>()
					.join("\n\n");

				if transcript.is_empty() {
					input.to_string()
				} else {
					format!("{}\n\n[Current task]\n{}", transcript, input)
				}
			}

			InputMode::Summary => {
				// For summary mode, we generate a concise summary of the conversation
				// This helps maintain context while reducing token usage
				crate::session::summarize_context(session, input)
			}
		}
	}
}
