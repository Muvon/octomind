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
//! Two free signals are fused before any model is woken:
//! 1. **Self-report** — the agent annotates each turn with a `<sup>state</sup>`
//!    token (it already knows whether it is exploring / stuck / done).
//! 2. **Novelty counters** — derived from a single primitive: did this action
//!    add *new information* to the agent's state? Loop = the same result repeats;
//!    no-progress = a window of actions with zero novelty.
//!
//! Agreement needs no model. Only a *conflict* (e.g. counter says "no progress"
//! while the agent reports `progressing`) is worth the rare model confirmation.

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
`<sup>{"state":"STATE","focus":"current subgoal and why","next":"next action","carry":["minimum fact or opaque reference needed after context loss"],"plan":null}</sup>`
Use valid single-line JSON with exactly those fields. `carry` may be empty and `next` is `null` when nothing remains to do; keep only information genuinely needed to resume. Never copy credentials or secret values into the report — retain only an opaque pointer, name, or location used to obtain them. Avoid generic text such as "working" or "continuing". STATE must be exactly one of:
- `exploring` — still gathering context, reading code
- `progressing` — actively making changes
- `blocked` — stuck, cannot proceed
- `need_input` — asking the user a question and waiting on them
- `done` — the user's task is fully complete

`plan` is normally `null`. Set it to `"request"` once, alongside real work, only when the task clearly needs 3+ dependent outcomes or durable tracking. With an injected plan, use `"phase_complete"` alongside the next work batch only after the current outcome is evidenced, or `"reassess"` when evidence invalidates the remaining route. The external manager owns the plan; never emit a response only for planning.
Example: `<sup>{"state":"progressing","focus":"checking the active operation","next":"perform the next status check","carry":["use the resource reference established earlier"],"plan":null}</sup>`
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
/// tool itself must not declare mutation intent, and it must not belong to one
/// of octomind's own builtin control-plane servers (authoritative: resolved via
/// the same registry the dispatcher routes with — `plan` takes a `command`
/// parameter too, but the runtime knows it executes nothing). Whether the
/// round actually verified is then decided OBSERVATIONALLY in
/// [`Detectors::note_round_verification`]: a candidate that dirtied the tree
/// is a mutator, not a verifier.
pub fn is_verifier_shaped(tool: &str, parameters: &serde_json::Value) -> bool {
	let Some(cmd) = parameters.get("command").and_then(|v| v.as_str()) else {
		return false;
	};
	// A tool whose NAME declares mutation intent is never a verification
	// candidate, whatever its parameter shape: editor tools also take a string
	// `command` (octofs text_editor's command="str_replace" selects an edit
	// operation, it executes nothing) — without this guard an edit round
	// classified itself as its own verifier.
	if is_mutation_call(tool, parameters) {
		crate::log_debug!("verifier-shape: {} rejected: mutation tool", tool);
		return false;
	}
	// Reject empty command strings: they execute nothing and cannot validate
	// completion.
	if cmd.trim().is_empty() {
		crate::log_debug!("verifier-shape: {} rejected: empty command", tool);
		return false;
	}
	crate::log_debug!("verifier-shape: {} accepted: {}", tool, cmd);
	match crate::mcp::tool_map::get_tool_server_name(tool) {
		Some(server) => !matches!(
			server.as_str(),
			"core" | "runtime" | "orchestration" | "agent"
		),
		// Unregistered tool with a command param: treat as a candidate — the
		// observational tree check still guards against false verification.
		None => true,
	}
}

/// Stable identity for one command-shaped check. Recovery uses the concrete
/// tool + command pair so a later success discharges only the failure it can
/// actually prove resolved; an unrelated successful command is not progress on
/// that check.
pub fn verifier_key(tool: &str, parameters: &serde_json::Value) -> Option<u64> {
	if !is_verifier_shaped(tool, parameters) {
		return None;
	}
	let command = parameters.get("command")?.as_str()?.trim();
	Some(hash2(tool, command))
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

/// Classify one concrete call. MCP annotations supply the generic cross-domain
/// signal when present; command/action parameters cover multi-operation tools
/// such as editors; normalized intent tokens are the compatibility fallback.
pub fn is_mutation_call(tool: &str, parameters: &serde_json::Value) -> bool {
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
	if contains_mutation_intent(tool) {
		return true;
	}
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

const SEEN_CAP: usize = 128;

/// Identical result this many times in a row → loop fired.
pub const LOOP_THRESHOLD: usize = 3;

/// Rounds without new information → no-progress fired. Also the bounded
/// failure budget for the recovery signal (failed command-shaped checks).
pub const NO_PROGRESS_WINDOW: usize = 5;

/// Cap on remembered agent-mutated paths (read-back verification candidates).
/// Oldest evicted — a task touching more artifacts than this verifies via the
/// most recent ones, which is where the read-back lands anyway.
const MUTATED_PATHS_CAP: usize = 32;

/// Cap on distinct command-shaped checks that have failed without a later
/// success from the same check. Recovery tracking is a small current-turn
/// ledger, not an unbounded command history.
const FAILED_VERIFIERS_CAP: usize = 64;

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
	/// `agent_dirty` is armed, the pre-gate compares the live fingerprint
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
	/// Command-shaped checks that failed and have not subsequently succeeded
	/// with the same tool + command identity. An unrelated successful read,
	/// diff, or probe must not erase a failed behavioral check.
	failed_verifiers: HashSet<u64>,
	/// Failed verifier rounds accumulated while the ledger above remains
	/// unresolved. Counted per round because a parallel batch is one model
	/// decision; reset when all failed checks are discharged or after emitting
	/// a recovery steer.
	failed_verifier_rounds: usize,
}

/// What the deterministic layer concluded for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorSignal {
	/// Nothing notable.
	None,
	/// The same result repeated `loop_threshold` times — even across reworded
	/// args (keyed on result, so near-duplicate calls are caught too).
	Loop,
	/// `no_progress_window` actions elapsed with zero new information.
	NoProgress,
	/// Repeated command-shaped checks have failed without the same checks later
	/// succeeding. Unlike generic no-progress, unrelated fresh reads cannot hide
	/// this unresolved recovery episode.
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
		result: &str,
		is_error: bool,
		is_mutation: bool,
	) -> (u64, bool) {
		// Identity of this action's RESULT, keyed on tool+result so the same
		// output from differently-worded calls still reads as a repeat.
		let rhash = hash2(tool, result);

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
		let novel = is_mutation || (!is_error && fresh);
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
		let looping = loop_threshold > 0
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

	/// Fold command-shaped verification outcomes into an unresolved-failure
	/// ledger. A failed check is discharged only when that same tool + command
	/// later succeeds; unrelated successful calls do not prove the failed
	/// behavior. Once `threshold` failed verifier rounds accumulate, emit one
	/// recovery signal and restart only the emission counter while retaining the
	/// unresolved ledger. `threshold == 0` disables the signal.
	pub fn record_round_verifier_outcomes(
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
			self.failed_verifiers.remove(key);
		}
		if !failed.is_empty() {
			self.failed_verifier_rounds = self.failed_verifier_rounds.saturating_add(1);
			for key in failed {
				if self.failed_verifiers.len() < FAILED_VERIFIERS_CAP
					|| self.failed_verifiers.contains(&key)
				{
					self.failed_verifiers.insert(key);
				}
			}
		}
		if self.failed_verifiers.is_empty() {
			self.failed_verifier_rounds = 0;
		}
		if threshold > 0 && self.failed_verifier_rounds >= threshold {
			self.failed_verifier_rounds = 0;
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
	/// work still prefers the stronger exit — a check run — but a read-back is
	/// exactly what the pre-gate note asks for ("inspect the resulting state"),
	/// so it must count.
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
		self.failed_verifiers.clear();
		self.failed_verifier_rounds = 0;
	}

	/// Free pre-gate signal: an agent round changed the tree and nothing has
	/// been run since to check it. Armed ONLY by the agent's own rounds
	/// (`agent_dirty`) — an agent that changed nothing is reporting, not
	/// claiming work, and has nothing to verify, however much the tree drifts
	/// externally. `fp_now` is the live fingerprint measured at decision time;
	/// it stands the gate down when the tree is back at its last verified
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

/// Fuse the deterministic signal with the agent's free self-report (no model
/// call). The decision table:
/// - any `done`                          → defer to the verify-gate (no steer)
/// - no-progress while `exploring`      → wait (legitimate exploration)
/// - loop, no-progress                   → steer
pub fn should_steer(signal: DetectorSignal, report: Option<SelfReport>) -> bool {
	if signal == DetectorSignal::None {
		return false;
	}
	match report {
		Some(SelfReport::Done) => false,
		// No-progress can be legitimate while exploring; every other signal steers
		// regardless of intent.
		Some(SelfReport::Exploring) if signal == DetectorSignal::NoProgress => false,
		_ => true,
	}
}

/// Short human description of a fired signal — for the user-facing
/// `· Supervisor: steering — …` notice.
pub fn signal_description(signal: DetectorSignal) -> &'static str {
	match signal {
		DetectorSignal::Loop => "repeated action without new results",
		DetectorSignal::NoProgress => "no new information in recent steps",
		DetectorSignal::Recovery => {
			"verification keeps failing — unresolved checks need a different recovery strategy"
		}
		DetectorSignal::None => "",
	}
}

/// Shared persistent-failure frame: the model has been steered through the full
/// 0→1→2 ladder on a *stuck* signal and still has not broken out, so small tweaks are
/// clearly not working. Signal-agnostic and held on clamp.
///
/// POLYMORPHIC by design: the persistent frame is re-emitted on the backoff schedule
/// (attempts 3,4,6,10,…), and a *verbatim* repeat of a warning loses effect within 2-3
/// exposures (habituation / repetition-suppression — Ancker 2017 measures ~30% drop in
/// acceptance per identical repeat; Anderson 2015 CHI shows polymorphic warnings resist
/// it). So we rotate equally-firm rephrasings by attempt index — each re-emit is a fresh
/// stimulus that re-recruits attention. Derived from the counter, so still parameter-free.
/// All variants carry the same firm ask (a fundamentally different path, or report
/// `blocked`) so callers/tests can rely on the invariant content.
const PERSISTENT_VARIANTS: &[&str] = &[
	"<pay-attention>\nYou have been steered several times here and have not broken out — small adjustments are not working. Stop iterating on the same approach: either take a fundamentally different path to the goal, or report `blocked` and name the single obstacle in your way.\n</pay-attention>",
	"<pay-attention>\nSame approach, same wall — the repeated nudges have not changed the outcome. Do not retry a near-identical call again. Either switch to a fundamentally different strategy (a different tool, scope, or sub-goal), or stop and report `blocked` with the one concrete thing standing in your way.\n</pay-attention>",
	"<pay-attention>\nYou are repeating work that has not moved the task despite several course-corrections. Pause and decide, in one line, the single obstacle in your way. If a fundamentally different path to the goal exists, take it now; if it does not, report `blocked` instead of trying the same thing again.\n</pay-attention>",
];

/// Conflict framing: a no-progress signal while the agent self-reports
/// `progressing`. The counters and the self-assessment disagree — the canonical
/// reason the supervisor escalates at all — so name the contradiction directly
/// instead of the generic no-progress note. Same 0→1→2 escalation.
const CONFLICT_VARIANTS: &[&str] = &[
	"<pay-attention>\nYou reported you are making progress, but the last several actions added nothing new — your self-assessment and what the actions show disagree. Check which is right before continuing.\n</pay-attention>",
	"<pay-attention>\nYou report progressing, yet no new information has appeared. Name in one line the concrete result your recent steps produced. If you cannot, the work has stalled — take a single different step that visibly moves the goal, not another like the ones that yielded nothing.\n</pay-attention>",
	"<pay-attention>\nYour actions are not advancing the task despite a `progressing` report. Re-anchor: state the goal, what is actually done, and the one next step that moves it — then take it. If nothing does, report `blocked` with what is missing.\n</pay-attention>",
];

/// The advisory steer note for a fired signal. Out-of-band; the `<pay-attention>`
/// framing keeps it distinct from user content. Wording is positive-forward (the
/// concrete action to take, not a bare prohibition) and puts that action last, in
/// the recency slot — negation and buried directives are the empirically weakest
/// forms for instruction-following.
///
/// `attempt` rotates the *framing* when the same signal re-fires without the model
/// breaking out. Re-sending identical text loses salience (habituation), so each
/// retry reframes the same constraint from a different angle:
///   0 → diagnostic (what is happening; soft reconsider)
///   1 → directive  (a grounded one-line self-check + the concrete alternative)
///   2 → stop       (firm: a different approach now, or report `blocked`)
///  3+ → persistent ([`PERSISTENT_VARIANTS`]: fundamentally different path or `blocked`)
/// Advance-then-clamp, not modulo: never soften once the model has proven it is
/// stuck — hold the firmest frame. `report` lets a no-progress signal switch to
/// [`CONFLICT_VARIANTS`] when the agent insists it is `progressing`.
pub fn steer_note(
	signal: DetectorSignal,
	report: Option<SelfReport>,
	attempt: usize,
) -> &'static str {
	// Ladder exhausted on a stuck signal without breakout → hold the firmest frame, but
	// rotate its phrasing each re-emit so the repeated nudge does not habituate (see
	// PERSISTENT_VARIANTS), keyed on how far past the ladder we are.
	if is_stuck(signal) && attempt >= PERSISTENT_ATTEMPT {
		return PERSISTENT_VARIANTS[(attempt - PERSISTENT_ATTEMPT) % PERSISTENT_VARIANTS.len()];
	}
	// Counters say no-progress while the agent reports progressing: name the conflict.
	if signal == DetectorSignal::NoProgress && report == Some(SelfReport::Progressing) {
		return CONFLICT_VARIANTS[attempt.min(CONFLICT_VARIANTS.len() - 1)];
	}
	let variants: &[&str] = match signal {
		DetectorSignal::Loop => &[
			"<pay-attention>\nThis result is identical to one already in your context — the last call added nothing, so the current approach has stalled. Reconsider what is actually blocking progress before the next call.\n</pay-attention>",
			"<pay-attention>\nSame result again — you are repeating a call that already failed to advance the task. In one sentence, name why it failed. Then change one concrete thing on the next call — a different tool, different arguments, or a different sub-goal — that approaches the goal a new way.\n</pay-attention>",
			"<pay-attention>\nThis is a loop: the same call keeps returning the same result. Make a different call that approaches the goal another way — a different tool, scope, or sub-goal — or report `blocked` with the one obstacle stopping you.\n</pay-attention>",
		],
		DetectorSignal::NoProgress => &[
			"<pay-attention>\nThe last few steps surfaced nothing new — this line of inquiry looks exhausted. Consider whether it can still reach what you need.\n</pay-attention>",
			"<pay-attention>\nStill nothing new. Name in one line what you still need but have not found, then take a single concrete step toward the goal using what you already know — a decision or an action, not another exploratory probe.\n</pay-attention>",
			"<pay-attention>\nThis exploration has stalled. Re-anchor on the user's actual request: state the goal in one line, what is done, and the one next step that delivers it — then take it. If no such step exists, report `blocked` with what is missing.\n</pay-attention>",
		],
		DetectorSignal::Recovery => &[
			"<pay-attention>\nSeveral command-shaped checks have failed, and unrelated successful calls do not resolve them. Use the latest failure to isolate one concrete cause, change that cause, then rerun the narrowest check that proves it. Do not repeat a broad check until relevant state has changed.\n</pay-attention>",
			"<pay-attention>\nThe verification failures remain unresolved. Stop broad trial-and-error: name the single failing behavior you are fixing now, trace it to its owning source, make one focused correction, and run the smallest check that can confirm or reject that correction.\n</pay-attention>",
			"<pay-attention>\nThis recovery strategy is still producing failed checks. Re-anchor on the latest concrete failure and take a fundamentally different diagnostic or implementation path. Continue only with a focused cause-and-check loop, or report the specific blocker instead of accumulating more broad retries.\n</pay-attention>",
		],
		DetectorSignal::None => return "",
	};
	variants[attempt.min(variants.len() - 1)]
}

/// The "stuck" signal class — every real-waste failure mode. These escalate to
/// [`PERSISTENT_VARIANTS`]; factored so the steer loop and the escalation
/// ladder classify signals the same way.
fn is_stuck(signal: DetectorSignal) -> bool {
	matches!(
		signal,
		DetectorSignal::Loop | DetectorSignal::NoProgress | DetectorSignal::Recovery
	)
}

/// The escalation rung at which a stuck signal stops reframing and holds the firmest
/// [`PERSISTENT_VARIANTS`] frame — and the earliest rung at which the critical-signal
/// de-spam cooldown may begin (the full 0→1→2 ladder plus one persistent frame have all
/// been delivered by then).
pub const PERSISTENT_ATTEMPT: usize = 3;

/// Order-independent hash of a round's tool calls, keyed on each call's CHOSEN identity
/// (`tool_name` + `parameters`) — NOT its result. This is the discriminator between a
/// model IGNORING a steer (re-issues the byte-identical call-set) and one TRYING (a
/// different call, even if it still trips the same detector). `tool_id` is a per-call
/// unique id and is excluded so the same calls hash equal across rounds. Parameter JSON
/// is key-order-canonical (serde_json `Value` is BTreeMap-backed here), so equal calls
/// always hash equal.
///
/// Known limit (accepted): cosmetic param churn — a model thrashing to *look* like it is
/// trying — evades the THROTTLE but not the same-signal frame escalation nor the
/// circuit-breaker ceiling. Closing it would need an LLM judge, which violates the
/// free/deterministic contract, so we keep the cheap exact gate and let the breaker backstop.
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
mod tests {
	use super::*;

	impl Detectors {
		/// Test shim: run ONE call through the real two-phase path as a single-call
		/// round (note_call → record_round_signals) and return the round signal. Lets
		/// the existing per-call tests exercise the new per-round code unchanged.
		#[allow(clippy::too_many_arguments)]
		fn record_action(
			&mut self,
			tool: &str,
			result: &str,
			is_error: bool,
			is_mutation: bool,
			loop_threshold: usize,
			no_progress_window: usize,
		) -> DetectorSignal {
			let (rhash, novel) = self.note_call(tool, result, is_error, is_mutation);
			self.record_round_signals(&[rhash], novel, loop_threshold, no_progress_window)
		}
	}

	#[test]
	fn parses_state_only() {
		assert_eq!(
			parse_self_report("work\n<sup>done</sup>"),
			Some((SelfReport::Done, None))
		);
	}

	#[test]
	fn parses_state_with_reason() {
		let r = parse_self_report("x <sup>progressing · editing api</sup> y");
		assert_eq!(
			r,
			Some((SelfReport::Progressing, Some("editing api".into())))
		);
	}

	#[test]
	fn parses_structured_handoff_and_strips_it() {
		let text = r#"answer
<sup>{"state":"progressing","focus":"inspect the active state","next":"continue from the last verified checkpoint","carry":["credential source is configured externally","retain opaque-run-ref"]}</sup>"#;
		let parsed = parse_self_report_handoff(text).expect("structured report");
		assert_eq!(parsed.state, SelfReport::Progressing);
		assert_eq!(parsed.handoff.focus, "inspect the active state");
		assert_eq!(
			parsed.handoff.next,
			"continue from the last verified checkpoint"
		);
		assert_eq!(parsed.handoff.carry.len(), 2);
		assert_eq!(parsed.plan, None);
		assert_eq!(strip_self_report(text), "answer");
	}

	#[test]
	fn parses_external_plan_signal_without_plan_content() {
		let text = r#"<sup>{"state":"progressing","focus":"surveying sources","next":"compare findings","carry":[],"plan":"request"}</sup>"#;
		let parsed = parse_self_report_handoff(text).expect("structured report");
		assert_eq!(
			parsed.plan,
			Some(crate::supervisor::plan::PlanSignal::Request)
		);
	}

	#[test]
	fn malformed_structured_handoff_is_not_accepted_as_status() {
		// Rejected as a status (no `carry`) — but still never shown to the user.
		let malformed = r#"<sup>{"state":"progressing","focus":"x"}</sup>"#;
		assert!(parse_self_report_handoff(malformed).is_none());
		assert_eq!(strip_self_report(malformed), "");
		// Truncated mid-token: not parseable at all, still hidden.
		assert_eq!(strip_self_report(r#"a <sup>{"state":"do</sup>"#), "a");
	}

	#[test]
	fn done_report_with_null_next_parses_and_is_hidden() {
		let text = r#"answer
<sup>{"state":"done","focus":"briefed the staged changes","next":null,"carry":["one file left untracked"],"plan":null}</sup>"#;
		let parsed = parse_self_report_handoff(text).expect("null next is a valid done report");
		assert_eq!(parsed.state, SelfReport::Done);
		assert_eq!(parsed.handoff.next, "");
		assert_eq!(strip_self_report(text), "answer");
	}

	#[test]
	fn need_input_variants() {
		assert_eq!(
			parse_self_report("<sup>need_input</sup>").map(|(s, _)| s),
			Some(SelfReport::NeedInput)
		);
	}

	#[test]
	fn strips_token_and_trailing_blank() {
		assert_eq!(strip_self_report("answer\n\n<sup>done</sup>"), "answer");
	}

	#[test]
	fn strips_and_parses_real_multiword_reason() {
		let s = "answer\n<sup>need_input · Phase 1 complete, awaiting user direction</sup>";
		assert_eq!(strip_self_report(s), "answer");
		let (st, reason) = parse_self_report(s).unwrap();
		assert_eq!(st, SelfReport::NeedInput);
		assert_eq!(
			reason.as_deref(),
			Some("Phase 1 complete, awaiting user direction")
		);
	}

	#[test]
	fn handles_non_dot_separators() {
		assert_eq!(strip_self_report("x <sup>done: all good</sup>"), "x");
		assert_eq!(
			strip_self_report("x <sup>blocked - cannot proceed</sup>"),
			"x"
		);
		assert_eq!(
			parse_self_report("<sup>done: all good</sup>").map(|(s, _)| s),
			Some(SelfReport::Done)
		);
	}

	#[test]
	fn keeps_legitimate_superscript() {
		// `<sup>2</sup>` (x squared) is not a state token — keep it verbatim.
		assert_eq!(strip_self_report("x<sup>2</sup> + 1"), "x<sup>2</sup> + 1");
		// A short non-state superscript with no separator stays too.
		assert_eq!(
			strip_self_report("the 5<sup>th</sup>"),
			"the 5<sup>th</sup>"
		);
	}

	#[test]
	fn strips_echoed_state_placeholder() {
		// The reported leak: a model copies the literal `STATE` placeholder.
		// It must never reach the screen, and we recover the intended state.
		assert_eq!(
			strip_self_report("answer\n<sup>STATE · done</sup>"),
			"answer"
		);
		assert_eq!(
			parse_self_report("answer\n<sup>STATE · done</sup>"),
			Some((SelfReport::Done, None))
		);
		// Bare echoed placeholder (no reason) is stripped as well.
		assert_eq!(strip_self_report("answer <sup>STATE</sup>"), "answer");
	}

	#[test]
	fn strips_report_with_unknown_lead_but_separator() {
		// Even a malformed state word can't leak once the `·` separator is present.
		assert_eq!(
			strip_self_report("ok\n<sup>finished · all good</sup>"),
			"ok"
		);
	}

	#[test]
	fn loop_fires_on_repeated_result() {
		let mut d = Detectors::default();
		assert_eq!(
			d.record_action("grep", "same", false, false, 3, 9),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("grep", "same", false, false, 3, 9),
			DetectorSignal::None
		);
		// Third identical RESULT → loop.
		assert_eq!(
			d.record_action("grep", "same", false, false, 3, 9),
			DetectorSignal::Loop
		);
	}

	#[test]
	fn no_progress_fires_on_zero_novelty_window() {
		let mut d = Detectors::default();
		d.record_action("a", "r", false, false, 9, 3); // first "r" → novel
		d.record_action("a", "r", false, false, 9, 3); // seen → not novel
		d.record_action("a", "r", false, false, 9, 3); // not novel
		assert_eq!(
			d.record_action("a", "r", false, false, 9, 3),
			DetectorSignal::NoProgress
		);
	}

	#[test]
	fn mutation_counts_as_progress() {
		let mut d = Detectors::default();
		d.record_action("read", "same", false, false, 9, 2);
		d.record_action("read", "same", false, false, 9, 2);
		// An edit always advances state → breaks the stall.
		assert_eq!(
			d.record_action("edit", "ok", false, true, 9, 2),
			DetectorSignal::None
		);
	}

	#[test]
	fn verification_shape_fallback_without_fingerprints() {
		let mut d = Detectors::default();
		assert!(!d.needs_verification(None));
		// Mutation-shaped round, no verifier → unverified.
		d.note_round_verification(None, None, false, false, true, false, true);
		assert!(d.needs_verification(None));
		// A read-only round changes nothing — looking is not verifying.
		d.note_round_verification(None, None, false, false, false, false, false);
		assert!(d.needs_verification(None));
		// A round where the verifier ran alongside a mutation proves nothing.
		d.note_round_verification(None, None, true, false, true, false, true);
		assert!(d.needs_verification(None));
		// A clean verifier round clears it.
		d.note_round_verification(None, None, true, false, false, false, true);
		assert!(!d.needs_verification(None));
	}

	#[test]
	fn verification_tracks_tree_fingerprint() {
		let mut d = Detectors::default();
		// Round 1 seeds the baseline (10 = task-start tree); the round's edit
		// moved the tree to 11 → unverified.
		d.note_round_verification(Some(10), Some(11), false, false, true, false, true);
		assert!(d.needs_verification(Some(11)));
		// Verifier ran but the same round dirtied the tree (11→12): ambiguous
		// state, proves nothing.
		d.note_round_verification(Some(11), Some(12), true, false, true, false, true);
		assert!(d.needs_verification(Some(12)));
		// Clean verifier on an unchanged tree → verified at 12.
		d.note_round_verification(Some(12), Some(12), true, false, false, false, true);
		assert!(!d.needs_verification(Some(12)));
		// Drift with NO agent round in between is external (the user editing
		// their own tree): the agent changed nothing since its clean
		// verification, so there is nothing for it to verify. Agent-made edits
		// through ANY tool (`shell sed -i` included) are still caught — they
		// move the fingerprint ACROSS their own round, as above.
		assert!(!d.needs_verification(Some(13)));
	}

	#[test]
	fn external_drift_never_arms_verification() {
		let mut d = Detectors::default();
		// Read-only rounds over a tree that drifts externally mid-session — the
		// observe-only job shape (review/brief/audit): the deliverable is a
		// report, and a done-claim needs no check run.
		d.note_round_verification(Some(10), Some(10), false, false, false, false, false);
		assert!(!d.needs_verification(Some(11)));
		// Drift DURING a pure-read round (a concurrent editor, a dev server, a
		// generated artifact moving the tree while the agent only views and
		// searches): no call could have written, so the movement is external
		// and must not arm — this is what falsely gated read-only jobs.
		d.note_round_verification(Some(11), Some(12), false, false, false, false, false);
		assert!(!d.needs_verification(Some(12)));
		// A write-capable round that moved the tree arms it, even when no call
		// was mutation-shaped (an edit hidden inside a shell command).
		d.note_round_verification(Some(12), Some(13), false, false, false, false, true);
		assert!(d.needs_verification(Some(13)));
	}

	#[test]
	fn delegated_verification_clears_a_round_that_changed_the_tree() {
		let mut d = Detectors::default();
		// An orchestrator's `tap run`: the specialist edited AND checked inside
		// this one parent round, so the tree moved and no parent call could ever
		// be verifier-shaped. Without the child's verdict this latches dirty
		// forever and every `done` re-triggers the mutation pre-gate. A round
		// with delegated runs is always write-capable — the child can write
		// through any tool of its own.
		d.note_round_verification(Some(10), Some(11), false, false, false, false, true);
		assert!(d.needs_verification(Some(11)));
		// Same round shape, child reported verified → accepted, and the
		// post-round tree becomes the new baseline.
		let mut d = Detectors::default();
		d.note_round_verification(Some(10), Some(11), false, false, false, true, true);
		assert!(!d.needs_verification(Some(11)));
	}

	#[test]
	fn delegated_verification_does_not_cover_the_parents_own_edit() {
		let mut d = Detectors::default();
		// Parallel round: a verified subagent alongside the parent's own
		// mutation-shaped call. The child never checked the parent's edit, so
		// its verdict must not clear the round.
		d.note_round_verification(Some(10), Some(11), false, false, true, true, true);
		assert!(d.needs_verification(Some(11)));
	}

	#[test]
	fn readback_of_mutated_path_verifies_artifact_work() {
		use serde_json::json;
		let mut d = Detectors::default();
		// Round 1: agent edits a doc — mutation round, tree moves, dirty.
		d.note_mutated_paths(&json!({"path": "blog/post/index.md"}));
		d.note_round_verification(Some(10), Some(11), false, false, true, false, true);
		assert!(d.needs_verification(Some(11)));
		// Round 2: agent re-reads the exact artifact it changed — that IS the
		// verification for work with no command to run.
		let readback = d.is_readback_call(
			&json!({"path": "blog/post/index.md", "start": 85}),
			false,
			false,
		);
		assert!(readback);
		d.note_round_verification(Some(11), Some(11), false, readback, false, false, false);
		assert!(!d.needs_verification(Some(11)));
	}

	#[test]
	fn readback_requires_matching_path_success_and_no_mutation() {
		use serde_json::json;
		let mut d = Detectors::default();
		d.note_mutated_paths(&json!({"path": "a.md"}));
		// Different artifact → not a read-back.
		assert!(!d.is_readback_call(&json!({"path": "b.md"}), false, false));
		// Mutation call re-touching the path is more editing, not verification.
		assert!(!d.is_readback_call(&json!({"path": "a.md"}), true, false));
		// Failed read proves nothing.
		assert!(!d.is_readback_call(&json!({"path": "a.md"}), false, true));
		// No mutated paths recorded → nothing to read back.
		let fresh = Detectors::default();
		assert!(!fresh.is_readback_call(&json!({"path": "a.md"}), false, false));
	}

	#[test]
	fn param_paths_collects_pathish_keys_only() {
		use serde_json::json;
		let p = json!({
			"path": "a.md",
			"from_path": "b.rs",
			"files": ["c.py", ""],
			"command": "rm -rf /",
			"content": "path-like text ignored"
		});
		let mut got = param_paths(&p);
		got.sort();
		assert_eq!(got, vec!["a.md", "b.rs", "c.py"]);
		assert!(param_paths(&json!({"command": "x"})).is_empty());
	}

	#[test]
	fn verifier_shape_requires_command_string_param() {
		use serde_json::json;
		// Command-string param → candidate (tool_map is empty in unit tests, so
		// the control-plane exclusion is exercised in integration, not here).
		assert!(is_verifier_shaped(
			"shell",
			&json!({"command": "cargo test"})
		));
		assert!(!is_verifier_shaped("view", &json!({"path": "a.rs"})));
		assert!(!is_verifier_shaped("shell", &json!({"command": 42})));
		assert!(!is_verifier_shaped("shell", &json!({})));
		assert!(!is_verifier_shaped("shell", &json!({"command": ""})));
	}

	#[test]
	fn verifier_shape_is_domain_agnostic() {
		use serde_json::json;
		// Any non-mutation command execution is a verifier candidate: the
		// framework does not hard-code program or script names. Whether a
		// candidate actually verifies is decided observationally (tree unchanged).
		assert!(is_verifier_shaped(
			"shell",
			&json!({"command": "bash scripts/lint-capabilities.sh \"$PWD/capabilities/\""})
		));
		assert!(is_verifier_shaped(
			"shell",
			&json!({"command": "cd /proj && sh scripts/test.sh"})
		));
		assert!(!is_verifier_shaped(
			"shell",
			&json!({"command": "bash scripts/deploy.sh"})
		));
		assert!(is_verifier_shaped(
			"shell",
			&json!({"command": "python check_booking.py --ref ABC123"})
		));
		assert!(!is_verifier_shaped(
			"text_editor",
			&json!({"command": "str_replace"})
		));
	}

	#[test]
	fn mutation_classification_uses_call_intent_and_mcp_hint() {
		use serde_json::json;
		assert!(is_mutation_call(
			"text_editor",
			&json!({"command":"str_replace"})
		));
		assert!(is_mutation_call(
			"generic_runner",
			&json!({"command":"deploy release"})
		));
		assert!(!is_mutation_call(
			"generic_runner",
			&json!({"command":"check booking status"})
		));
		register_tool_read_only_hint("remotePublisherForTest", Some(false));
		register_tool_read_only_hint("remoteLookupForTest", Some(true));
		assert!(is_mutation_call("remotePublisherForTest", &json!({})));
		assert!(!is_mutation_call("remoteLookupForTest", &json!({})));
	}

	#[test]
	fn reset_streak_clears_previous_task_verification_latch() {
		let mut d = Detectors::default();
		d.note_round_verification(None, None, false, false, true, false, true);
		assert!(d.needs_verification(None));
		// A new genuine task must not inherit an earlier task's mutation.
		d.reset_streak();
		assert!(!d.needs_verification(None));
	}

	#[test]
	fn steer_defers_to_gate_on_done() {
		assert!(!should_steer(
			DetectorSignal::NoProgress,
			Some(SelfReport::Done)
		));
		assert!(!should_steer(DetectorSignal::Loop, Some(SelfReport::Done)));
	}

	#[test]
	fn steer_waits_while_exploring_but_fires_on_loop() {
		assert!(!should_steer(
			DetectorSignal::NoProgress,
			Some(SelfReport::Exploring)
		));
		assert!(should_steer(
			DetectorSignal::Loop,
			Some(SelfReport::Exploring)
		));
		assert!(should_steer(
			DetectorSignal::NoProgress,
			Some(SelfReport::Progressing)
		));
	}

	#[test]
	fn conflict_framing_when_progressing_but_no_progress() {
		// No-progress signal while the agent insists it is progressing → conflict text.
		let conflict = steer_note(DetectorSignal::NoProgress, Some(SelfReport::Progressing), 0);
		assert!(conflict.contains("disagree"));
		// Without the progressing claim it stays the generic no-progress note.
		let generic = steer_note(DetectorSignal::NoProgress, None, 0);
		assert!(!generic.contains("disagree"));
	}

	#[test]
	fn failed_verifier_recovery_survives_unrelated_successes() {
		let mut d = Detectors::default();
		let failing_check = 11;
		let unrelated_check = 22;
		assert_eq!(
			d.record_round_verifier_outcomes(&[(failing_check, false)], 3),
			DetectorSignal::None
		);
		// A different successful command does not prove the failed behavior.
		assert_eq!(
			d.record_round_verifier_outcomes(&[(unrelated_check, true)], 3),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_round_verifier_outcomes(&[(failing_check, false)], 3),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_round_verifier_outcomes(&[(failing_check, false)], 3),
			DetectorSignal::Recovery
		);
	}

	#[test]
	fn same_verifier_success_discharges_recovery() {
		let mut d = Detectors::default();
		let check = 11;
		assert_eq!(
			d.record_round_verifier_outcomes(&[(check, false)], 2),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_round_verifier_outcomes(&[(check, true)], 2),
			DetectorSignal::None
		);
		// The old failed episode is gone, so one new failure is below threshold.
		assert_eq!(
			d.record_round_verifier_outcomes(&[(check, false)], 2),
			DetectorSignal::None
		);
	}

	#[test]
	fn conflicting_parallel_verifier_outcomes_do_not_clear_failure() {
		let mut d = Detectors::default();
		let check = 11;
		assert_eq!(
			d.record_round_verifier_outcomes(&[(check, false)], 2),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_round_verifier_outcomes(&[(check, true), (check, false)], 2),
			DetectorSignal::Recovery
		);
	}

	#[test]
	fn user_turn_reset_clears_failed_verifier_recovery() {
		let mut d = Detectors::default();
		assert_eq!(
			d.record_round_verifier_outcomes(&[(11, false)], 2),
			DetectorSignal::None
		);
		d.reset_streak();
		assert_eq!(
			d.record_round_verifier_outcomes(&[(11, false)], 2),
			DetectorSignal::None
		);
	}

	#[test]
	fn call_set_hash_ignores_order_and_id_but_tracks_params() {
		use crate::mcp::McpToolCall;
		let mk = |name: &str, p: serde_json::Value| McpToolCall {
			tool_name: name.into(),
			parameters: p,
			tool_id: "per-call-unique".into(),
		};
		let read = mk("read", serde_json::json!({"path": "x"}));
		let grep = mk("grep", serde_json::json!({"q": "y"}));
		// Same calls, any order, any tool_id → equal hash (re-issuing them = ignoring).
		assert_eq!(
			call_set_hash(&[read.clone(), grep.clone()]),
			call_set_hash(&[
				mk("grep", serde_json::json!({"q": "y"})),
				mk("read", serde_json::json!({"path": "x"})),
			])
		);
		// A changed parameter → different hash (the model trying a different call).
		assert_ne!(
			call_set_hash(&[read]),
			call_set_hash(&[mk("read", serde_json::json!({"path": "z"}))])
		);
	}

	#[test]
	fn persistent_frame_clamps_stuck_signals_past_the_ladder() {
		// A stuck signal re-firing past the 0→1→2 ladder holds the firmest frame: every
		// persistent variant carries the same firm ask (a different path, or `blocked`).
		assert!(steer_note(DetectorSignal::Loop, None, 5).contains("blocked"));
		// …but the phrasing ROTATES each re-emit so the repeated nudge does not habituate
		// (polymorphic warnings resist habituation — Anderson 2015 / Ancker 2017).
		assert_ne!(
			steer_note(DetectorSignal::Loop, None, PERSISTENT_ATTEMPT),
			steer_note(DetectorSignal::Loop, None, PERSISTENT_ATTEMPT + 1)
		);
	}
}
