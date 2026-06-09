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

//! Cross-session adaptive learning module.
//!
//! Extracts generalizable lessons from conversations and injects relevant ones
//! into future sessions. Separate from memory (octobrain) — learning is narrower
//! and structured: actionable facts scored by confidence with deduplication.
//!
//! Two backends:
//! - `file` (default): `.md` files with YAML frontmatter in `learning/{role}/{project}/`
//! - `mcp`: any MCP tool (e.g. octobrain) with configurable field mapping

pub mod backend;
pub mod extract;
pub mod inject;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single learned lesson — the canonical schema all backends map to/from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
	pub content: String,
	/// Short summary title (required by some MCP backends like octobrain).
	#[serde(default)]
	pub title: String,
	#[serde(default = "default_memory_type")]
	pub memory_type: String,
	#[serde(default = "default_importance")]
	pub importance: f64,
	#[serde(default = "default_confidence")]
	pub confidence: String,
	#[serde(default)]
	pub tags: Vec<String>,
	#[serde(default)]
	pub source: String,
	#[serde(default)]
	pub role: String,
	#[serde(default)]
	pub project: String,
	/// Memory scope: "scoped" (project×role, contextual/short-lived) or
	/// "global" (cross-project user preference, durable/long-lived).
	#[serde(default = "default_scope")]
	pub scope: String,
	#[serde(default)]
	pub created: String,
}

fn default_memory_type() -> String {
	"learning".into()
}
fn default_importance() -> f64 {
	0.5
}
fn default_confidence() -> String {
	"medium".into()
}
fn default_scope() -> String {
	"scoped".into()
}

impl Default for Lesson {
	fn default() -> Self {
		Self {
			content: String::new(),
			title: String::new(),
			memory_type: "learning".into(),
			importance: 0.5,
			confidence: "medium".into(),
			tags: Vec::new(),
			source: String::new(),
			role: String::new(),
			project: String::new(),
			scope: "scoped".into(),
			created: String::new(),
		}
	}
}

impl Lesson {
	/// Get a field value by canonical name for field mapping.
	pub fn get_field(&self, name: &str) -> Option<serde_json::Value> {
		match name {
			"content" => Some(serde_json::Value::String(self.content.clone())),
			"title" => Some(serde_json::Value::String(self.title.clone())),
			"memory_type" => Some(serde_json::Value::String(self.memory_type.clone())),
			"importance" => Some(serde_json::json!(self.importance)),
			// confidence → maps to octobrain's "source" trust tier when field_map says so
			"confidence" => {
				let mapped = match self.confidence.as_str() {
					"high" => "user_confirmed",
					_ => "agent_inferred",
				};
				Some(serde_json::Value::String(mapped.to_string()))
			}
			"tags" => Some(serde_json::json!(self.tags)),
			"source" => Some(serde_json::Value::String(self.source.clone())),
			"role" => Some(serde_json::Value::String(self.role.clone())),
			"project" => Some(serde_json::Value::String(self.project.clone())),
			"scope" => Some(serde_json::Value::String(self.scope.clone())),
			"created" => Some(serde_json::Value::String(self.created.clone())),
			_ => None,
		}
	}

	/// Stable file id (filename stem) for the file backend: `{ts}-{slug}` of
	/// content + created timestamp. Canonical — used by store, supersede, and
	/// the `/learning` command so all three agree on identity.
	pub fn file_id(&self) -> String {
		let slug: String = self
			.content
			.chars()
			.filter_map(|c| {
				if c.is_alphanumeric() {
					Some(c.to_ascii_lowercase())
				} else if c == ' ' || c == '_' || c == '-' {
					Some('-')
				} else {
					None
				}
			})
			.take(40)
			.collect::<String>()
			.trim_end_matches('-')
			.to_string();
		let ts: String = self
			.created
			.replace([':', '-', 'T'], "")
			.chars()
			.take(14)
			.collect();
		if slug.is_empty() {
			ts
		} else {
			format!("{}-{}", ts, slug)
		}
	}
}

/// Context for retrieving relevant lessons.
pub struct RetrievalContext {
	/// The user's task/input text.
	pub query: String,
	/// Current role (e.g. "developer:general").
	pub role: String,
	/// Current project basename.
	pub project: String,
	/// Max lessons to return.
	pub limit: usize,
}

/// Learning configuration — added to the main Config struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningConfig {
	/// Enable the learning system.
	#[serde(default)]
	pub enabled: bool,
	/// Model for extraction and retrieval prep LLM calls (cheap model recommended).
	#[serde(default = "default_learning_model")]
	pub model: String,
	/// Backend type: "file" or "mcp".
	#[serde(default = "default_backend")]
	pub backend: String,
	/// Minimum user messages before intermediate learning triggers during auto-compaction.
	#[serde(default = "default_min_messages")]
	pub min_messages_for_intermediate: usize,
	/// Max lessons to inject into the system prompt.
	#[serde(default = "default_max_inject")]
	pub max_inject: usize,
	/// MCP store configuration (only used when backend = "mcp").
	#[serde(default)]
	pub store: Option<McpEndpointConfig>,
	/// MCP retrieve configuration (only used when backend = "mcp").
	#[serde(default)]
	pub retrieve: Option<McpEndpointConfig>,
}

/// Configuration for an MCP endpoint (store or retrieve).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpEndpointConfig {
	/// MCP tool name (e.g. "memorize", "remember").
	pub tool: String,
	/// Field mapping: canonical field name → MCP argument name. Empty string = omit.
	#[serde(default)]
	pub field_map: HashMap<String, String>,
}

fn default_learning_model() -> String {
	"anthropic:claude-haiku-4-5-20251001".into()
}
fn default_backend() -> String {
	"file".into()
}
fn default_min_messages() -> usize {
	3
}
fn default_max_inject() -> usize {
	5
}

impl Default for LearningConfig {
	fn default() -> Self {
		Self {
			enabled: false,
			model: default_learning_model(),
			backend: default_backend(),
			min_messages_for_intermediate: default_min_messages(),
			max_inject: default_max_inject(),
			store: None,
			retrieve: None,
		}
	}
}
