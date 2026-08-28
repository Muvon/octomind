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
pub mod retention;

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
	/// Stable memory IDs this record directly relates to. File records retain
	/// these as inspectable graph edges; retrieval expands one hop in either direction.
	#[serde(default)]
	pub related: Vec<String>,
	/// Addressable provenance handles, currently `session://<name>/message/<n>`.
	#[serde(default)]
	pub evidence: Vec<String>,
	/// Outcome of the trajectory that produced this record.
	#[serde(default)]
	pub outcome: TrajectoryOutcome,
	/// Last time this memory was materially used by the specialist. Exposure
	/// alone does not update it.
	#[serde(default)]
	pub last_used: String,
	/// Number of materially-attributed uses. This is retention evidence, not a
	/// proxy for truth; outcome credit remains in `importance`.
	#[serde(default)]
	pub use_count: u64,
	/// Runtime source path populated by the file backend. It is deliberately not
	/// serialized into records: hot/cold moves must not make stored metadata stale.
	#[serde(skip)]
	pub storage_path: String,
}

/// Runtime outcome label handed to detached extraction. Unknown is honest: a
/// compaction or explicit `/done` can happen before the completion gate runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryOutcome {
	#[default]
	Unknown,
	Verified,
	Failed,
}

impl TrajectoryOutcome {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Unknown => "unknown",
			Self::Verified => "verified",
			Self::Failed => "failed",
		}
	}
}

impl std::str::FromStr for TrajectoryOutcome {
	type Err = String;

	fn from_str(value: &str) -> Result<Self, Self::Err> {
		match value.trim().to_ascii_lowercase().as_str() {
			"unknown" => Ok(Self::Unknown),
			"verified" => Ok(Self::Verified),
			"failed" => Ok(Self::Failed),
			other => Err(format!("unknown learning outcome '{other}'")),
		}
	}
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
			related: Vec::new(),
			evidence: Vec::new(),
			outcome: TrajectoryOutcome::Unknown,
			last_used: String::new(),
			use_count: 0,
			storage_path: String::new(),
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
			"related" => Some(serde_json::json!(self.related)),
			"evidence" => Some(serde_json::json!(self.evidence)),
			"outcome" => Some(serde_json::Value::String(self.outcome.as_str().to_string())),
			"last_used" => Some(serde_json::Value::String(self.last_used.clone())),
			"use_count" => Some(serde_json::json!(self.use_count)),
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

/// Minimum user messages before intermediate learning triggers during
/// auto-compaction.
pub const MIN_MESSAGES_FOR_INTERMEDIATE: usize = 3;

/// Soft time-decay: scoped entries unused for this many days lose confidence.
pub const DECAY_DAYS: u64 = 90;

impl Default for LearningConfig {
	fn default() -> Self {
		Self {
			enabled: false,
			model: default_learning_model(),
			backend: default_backend(),
			store: None,
			retrieve: None,
		}
	}
}
