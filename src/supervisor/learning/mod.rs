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
//! One backend: `file` — `.md` files with YAML frontmatter in
//! `learning/{project}/{role_base}/` (cross-project lessons in `learning/_/`).
//! External memory MCP tools are never a learning store.

pub mod backend;
pub mod evolution;
pub mod extract;
pub mod inject;
pub mod retention;

use serde::{Deserialize, Serialize};

/// A single learned lesson stored in the file-backed learning corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
	pub content: String,
	/// Short summary title used for retrieval and display.
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
	/// Absent on legacy records. A citation alone does not establish verification.
	#[serde(default)]
	pub user_rule: Option<UserRuleProvenance>,
	/// Replaced scope-qualified IDs; interrupted cleanup cannot revive an old rule.
	#[serde(default)]
	pub supersedes: Vec<String>,
	/// Evolution record IDs of the generated behavior applied in the session
	/// that produced this record: the harness it ran under. Recall names the
	/// ones applied in the recalling session alongside the record.
	#[serde(default)]
	pub artifacts: Vec<String>,
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

/// The exact rule and scope accepted by extraction's grounding verifier.
/// This is provenance in the trusted local store, not authentication against
/// a writer who can replace both the rule and its provenance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct UserRuleProvenance {
	pub quote: String,
	pub session: String,
	pub message_id: String,
	pub content: String,
	pub scope: String,
	pub project: String,
	pub role: String,
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
			user_rule: None,
			supersedes: Vec::new(),
			artifacts: Vec::new(),
			outcome: TrajectoryOutcome::Unknown,
			last_used: String::new(),
			use_count: 0,
			storage_path: String::new(),
		}
	}
}

impl Lesson {
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
		let ts: String = self.created.replace([':', '-', 'T'], "").chars().take(14).collect();
		if slug.is_empty() {
			ts
		} else {
			format!("{}-{}", ts, slug)
		}
	}

	pub fn is_verified_user_rule(&self) -> bool {
		self.user_rule.is_some()
	}

	pub(crate) fn record_key(&self) -> String {
		if self.scope == "global" {
			format!("_/{}", self.file_id())
		} else {
			let role = self.role.split(':').next().expect("split always returns a role base");
			format!("{}/{}/{}", self.project, role, self.file_id())
		}
	}

	pub(crate) fn validate_user_rule(&self) -> anyhow::Result<()> {
		let Some(rule) = &self.user_rule else {
			return Ok(());
		};
		anyhow::ensure!(
			!rule.quote.trim().is_empty()
				&& !rule.session.is_empty()
				&& !rule.message_id.is_empty()
				&& !rule.content.trim().is_empty(),
			"user_rule requires non-empty quote, session, message_id and content"
		);
		anyhow::ensure!(
			self.memory_type == "learning"
				&& rule.content == self.content
				&& rule.scope == self.scope
				&& matches!(rule.scope.as_str(), "scoped" | "global")
				&& rule.project == self.project
				&& rule.role == self.role,
			"user_rule no longer matches the verified content, type or scope"
		);
		Ok(())
	}
}

/// Learning configuration — added to the main Config struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearningConfig {
	/// Enable the learning system.
	#[serde(default)]
	pub enabled: bool,
	/// Grounded behavior synthesis and lifecycle management.
	pub evolution: evolution::EvolutionConfig,
}

/// Soft time-decay: weak scoped entries unused for this many days (counted from
/// the last material use, or creation when never used) move to the cold archive.
pub const DECAY_DAYS: u64 = 90;

#[cfg(test)]
#[path = "mod_tests.rs"]
pub(crate) mod mod_tests;
