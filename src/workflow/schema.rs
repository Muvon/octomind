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

//! Workflow file schema. Parsed from a standalone TOML document.

use serde::{Deserialize, Deserializer};

/// Top-level workflow definition.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowDef {
	pub name: String,
	#[serde(default)]
	pub description: Option<String>,
	/// Step name whose output becomes stdout. Defaults to the last step
	/// (resolved by the executor — None here means "use last").
	#[serde(default)]
	pub result: Option<String>,
	#[serde(default)]
	pub steps: Vec<Step>,
}

/// Session reuse policy for a single step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
	#[default]
	Fresh,
	Continue,
}

/// Leaf step: actually invokes `octomind run`.
#[derive(Debug, Clone, Deserialize)]
pub struct Sequential {
	pub name: String,
	pub role: String,
	pub prompt: String,
	#[serde(default)]
	pub session: SessionMode,
	/// Seconds. 0 = no timeout.
	#[serde(default)]
	pub timeout: u64,
	#[serde(default)]
	pub retries: u32,
}

/// Pattern test against a step's output.
#[derive(Debug, Clone, Deserialize)]
pub struct Condition {
	/// Step whose output to test. None = immediately preceding step.
	#[serde(default)]
	pub output: Option<String>,
	#[serde(default)]
	pub contains: Option<String>,
	#[serde(default)]
	pub matches: Option<String>,
}

/// Top-level step kinds.
///
/// TOML uses boolean flags (`parallel = true`, `loop = true`,
/// `conditional = true`) to discriminate. A custom Deserialize routes
/// the raw table to the right variant; without a flag it is `Sequential`.
#[derive(Debug, Clone)]
pub enum Step {
	Sequential(Sequential),
	Parallel(ParallelStep),
	Loop(LoopStep),
	Conditional(ConditionalStep),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParallelStep {
	pub name: String,
	pub run: Vec<Sequential>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoopStep {
	pub name: String,
	#[serde(default = "default_max_iterations")]
	pub max_iterations: u32,
	#[serde(default)]
	pub exit_when: Option<Condition>,
	pub run: Vec<Sequential>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConditionalStep {
	pub name: String,
	pub condition: Condition,
	#[serde(default)]
	pub on_match: Vec<String>,
	#[serde(default)]
	pub on_no_match: Vec<String>,
	pub run: Vec<Sequential>,
}

fn default_max_iterations() -> u32 {
	10
}

// ── Step discrimination ──────────────────────────────────────────────────────

impl<'de> Deserialize<'de> for Step {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		let mut table = toml::Table::deserialize(d)?;

		let parallel = table
			.remove("parallel")
			.and_then(|v| v.as_bool())
			.unwrap_or(false);
		let loop_ = table
			.remove("loop")
			.and_then(|v| v.as_bool())
			.unwrap_or(false);
		let conditional = table
			.remove("conditional")
			.and_then(|v| v.as_bool())
			.unwrap_or(false);

		let flags = [parallel, loop_, conditional].iter().filter(|b| **b).count();
		if flags > 1 {
			return Err(serde::de::Error::custom(
				"step may set at most one of: parallel, loop, conditional",
			));
		}

		let val = toml::Value::Table(table);
		if parallel {
			let s: ParallelStep = val.try_into().map_err(serde::de::Error::custom)?;
			Ok(Step::Parallel(s))
		} else if loop_ {
			let s: LoopStep = val.try_into().map_err(serde::de::Error::custom)?;
			Ok(Step::Loop(s))
		} else if conditional {
			let s: ConditionalStep = val.try_into().map_err(serde::de::Error::custom)?;
			Ok(Step::Conditional(s))
		} else {
			let s: Sequential = val.try_into().map_err(serde::de::Error::custom)?;
			Ok(Step::Sequential(s))
		}
	}
}

impl Step {
	pub fn name(&self) -> &str {
		match self {
			Step::Sequential(s) => &s.name,
			Step::Parallel(p) => &p.name,
			Step::Loop(l) => &l.name,
			Step::Conditional(c) => &c.name,
		}
	}
}
