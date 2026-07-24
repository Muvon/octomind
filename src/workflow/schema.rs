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

/// Reserved graph target that terminates workflow execution.
pub const END_NODE: &str = "$end";

/// Top-level workflow definition.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowDef {
	pub name: String,
	#[serde(default)]
	pub description: Option<String>,
	/// Graph-mode entry node. When omitted together with `edges`, steps retain
	/// their legacy declaration-order execution.
	#[serde(default)]
	pub entry: Option<String>,
	/// Maximum nodes executed in graph mode, including repeated visits through
	/// cycles. Required when graph mode is enabled.
	#[serde(default)]
	pub max_transitions: Option<u32>,
	/// Optional hard spending cap (USD) for the whole workflow. Once the summed
	/// cost across completed steps exceeds this, the workflow aborts before the
	/// next step — bounds runaway loops where per-session caps reset each step.
	/// None = no cap.
	#[serde(default)]
	pub max_cost: Option<f64>,
	#[serde(default)]
	pub steps: Vec<Step>,
	/// Ordered graph routes. The first matching conditional edge is selected;
	/// every node must end with one unconditional default edge.
	#[serde(default)]
	pub edges: Vec<Edge>,
}

impl WorkflowDef {
	pub fn is_graph(&self) -> bool {
		self.entry.is_some() || !self.edges.is_empty()
	}

	pub fn graph_max_transitions(&self) -> u32 {
		self.max_transitions
			.expect("validated graph workflows set max_transitions")
	}
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
	/// Optional model override forwarded as `--model` to the subprocess.
	#[serde(default)]
	pub model: Option<String>,
	/// Optional working directory for the subprocess. Relative paths
	/// resolve against the orchestrator's cwd at execution time.
	/// None = inherit the orchestrator's cwd.
	#[serde(default)]
	pub workdir: Option<String>,
	/// Parallel-only: run this sub-step `count` times unchanged — same role,
	/// model, and prompt. The model is non-deterministic, so the runs differ
	/// (best-of-N sampling); shorthand for copy-pasting an identical block.
	/// Must be >= 2. Rejected on non-parallel steps. None = a single run. For
	/// different models or different prompts, write explicit named sub-steps —
	/// each carries its own `model`/`prompt`.
	#[serde(default)]
	pub count: Option<u32>,
	/// Skills to force-load in the subprocess before its first turn, forwarded
	/// as `OCTOMIND_SKILLS` (comma-joined) — same mechanism an interactive
	/// session uses when that env var is set. None = no env-loaded skills.
	#[serde(default)]
	pub skills: Option<Vec<String>>,
	/// Capabilities to force-load in the subprocess before its first turn,
	/// forwarded as `OCTOMIND_CAPABILITIES` (comma-joined) — same mechanism an
	/// interactive session uses when that env var is set. None = no env-loaded
	/// capabilities.
	#[serde(default)]
	pub capabilities: Option<Vec<String>>,
}

impl Sequential {
	/// How many parallel replicas this sub-step expands to (1 unless `count` set).
	pub fn replica_count(&self) -> u32 {
		self.count.unwrap_or(1)
	}
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

/// One directed graph route. Edges are evaluated in declaration order for a
/// completed `from` node. `when = None` is the required default route.
#[derive(Debug, Clone, Deserialize)]
pub struct Edge {
	pub from: String,
	pub to: String,
	#[serde(default)]
	pub when: Option<Condition>,
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
	/// Output to split in dynamic fan-out mode. Required when `match` is set.
	#[serde(default)]
	pub source: Option<String>,
	/// Dynamic fan-out: a regex applied to `source`, splitting it into items
	/// (capture group 1 of each match; the regex must define one).
	/// Each item becomes one branch running the single sub-step template. The
	/// block's OWN name is the loop variable — within each branch it resolves to
	/// that branch's item, so the template references `{{<block-name>}}` to get
	/// the one task. The accumulated output lands under the sub-step's name.
	/// Presence switches the block from static (the listed sub-steps) to dynamic
	/// (one branch per match).
	#[serde(default, rename = "match")]
	pub match_pattern: Option<String>,
	/// Minimum number of replicas (counted across the whole block, after
	/// `count`/`match` expansion) that must succeed for the block to pass.
	/// None = strict: every replica must succeed.
	#[serde(default)]
	pub min_success: Option<u32>,
	/// Cap on how many replicas run concurrently. None = unbounded (all at once).
	#[serde(default)]
	pub max_parallel: Option<usize>,
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

		let flags = [parallel, loop_, conditional]
			.iter()
			.filter(|b| **b)
			.count();
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
