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

//! Detectors — deterministic, free, every turn.
//!
//! Call identity, recorded results and self-reports nominate advisory hints.
//! Repetition may be polling; errors may be expected probes. Neither proves a
//! task failure, so these hints never demand a strategy change or a blocked
//! handback. Completion and authorization are judged at their own boundaries.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};

/// The agent's self-reported state for a turn, parsed from its `<sup>…</sup>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfReport {
	Exploring,
	Progressing,
	Blocked,
	NeedInput,
	Done,
}

/// Compact handoff authored by the main agent at the end of each response.
/// It is an attention signal, not ground truth; compression reconciles it
/// against the transcript before promoting anything to durable knowledge.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SelfReportHandoff {
	pub focus: String,
	pub next: String,
	pub carry: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSelfReport {
	pub state: SelfReport,
	pub handoff: SelfReportHandoff,
	pub plan: Option<super::plan::PlanSignal>,
	/// Pack-local memory IDs that materially affected this response/action.
	pub used_memories: Vec<String>,
	/// Generated skill IDs that materially affected this response/action.
	pub used_behaviors: Vec<String>,
}

impl SelfReport {
	fn from_token(s: &str) -> Option<Self> {
		match s.trim().to_ascii_lowercase().as_str() {
			"exploring" => Some(Self::Exploring),
			"progressing" => Some(Self::Progressing),
			"blocked" => Some(Self::Blocked),
			"need_input" | "need-input" | "needinput" => Some(Self::NeedInput),
			"done" => Some(Self::Done),
			_ => None,
		}
	}

	pub fn as_str(self) -> &'static str {
		match self {
			Self::Exploring => "exploring",
			Self::Progressing => "progressing",
			Self::Blocked => "blocked",
			Self::NeedInput => "need_input",
			Self::Done => "done",
		}
	}
}

/// One-time system-side instruction that makes the agent self-annotate. Injected
/// out-of-band; the resulting tags are stripped before display.
pub const SELF_REPORT_INSTRUCTION: &str = r#"Finish every response with one compact JSON status line — the last line, nothing after it:
	`<sup>{"state":"STATE","focus":"current subgoal and why","next":"next action","carry":["minimum fact or opaque reference needed after context loss"],"plan":null,"memories":[],"behaviors":[]}</sup>`
Use valid single-line JSON with exactly those fields. `carry`, `memories`, and `behaviors` may be empty and `next` is `null` when nothing remains to do. Put an active-memory ID such as `M2` in `memories` only when that entry materially affected this response or its chosen action; put an evolved skill's `evolution_id` in `behaviors` only when its instructions materially affected the work. Never list entries merely because they were shown. Keep only information genuinely needed to resume. Never copy credentials or secret values into the report — retain only an opaque pointer, name, or location used to obtain them. Avoid generic text such as "working" or "continuing". STATE must be exactly one of:
- `exploring` — still gathering context, reading code
- `progressing` — actively making changes
- `blocked` — stuck, cannot proceed
- `need_input` — asking the user a question and waiting on them
- `done` — the user's task is fully complete

`plan` is normally `null`. Set it to `"request"` once, alongside real work, only when the task clearly needs 3+ dependent outcomes or durable tracking. With an injected plan, use `"phase_complete"` alongside the next work batch only after the current outcome is evidenced, or `"reassess"` when evidence invalidates the remaining route. The external manager owns the plan; never emit a response only for planning.
Example: `<sup>{"state":"progressing","focus":"checking the active operation","next":"perform the next status check","carry":["use the resource reference established earlier"],"plan":null,"memories":["M2"],"behaviors":[]}</sup>`
This line is read by the system and hidden from the user. Emit exactly one."#;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSelfReport {
	state: String,
	focus: String,
	// Null at `done`: a finished turn has no next action, and the schema has to
	// say so — a rejected parse loses the terminal state the gate runs on.
	next: Option<String>,
	carry: Vec<String>,
	#[serde(default)]
	plan: Option<super::plan::PlanSignal>,
	#[serde(default)]
	memories: Vec<String>,
	#[serde(default)]
	behaviors: Vec<String>,
}

pub fn parse_self_report_handoff(text: &str) -> Option<ParsedSelfReport> {
	let end = text.rfind("</sup>")?;
	let start = text[..end].rfind("<sup>")? + "<sup>".len();
	let inner = text[start..end].trim();
	if inner.starts_with('{') {
		let wire: WireSelfReport = serde_json::from_str(inner).ok()?;
		return Some(ParsedSelfReport {
			state: SelfReport::from_token(&wire.state)?,
			handoff: SelfReportHandoff {
				focus: wire.focus.trim().to_string(),
				next: wire.next.unwrap_or_default().trim().to_string(),
				carry: wire
					.carry
					.into_iter()
					.map(|entry| entry.trim().to_string())
					.filter(|entry| !entry.is_empty())
					.collect(),
			},
			plan: wire.plan,
			used_memories: wire
				.memories
				.into_iter()
				.map(|id| id.trim().to_string())
				.filter(|id| !id.is_empty())
				.collect(),
			used_behaviors: wire
				.behaviors
				.into_iter()
				.map(|id| id.trim().to_string())
				.filter(|id| !id.is_empty())
				.collect(),
		});
	}

	let (state, reason) = parse_legacy_self_report_inner(inner)?;
	Some(ParsedSelfReport {
		state,
		handoff: SelfReportHandoff {
			focus: reason.unwrap_or_default(),
			..Default::default()
		},
		plan: None,
		used_memories: Vec::new(),
		used_behaviors: Vec::new(),
	})
}

/// Parse the *last* `<sup>…</sup>` token from a response. Returns the state and
/// an optional short reason. Tolerant of the `·` or `|` reason separator.
/// Test-only harness for the legacy parse path; the runtime reaches it through
/// [`parse_self_report_handoff`]'s fallback.
#[cfg(test)]
fn parse_self_report(text: &str) -> Option<(SelfReport, Option<String>)> {
	let end = text.rfind("</sup>")?;
	let start = text[..end].rfind("<sup>")? + "<sup>".len();
	let inner = text[start..end].trim();
	if inner.starts_with('{') {
		let parsed = parse_self_report_handoff(text)?;
		let reason = (!parsed.handoff.focus.is_empty()).then_some(parsed.handoff.focus);
		return Some((parsed.state, reason));
	}
	parse_legacy_self_report_inner(inner)
}

fn parse_legacy_self_report_inner(inner: &str) -> Option<(SelfReport, Option<String>)> {
	// Normal: the body leads with the state. Echo: a model copied the literal
	// `STATE` placeholder from the instruction, so the real state is the next
	// token (`<sup>STATE · done</sup>` → done). Robust to `·`, `|`, `:`, `-`, space.
	let lead = leading_state_token(inner);
	let (state, after) = match SelfReport::from_token(&lead) {
		Some(s) => (s, &inner[lead.len()..]),
		None if lead.eq_ignore_ascii_case("state") => {
			let rest = inner[lead.len()..].trim_start_matches([' ', '·', '|', ':', '-', '\t']);
			let next = leading_state_token(rest);
			(SelfReport::from_token(&next)?, &rest[next.len()..])
		}
		None => return None,
	};
	let reason = after
		.trim_start_matches([' ', '·', '|', ':', '-', '\t'])
		.trim();
	Some((state, (!reason.is_empty()).then(|| reason.to_string())))
}

/// The leading identifier run (`[A-Za-z_-]+`) of a `<sup>` body — the candidate
/// state token, separator-agnostic.
fn leading_state_token(inner: &str) -> String {
	inner
		.trim_start()
		.chars()
		.take_while(|c| c.is_ascii_alphabetic() || *c == '_' || *c == '-')
		.collect()
}

/// Does this `<sup>` body look like a self-report rather than legitimate
/// superscript (`2`, `th`, `®`)? True when it leads with a known state, with the
/// `STATE` placeholder a model may echo from the instruction, or carries the
/// reason separator (`·`/`|`) that real superscript never contains. This is the
/// safety net: an echoed or malformed report still never reaches the screen.
///
/// The JSON form is matched on *shape*, not by deserializing [`WireSelfReport`]:
/// hiding the token must not depend on the model honoring the schema, or an
/// unknown state, an extra field, or truncated JSON puts it on the user's screen.
/// Superscript the user actually wrote (`2`, `th`, `®`) is never a JSON object
/// carrying a `state` key.
fn is_self_report_body(inner: &str) -> bool {
	if inner.trim_start().starts_with('{') {
		return inner.contains("\"state\"");
	}
	let lead = leading_state_token(inner);
	SelfReport::from_token(&lead).is_some()
		|| lead.eq_ignore_ascii_case("state")
		|| inner.contains('·')
		|| inner.contains('|')
}

/// Remove `<sup>…</sup>` tokens that look like a self-report (see
/// [`is_self_report_body`]), leaving legitimate superscript markup untouched.
pub fn strip_self_report(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut rest = text;
	while let Some(start) = rest.find("<sup>") {
		match rest[start..].find("</sup>") {
			Some(rel_end) => {
				let inner = &rest[start + "<sup>".len()..start + rel_end];
				if is_self_report_body(inner) {
					// Drop this token; keep text before it.
					out.push_str(&rest[..start]);
					rest = &rest[start + rel_end + "</sup>".len()..];
				} else {
					// Not ours — keep `<sup>…</sup>` verbatim and continue past it.
					let keep_to = start + rel_end + "</sup>".len();
					out.push_str(&rest[..keep_to]);
					rest = &rest[keep_to..];
				}
			}
			None => break,
		}
	}
	out.push_str(rest);
	out.trim_end().to_string()
}

/// Shape-based: is this call a candidate VERIFIER — something that executes a
/// command whose outcome can validate that the job is done? Judged from what
/// the runtime actually knows, not from hard-coded program names: the call must
/// carry a string `command` parameter (the execution signature — shells,
/// runners, remote executors and domain-specific validators all take one), the
/// call itself must not declare mutation intent, and it must not belong to one
/// of octomind's own builtin control-plane servers (authoritative: resolved via
/// the same registry the dispatcher routes with — `plan` takes a `command`
/// parameter too, but the runtime knows it executes nothing). Whether the
/// round actually verified is then decided OBSERVATIONALLY in
/// [`Detectors::note_round_verification`]: a candidate that dirtied the tree
/// is a mutator, not a verifier.
pub fn is_verifier_shaped(tool: &str, parameters: &serde_json::Value) -> bool {
	!is_mutation_call(tool, parameters) && is_command_execution(tool, parameters)
}

/// A command execution can provide evidence even when it also mutates state.
/// Keep that output for the independent verifier without granting deterministic
/// verification credit. Registered runners are identified by their schema;
/// unregistered tools retain the conservative non-mutation shape fallback.
pub fn is_command_execution(tool: &str, parameters: &serde_json::Value) -> bool {
	let Some(cmd) = parameters.get("command").and_then(|v| v.as_str()) else {
		return false;
	};
	if !executes_free_form_command(tool) && is_mutation_call(tool, parameters) {
		return false;
	}
	// Reject empty command strings: they execute nothing and cannot validate
	// completion.
	if cmd.trim().is_empty() {
		return false;
	}
	match crate::mcp::tool_map::get_tool_server_name(tool) {
		Some(server) => !matches!(
			server.as_str(),
			"core" | "runtime" | "orchestration" | "agent"
		),
		// Unregistered command tool: retain the output as possible evidence.
		None => true,
	}
}

/// Identity of a command execution for the advisory error ledger. Include all
/// arguments (such as workdir), since an identical command can address a
/// different target. This identifies receipts, not behavioral test coverage.
pub fn command_key(tool: &str, parameters: &serde_json::Value) -> Option<u64> {
	is_command_execution(tool, parameters).then(|| hash2(tool, &parameters.to_string()))
}

/// Path-like values in a tool call's parameters — the artifact identities a
/// mutation touches and a later read-back can verify. Generic across tools and
/// domains: any non-empty string under a key containing "path" or "file", plus
/// string arrays under such keys. No tool names, no extension lists.
pub fn param_paths(parameters: &serde_json::Value) -> Vec<String> {
	let mut out = Vec::new();
	if let Some(obj) = parameters.as_object() {
		for (k, v) in obj {
			let kl = k.to_ascii_lowercase();
			if !(kl.contains("path") || kl.contains("file")) {
				continue;
			}
			match v {
				serde_json::Value::String(s) if !s.trim().is_empty() => out.push(s.clone()),
				serde_json::Value::Array(a) => out.extend(
					a.iter()
						.filter_map(|x| x.as_str())
						.filter(|s| !s.trim().is_empty())
						.map(str::to_string),
				),
				_ => {}
			}
		}
	}
	out
}

/// Normal form for mutated-path bookkeeping: the canonical filesystem path when
/// it resolves (tolerates relative-vs-absolute and symlink spellings), else a
/// lexical cleanup (a deleted or virtual path still compares by its own name).
fn normalize_path(path: &str) -> String {
	std::fs::canonicalize(path.trim())
		.map(|p| p.to_string_lossy().into_owned())
		.unwrap_or_else(|_| path.trim().trim_start_matches("./").to_string())
}

/// Classify one concrete call. Three signals, strongest first, each answering
/// what it actually knows:
///
/// 1. The tool's own SCHEMA says whether the call selects a fixed operation or
///    executes a free-form command ([`register_tool_command_shape`]). A command
///    runner is write-CAPABLE by construction — `octofs shell` and every other
///    honest runner annotates `readOnlyHint: false` — so its tool-level hint
///    carries no information about the concrete call, and answering from it
///    classified every build, test and validator run as a mutation. Those runs
///    are the only thing that can verify a change, so nothing could ever clear
///    the pre-gate. What the runner's command actually did to the tree is then
///    observed, not guessed: see [`Detectors::note_round_verification`].
/// 2. Otherwise the MCP `readOnlyHint` annotation, which for a single-purpose
///    tool (an editor, a reader) describes the call as well as the tool.
/// 3. Otherwise normalized intent tokens — the compatibility fallback for tools
///    that ship no annotation, and the only signal available for a runner's
///    concrete command.
pub fn is_mutation_call(tool: &str, parameters: &serde_json::Value) -> bool {
	if executes_free_form_command(tool) {
		// The command only. A runner's NAME describes the tool the same way its
		// `readOnlyHint` does — `deploy_shell` running `ls` changes nothing — so
		// neither can answer for the concrete call.
		return command_intent_is_mutation(parameters);
	}
	if let Some(read_only) = tool_read_only_hint(tool) {
		return !read_only;
	}
	has_explicit_mutation_intent(tool, parameters)
}

/// High-confidence mutation signal from the concrete call itself, ignoring a
/// tool-level `readOnly=false` capability hint: a generic shell/browser/API
/// tool may be capable of writes while the concrete call is only gathering
/// evidence, and classifying that read as a mutation would be a false positive.
fn has_explicit_mutation_intent(tool: &str, parameters: &serde_json::Value) -> bool {
	contains_mutation_intent(tool) || command_intent_is_mutation(parameters)
}

/// Mutation intent carried by the call's own operation parameters, with the
/// tool's identity left out of it.
fn command_intent_is_mutation(parameters: &serde_json::Value) -> bool {
	["command", "action", "operation"]
		.iter()
		.filter_map(|key| parameters.get(key).and_then(|value| value.as_str()))
		.any(contains_mutation_intent)
}

fn contains_mutation_intent(value: &str) -> bool {
	let mut normalized = String::with_capacity(value.len());
	let mut previous_lowercase = false;
	for character in value.chars() {
		if character.is_ascii_uppercase() && previous_lowercase {
			normalized.push(' ');
		}
		if character.is_ascii_alphanumeric() {
			normalized.push(character.to_ascii_lowercase());
			previous_lowercase = character.is_ascii_lowercase();
		} else {
			normalized.push(' ');
			previous_lowercase = false;
		}
	}
	let intents = [
		"write", "edit", "create", "replace", "apply", "insert", "delete", "remove", "patch",
		"mkdir", "rename", "move", "update", "set", "send", "publish", "post", "upload",
		"schedule", "book", "approve", "reject", "cancel", "deploy", "install", "commit", "push",
		"merge",
	];
	normalized
		.split_whitespace()
		.any(|token| intents.contains(&token))
}

fn tool_read_only_hints() -> &'static std::sync::RwLock<std::collections::HashMap<String, bool>> {
	static HINTS: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, bool>>> =
		std::sync::OnceLock::new();
	HINTS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Register the standard MCP read-only hint when an external tool inventory is
/// received. Per the MCP specification this is a hint, not an authorization or
/// safety boundary; it is used only for progress/evidence classification.
pub fn register_tool_read_only_hint(tool: &str, read_only: Option<bool>) {
	let Some(read_only) = read_only else {
		return;
	};
	if let Ok(mut hints) = tool_read_only_hints().write() {
		hints.insert(tool.to_string(), read_only);
	}
}

fn tool_read_only_hint(tool: &str) -> Option<bool> {
	tool_read_only_hints().read().ok()?.get(tool).copied()
}

fn tool_command_shapes() -> &'static std::sync::RwLock<std::collections::HashMap<String, bool>> {
	static SHAPES: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, bool>>> =
		std::sync::OnceLock::new();
	SHAPES.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Register what a tool's `command` parameter IS, read off the tool's own JSON
/// schema when its inventory arrives (see [`command_param_is_free_form`]).
/// `true` = it executes a free-form command (a runner: shells, remote
/// executors, domain validators); `false` = it selects one of a fixed set of
/// operations (an editor's `command: "str_replace"`). The distinction is
/// structural — no tool names, no program names, no keyword lists — and it is
/// what lets the runtime tell a build/test run apart from an edit when both
/// arrive as a string under the same parameter name.
pub fn register_tool_command_shape(tool: &str, free_form: bool) {
	if let Ok(mut shapes) = tool_command_shapes().write() {
		shapes.insert(tool.to_string(), free_form);
	}
}

/// Does this tool execute a free-form command? `false` when unregistered: a
/// tool whose schema the runtime never saw keeps the annotation-first
/// classification it had before.
fn executes_free_form_command(tool: &str) -> bool {
	tool_command_shapes()
		.read()
		.ok()
		.and_then(|shapes| shapes.get(tool).copied())
		.unwrap_or(false)
}

/// Is the `command` property of this tool's input schema a free-form string —
/// a command to execute — rather than a fixed operation vocabulary? Schema
/// facts only: a plain string type with no `enum`/`const` constraint and no
/// `$ref` to a named variant type. `anyOf`/`oneOf` branches are searched too,
/// so a runner that accepts either a string or an argv array still counts.
/// Absent or unreadable `command` → `false`; the caller then falls back to the
/// tool's annotation.
pub fn command_param_is_free_form(schema: &serde_json::Value) -> bool {
	let Some(param) = schema.get("properties").and_then(|p| p.get("command")) else {
		return false;
	};
	fn unconstrained_string(node: &serde_json::Value) -> bool {
		if node.get("enum").is_some() || node.get("const").is_some() || node.get("$ref").is_some() {
			return false;
		}
		match node.get("type") {
			Some(serde_json::Value::String(t)) => t == "string",
			// Nullable/union declarations render the type as a list.
			Some(serde_json::Value::Array(types)) => {
				types.iter().any(|t| t.as_str() == Some("string"))
			}
			_ => false,
		}
	}
	if unconstrained_string(param) {
		return true;
	}
	["anyOf", "oneOf"]
		.iter()
		.filter_map(|key| param.get(key).and_then(|v| v.as_array()))
		.flatten()
		.any(unconstrained_string)
}

const SEEN_CAP: usize = 128;

/// Identical calls and results this many rounds in a row nominate a loop hint.
pub const LOOP_THRESHOLD: usize = 3;

/// Rounds without new receipts nominate a no-progress hint. Also the bounded
/// error budget for advisory command-recovery hints.
pub const NO_PROGRESS_WINDOW: usize = 5;

/// Cap on remembered agent-mutated paths (read-back verification candidates).
/// Oldest evicted — a task touching more artifacts than this verifies via the
/// most recent ones, which is where the read-back lands anyway.
const MUTATED_PATHS_CAP: usize = 32;

/// Cap on distinct command executions with errors and no later success for
/// the same arguments. This advisory ledger is not a list of failed tests.
const FAILED_COMMANDS_CAP: usize = 64;

/// Deterministic per-session detector state, built on a single novelty primitive.
#[derive(Debug, Default)]
pub struct Detectors {
	/// Recent result hashes (loop detection), newest at back.
	loop_window: VecDeque<u64>,
	/// Recent novelty flags (no-progress detection), newest at back.
	novelty_window: VecDeque<bool>,
	/// Result hashes seen recently — for novelty. Bounded by `SEEN_CAP`.
	seen: HashSet<u64>,
	seen_order: VecDeque<u64>,
	/// Observational verification state (see `supervisor::workdir::fingerprint`):
	/// the working-tree fingerprint at the last clean verification — a
	/// verifier-shaped call that succeeded on an UNCHANGED tree. Seeded from the
	/// first observed round's pre-fingerprint (the task-start tree). Once
	/// `agent_dirty` is armed, the detector compares the live fingerprint
	/// against this. Trajectory state, NOT a streak: it persists across turns,
	/// so [`Detectors::reset_streak`] leaves it untouched.
	verified_fp: Option<u64>,
	/// True when some agent ROUND changed the tree — its pre/post fingerprints
	/// differ (a change made through ANY tool, `shell sed -i` included) or,
	/// without fingerprints, a mutation-shaped success — and no clean
	/// verification has run since. Keyed to the agent's own rounds, so external
	/// drift never arms it: between rounds (the user editing their tree
	/// mid-session) the fingerprint moves outside any round, and DURING a round
	/// arming additionally requires a write-capable call (mutation-shaped,
	/// command-executing, or delegated) — a round of pure reads cannot have
	/// moved the tree, so drift there is a concurrent writer, not the agent.
	agent_dirty: bool,
	/// Paths the agent's own successful mutation-shaped calls touched since the
	/// last clean verification — the artifacts a later read-back can verify.
	/// Normalized ([`normalize_path`]), deduped, capped at [`MUTATED_PATHS_CAP`]
	/// (oldest evicted). Cleared with `agent_dirty`: once a round verifies, the
	/// artifacts are accepted state and a fresh mutation restarts the set.
	mutated_paths: Vec<String>,
	/// HOW the last `agent_dirty` clearance happened: `true` when only a
	/// read-back (the agent re-reading its own edited artifacts) cleared it,
	/// with no command-shaped check in that round. Read-back is legitimate
	/// verification for artifact work (a doc, a config), but for behavioral
	/// claims it proves only content — the verify-gate needs to know which
	/// kind of evidence blessed the tree instead of inferring it from a raw
	/// action log ([`Detectors::cleared_by_readback_only`]).
	readback_only_clearance: bool,
	/// Command error receipts without a later success for the same tool and
	/// arguments. Their relevance to the task is unknown to this detector.
	failed_commands: HashSet<u64>,
	/// Rounds containing command errors while the ledger above is non-empty.
	/// Reset when all recorded errors have later successes, or after a hint.
	failed_command_rounds: usize,
}

/// A recorded pattern worth considering, not a verdict on task correctness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorSignal {
	/// Nothing notable.
	None,
	/// Identical calls and results repeated `loop_threshold` times without
	/// current-round novelty or a successful mutation.
	Loop,
	/// `no_progress_window` actions elapsed with zero new information.
	NoProgress,
	/// Several command executions returned errors without later successes for
	/// the same calls. They may be expected probes, not failed verification.
	Recovery,
}

impl DetectorSignal {
	/// Severity rank — higher wins when merging signals from a parallel batch.
	/// Mirrors the priority in `record_round_signals`'s return cascade.
	fn priority(self) -> u8 {
		match self {
			Self::None => 0,
			Self::Recovery => 1,
			Self::NoProgress => 2,
			Self::Loop => 3,
		}
	}

	/// Merge two signals from the same parallel batch — keep the higher-priority one.
	pub fn merge(self, other: Self) -> Self {
		if other.priority() > self.priority() {
			other
		} else {
			self
		}
	}
}

fn hash2(a: &str, b: &str) -> u64 {
	let mut h = DefaultHasher::new();
	a.hash(&mut h);
	b.hash(&mut h);
	h.finish()
}

impl Detectors {
	/// Fold ONE call's result into per-call state and return `(result_hash, novel)`
	/// for the caller to aggregate into the round. Updates only genuinely per-result
	/// state: the seen-set (novelty memory across time). It decides NO signal —
	/// every signal is a per-ROUND verdict, because a parallel batch is one model
	/// decision (see [`Detectors::record_round_signals`]).
	pub fn note_call(
		&mut self,
		tool: &str,
		parameters: &serde_json::Value,
		result: &str,
		is_error: bool,
		is_mutation: bool,
	) -> (u64, bool) {
		// Equal output from different requests is not repetition: each target
		// may have independently returned the same answer or write receipt.
		let mut h = DefaultHasher::new();
		tool.hash(&mut h);
		parameters.to_string().hash(&mut h);
		result.hash(&mut h);
		is_error.hash(&mut h);
		let rhash = h.finish();

		// Novelty: fresh = result content not seen in the recent window. Recorded
		// per result (memory is per-result), but the novelty SIGNAL is per round.
		let fresh = self.seen.insert(rhash);
		if fresh {
			self.seen_order.push_back(rhash);
			if self.seen_order.len() > SEEN_CAP {
				if let Some(old) = self.seen_order.pop_front() {
					self.seen.remove(&old);
				}
			}
		}
		// A new error can answer an exploratory question. A repeated failed
		// write is not evidence of progress merely because it was write-capable.
		let novel = fresh || (!is_error && is_mutation);
		(rhash, novel)
	}

	/// Decide the deterministic signal for ONE completed tool round. A parallel batch
	/// is ONE model decision, so the whole round is observed as a single unit — N
	/// identical calls in one shot count once, not N. Inputs are aggregated across
	/// the round by the caller: `call_hashes` are the per-call result hashes (from
	/// [`Detectors::note_call`]). Returns the highest-priority fired signal.
	pub fn record_round_signals(
		&mut self,
		call_hashes: &[u64],
		round_novel: bool,
		loop_threshold: usize,
		no_progress_window: usize,
	) -> DetectorSignal {
		// Round identity for Loop: the multiset of result hashes, order-independent
		// (parallel call order carries no meaning). The same batch re-issued round
		// after round hashes identically; 3 identical calls in ONE round are a single
		// entry, so they can't trip the loop threshold on their own.
		let round_hash = {
			let mut hs = call_hashes.to_vec();
			hs.sort_unstable();
			let mut h = DefaultHasher::new();
			hs.hash(&mut h);
			h.finish()
		};

		// Loop window: identical ROUND repeated.
		self.loop_window.push_back(round_hash);
		while self.loop_window.len() > loop_threshold.max(1) {
			self.loop_window.pop_front();
		}
		let looping = !round_novel
			&& loop_threshold > 0
			&& self.loop_window.len() >= loop_threshold
			&& self.loop_window.iter().all(|&h| h == round_hash);

		// Novelty window: ROUNDS without any new information (a round is novel if any
		// of its calls produced something fresh).
		self.novelty_window.push_back(round_novel);
		while self.novelty_window.len() > no_progress_window.max(1) {
			self.novelty_window.pop_front();
		}
		let stalled = no_progress_window > 0
			&& self.novelty_window.len() >= no_progress_window
			&& self.novelty_window.iter().all(|&n| !n);

		// Priority cascade — mirrors DetectorSignal::priority (Loop > NoProgress).
		if looping {
			DetectorSignal::Loop
		} else if stalled {
			DetectorSignal::NoProgress
		} else {
			DetectorSignal::None
		}
	}

	/// Record the artifact paths a successful mutation-shaped call touched —
	/// the identities a later read-back can verify ([`Detectors::is_readback_call`]).
	/// Called per successful mutation call; deduped and capped (oldest evicted).
	pub fn note_mutated_paths(&mut self, parameters: &serde_json::Value) {
		for p in param_paths(parameters) {
			let n = normalize_path(&p);
			if n.is_empty() {
				continue;
			}
			if self.mutated_paths.contains(&n) {
				continue;
			}
			if self.mutated_paths.len() >= MUTATED_PATHS_CAP {
				self.mutated_paths.remove(0);
			}
			self.mutated_paths.push(n);
		}
	}

	/// Fold command outcomes into a bounded error ledger. A later success for
	/// the same call removes its receipt; unrelated results leave it intact.
	/// After `threshold` rounds with errors, nominate one advisory hint and
	/// restart its counter. This does not establish a failed behavior or require
	/// recovery. `threshold == 0` disables the signal.
	pub fn record_round_command_outcomes(
		&mut self,
		outcomes: &[(u64, bool)],
		threshold: usize,
	) -> DetectorSignal {
		if outcomes.is_empty() {
			return DetectorSignal::None;
		}
		let mut failed = HashSet::new();
		let mut succeeded = HashSet::new();
		for &(key, success) in outcomes {
			if success {
				succeeded.insert(key);
			} else {
				failed.insert(key);
			}
		}

		// A parallel batch with conflicting outcomes for the same check is not a
		// clearance. Only unambiguously successful checks discharge prior debt.
		for key in succeeded.difference(&failed) {
			self.failed_commands.remove(key);
		}
		if !failed.is_empty() {
			self.failed_command_rounds = self.failed_command_rounds.saturating_add(1);
			for key in failed {
				if self.failed_commands.len() < FAILED_COMMANDS_CAP
					|| self.failed_commands.contains(&key)
				{
					self.failed_commands.insert(key);
				}
			}
		}
		if self.failed_commands.is_empty() {
			self.failed_command_rounds = 0;
		}
		if threshold > 0 && self.failed_command_rounds >= threshold {
			self.failed_command_rounds = 0;
			DetectorSignal::Recovery
		} else {
			DetectorSignal::None
		}
	}

	/// Is this successful non-mutation call a READ-BACK of an artifact the agent
	/// itself mutated — inspecting the resulting state, the correct verification
	/// for work with no command to run (documents, config, prose, data files)?
	/// Domain-agnostic by construction: it matches artifact identity (the path
	/// the agent changed), never tool names or file types. Command-verifiable
	/// work still prefers the stronger signal — a check run — but a read-back
	/// is evidence of artifact inspection and must count as such.
	pub fn is_readback_call(
		&self,
		parameters: &serde_json::Value,
		is_mutation: bool,
		is_error: bool,
	) -> bool {
		if is_mutation || is_error || self.mutated_paths.is_empty() {
			return false;
		}
		param_paths(parameters)
			.iter()
			.map(|p| normalize_path(p))
			.any(|n| self.mutated_paths.contains(&n))
	}

	/// Fold one completed tool ROUND into the observational verification state.
	/// `fp_before`/`fp_after` are workdir fingerprints measured around the round
	/// (`None` = unavailable, e.g. not a git repo). `verifier_ok` = some
	/// successful call in the round was verifier-shaped ([`is_verifier_shaped`]);
	/// `readback_ok` = some successful call read back an artifact the agent
	/// itself mutated ([`Detectors::is_readback_call`]); `mutation_ok` = some
	/// successful call was mutation-shaped (the no-fingerprint fallback signal).
	///
	/// A round VERIFIES only when a verifier or read-back ran on an unchanged
	/// tree — a "verifier" that also dirtied the tree (or ran in the same
	/// parallel batch as an edit) checked an ambiguous state and proves nothing.
	///
	/// `delegated_ok` is the one exception, and it is not a relaxation: a
	/// subagent handoff collapses the child's whole trajectory (change, THEN
	/// check) into a single parent round, so `tree_unchanged` is false by
	/// construction and can never be satisfied however diligent the child was.
	/// The child measures its own tree with this same code one level down, so
	/// the caller passes its verdict up (see [`crate::supervisor::delegate`])
	/// and it stands in for the tree check for that round only.
	///
	/// `write_capable` = the round carried at least one call that COULD have
	/// moved the tree: mutation-shaped, command-executing (an edit hides inside
	/// a shell command, and a command may write before erroring), or a delegated
	/// subagent run. A round of pure reads cannot have caused the movement, so a
	/// fingerprint that drifts across it is a concurrent writer (the user's
	/// editor, a dev server, a generated artifact) — attributing that to the
	/// agent armed the mutation pre-gate on observe-only jobs (review/audit),
	/// which then demanded a check run for work that changed nothing.
	#[allow(clippy::too_many_arguments)]
	pub fn note_round_verification(
		&mut self,
		fp_before: Option<u64>,
		fp_after: Option<u64>,
		verifier_ok: bool,
		readback_ok: bool,
		mutation_ok: bool,
		delegated_ok: bool,
		write_capable: bool,
	) {
		// First observation seeds the baseline: the task-start tree is, by
		// definition, the last state the user accepted.
		if self.verified_fp.is_none() {
			if let Some(b) = fp_before {
				self.verified_fp = Some(b);
			}
		}
		let tree_unchanged = match (fp_before, fp_after) {
			(Some(a), Some(b)) => a == b,
			// No fingerprints: fall back to call shape.
			_ => !mutation_ok,
		};
		// The child's verdict covers only what the child did. If the PARENT also
		// ran a mutation in the same round, that edit was never inside the
		// child's tree check and must not ride in on its verdict.
		let delegated = delegated_ok && !mutation_ok;
		if delegated || ((verifier_ok || readback_ok) && tree_unchanged) {
			if let Some(a) = fp_after {
				self.verified_fp = Some(a);
			}
			// Record the evidence KIND: read-back-only clearance means no
			// command-shaped check has succeeded since the last mutation.
			// Only meaningful while the agent had something to verify.
			if self.agent_dirty {
				self.readback_only_clearance = readback_ok && !verifier_ok && !delegated;
			}
			self.agent_dirty = false;
			self.mutated_paths.clear();
		} else if !tree_unchanged && write_capable {
			self.agent_dirty = true;
		}
		crate::log_debug!(
			"round verification: tree_unchanged={} verifier={} readback={} delegated={} write_capable={} -> verified_fp={:?} agent_dirty={}",
			tree_unchanged,
			verifier_ok,
			readback_ok,
			delegated,
			write_capable,
			self.verified_fp,
			self.agent_dirty
		);
	}

	/// Reset per-task detector state on a new genuine user turn. Rolling
	/// windows and the unverified-mutation latch must not cross task boundaries;
	/// the verified fingerprint remains as the accepted working-tree baseline.
	pub fn reset_streak(&mut self) {
		self.novelty_window.clear();
		self.loop_window.clear();
		self.agent_dirty = false;
		self.mutated_paths.clear();
		self.readback_only_clearance = false;
		self.failed_commands.clear();
		self.failed_command_rounds = 0;
	}

	/// Heuristic signal: an agent round changed the tree and no qualifying
	/// verification has been recognized since. This is not a completion verdict.
	/// Armed ONLY by the agent's own rounds
	/// (`agent_dirty`) — an agent that changed nothing is reporting, not
	/// claiming work, and has nothing to verify, however much the tree drifts
	/// externally. `fp_now` is the live fingerprint measured at decision time;
	/// it clears this signal when the tree is back at its last verified
	/// state (e.g. the change was reverted).
	pub fn needs_verification(&self, fp_now: Option<u64>) -> bool {
		let r = self.agent_dirty
			&& match (fp_now, self.verified_fp) {
				(Some(now), Some(verified)) => now != verified,
				_ => true,
			};
		crate::log_debug!(
			"needs_verification: fp_now={:?} verified_fp={:?} agent_dirty={} -> {}",
			fp_now,
			self.verified_fp,
			self.agent_dirty,
			r
		);
		r
	}

	/// Was the last dirty-state clearance a read-back only — the agent re-read
	/// its own edited artifacts, with no command-shaped check succeeding since
	/// the last mutation? Verification-evidence provenance for the verify-gate.
	pub fn cleared_by_readback_only(&self) -> bool {
		self.readback_only_clearance
	}
}

/// Whether an advisory is useful on this response. Legitimate handbacks are
/// left alone; the completion gate separately handles a `done` claim.
pub fn should_steer(signal: DetectorSignal, report: Option<SelfReport>) -> bool {
	if signal == DetectorSignal::None {
		return false;
	}
	match report {
		Some(SelfReport::Done | SelfReport::Blocked | SelfReport::NeedInput) => false,
		Some(SelfReport::Exploring) if signal == DetectorSignal::NoProgress => false,
		_ => true,
	}
}

/// Describe only the recorded signal, never a conclusion about task progress.
pub fn signal_description(signal: DetectorSignal) -> &'static str {
	match signal {
		DetectorSignal::Loop => "identical calls returned identical results",
		DetectorSignal::NoProgress => "recent call results repeated",
		DetectorSignal::Recovery => "several command executions returned errors",
		DetectorSignal::None => "",
	}
}

/// Counter signals are advisory at every repetition count. The main model
/// sees the actual tool receipts and user task; it can decide whether polling,
/// negative probes or retries are useful. Repetition never upgrades a heuristic
/// into evidence of failure or permission to disregard the user's instructions.
pub fn steer_note(signal: DetectorSignal) -> &'static str {
	match signal {
		DetectorSignal::Loop => "<pay-attention>\nAdvisory: identical calls have returned identical results across several rounds. This may be intentional polling or repeated observation, not a failure. Continue if these calls serve the user's task; consider a different approach only if the observed results show it is needed. This hint does not require extra work or a blocked handback.\n</pay-attention>",
		DetectorSignal::NoProgress => "<pay-attention>\nAdvisory: recent calls have repeated previously observed results. The detector cannot determine whether the task is advancing. Use the actual outcomes and the user's request to decide the next step; continue the current approach when justified. This hint does not require extra work or a blocked handback.\n</pay-attention>",
		DetectorSignal::Recovery => "<pay-attention>\nAdvisory: several command executions returned errors without a later success for those exact calls. These may be expected probes, obsolete attempts or failures unrelated to verification. Judge their relevance from the actual outputs and the user's task. Only an observed, relevant failure warrants recovery work; this hint does not require rerunning a command, changing code or reporting blocked. Honor all execution restrictions.\n</pay-attention>",
		DetectorSignal::None => "",
	}
}

/// Order-independent hash of a round's tool calls, keyed on each call's CHOSEN identity
/// (`tool_name` + `parameters`) — NOT its result. Used only to reduce repeated
/// advisory messages for the same calls, never as proof of non-compliance. `tool_id` is a per-call
/// unique id and is excluded so the same calls hash equal across rounds. Parameter JSON
/// is key-order-canonical (serde_json `Value` is BTreeMap-backed here), so equal calls
/// always hash equal.
///
/// Different arguments bypass duplicate-hint throttling because the runtime
/// cannot establish equivalence. They still receive only advisory context.
pub fn call_set_hash(calls: &[crate::mcp::McpToolCall]) -> u64 {
	let mut per_call: Vec<u64> = calls
		.iter()
		.map(|c| hash2(&c.tool_name, &c.parameters.to_string()))
		.collect();
	per_call.sort_unstable();
	let mut h = DefaultHasher::new();
	per_call.hash(&mut h);
	h.finish()
}

#[cfg(test)]
#[path = "detect_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "detect_tests.rs"]
mod unit_tests;
