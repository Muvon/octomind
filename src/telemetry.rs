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

//! Anonymous usage telemetry — which commands, tools, slash commands and models
//! people actually use, so the CLI gets shaped by evidence instead of guesses.
//!
//! Three rules, enforced by types rather than by filtering:
//!
//! 1. **Everything sent is a field on [`Event`] below.** There is no free-form
//!    map, so a path, a prompt or an argument value has nowhere to go even by
//!    accident.
//! 2. **Identifiers, never content.** Flag names never their values, tool names
//!    never their arguments, model ids never the conversation. Tool names that
//!    are not built in collapse to their `&'static str` category
//!    (`ext:github`), because MCP tool names come from user config.
//! 3. **Never in the way.** Events buffer in memory and are flushed once at
//!    exit behind a 2s timeout; every failure is silent. Telemetry that can
//!    delay or break the CLI is worse than no telemetry.
//!
//! Off via `DO_NOT_TRACK=1` ([consoledonottrack.com]), `OCTOMIND_TELEMETRY=0`,
//! or `telemetry = false` in the config. Opting out is purely local — no
//! request is made to announce it, and nothing is buffered.
//!
//! [consoledonottrack.com]: https://consoledonottrack.com

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::account;

pub const TELEMETRY_ENV: &str = "OCTOMIND_TELEMETRY";
/// Cross-vendor opt-out standard (consoledonottrack.com). Honoured before our
/// own switch: a user who set it once should not have to learn ours.
pub const DNT_ENV: &str = "DO_NOT_TRACK";

/// Payload shape. Bump when a field changes meaning, so the server can tell old
/// clients apart instead of guessing from the CLI version.
const SCHEMA_VERSION: u8 = 1;
/// Hard ceiling on how long process exit may wait for the flush.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
/// Mirrors the server's per-batch cap. A process that somehow produces more
/// drops the excess rather than getting the whole batch rejected.
const MAX_EVENTS: usize = 50;
/// Per-map cap (tools, slash commands). A runaway agent loop must not turn one
/// session into a megabyte of counters.
const MAX_KEYS: usize = 64;

/// Tools that ship with Octomind. Their names are ours, so they are safe to
/// send verbatim; everything else is an MCP tool named by user config and is
/// reduced to a static category by [`bucket_tool`].
const BUILTIN_TOOLS: &[&str] = &[
	"agent",
	"ast_grep",
	"batch_edit",
	"capability",
	"core",
	"extract_lines",
	"image",
	"list_files",
	"mcp",
	"plan",
	"read",
	"schedule",
	"shell",
	"skill",
	"tap",
	"text_editor",
	"view",
	"workdir",
	"write",
];

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Set by [`init`], read by the `start` event: the first run of a new install
/// is the only place activation and adoption can be measured from.
static FIRST_RUN: AtomicBool = AtomicBool::new(false);
/// Counted with an atomic rather than the state mutex: the only caller is the
/// Ctrl+C path, which must not take a lock that anything else can hold.
static CANCELS: AtomicU32 = AtomicU32::new(0);
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

#[derive(Default)]
struct State {
	events: Vec<Event>,
	/// Accumulated across the process and folded into the next session event —
	/// one row per session beats one row per tool call by three orders of
	/// magnitude, and the question is always "which tools, how often".
	tools: BTreeMap<String, u32>,
	tool_errors: BTreeMap<String, u32>,
	commands: BTreeMap<String, u32>,
	api_errors: BTreeMap<String, u32>,
}

/// One telemetry row. Flat rather than an enum per event kind: it maps 1:1 onto
/// the server's `cli_events` table, so no shape translation happens anywhere.
#[derive(Serialize, Default, Debug)]
pub struct Event {
	/// `start` | `session` | `error`
	name: &'static str,
	ts: u64,

	#[serde(skip_serializing_if = "String::is_empty")]
	command: String,
	/// Long flag names only (`--format`), never their values.
	#[serde(skip_serializing_if = "Vec::is_empty")]
	flags: Vec<String>,
	/// Session shape: `interactive` | `piped` | `daemon` | `acp` | `server` | `workflow`.
	#[serde(skip_serializing_if = "String::is_empty")]
	kind: String,
	/// Role or tap agent tag, e.g. `developer:general` — a software identifier.
	#[serde(skip_serializing_if = "String::is_empty")]
	agent: String,
	#[serde(skip_serializing_if = "String::is_empty")]
	provider: String,
	#[serde(skip_serializing_if = "String::is_empty")]
	model: String,
	/// `ok` | `error` | `cancel`
	#[serde(skip_serializing_if = "str::is_empty")]
	outcome: &'static str,
	/// Static classification only — never the error message.
	#[serde(skip_serializing_if = "str::is_empty")]
	error_kind: &'static str,

	#[serde(skip_serializing_if = "is_zero_u64")]
	duration_ms: u64,
	#[serde(skip_serializing_if = "is_zero_u32")]
	turns: u32,
	#[serde(skip_serializing_if = "is_zero_u32")]
	tool_calls: u32,
	#[serde(skip_serializing_if = "is_zero_u64")]
	tokens_in: u64,
	#[serde(skip_serializing_if = "is_zero_u64")]
	tokens_out: u64,
	#[serde(skip_serializing_if = "is_zero_u64")]
	tokens_cached: u64,
	#[serde(skip_serializing_if = "is_zero_u64")]
	tokens_reasoning: u64,
	/// Integer micro-USD, matching the control plane's money convention.
	#[serde(skip_serializing_if = "is_zero_i64")]
	cost_micro: i64,
	#[serde(skip_serializing_if = "is_zero_u32")]
	compressions: u32,
	#[serde(skip_serializing_if = "is_zero_u32")]
	mcp_servers: u32,

	#[serde(skip_serializing_if = "is_zero_u32")]
	cancels: u32,

	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	tools: BTreeMap<String, u32>,
	/// Failures per tool. Paired with `tools`, this is a per-tool success rate —
	/// the fastest way to see which integration is quietly broken.
	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	tool_errors: BTreeMap<String, u32>,
	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	commands: BTreeMap<String, u32>,
	/// Provider failures by fixed kind (`rate_limit`, `overloaded`, `auth`, …).
	/// Against `turns`, this is the answer to "is a provider degrading".
	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	api_errors: BTreeMap<String, u32>,

	#[serde(skip_serializing_if = "is_false")]
	resumed: bool,
	#[serde(skip_serializing_if = "is_false")]
	sandbox: bool,
	#[serde(skip_serializing_if = "is_false")]
	tty: bool,
	#[serde(skip_serializing_if = "is_false")]
	ci: bool,
	#[serde(skip_serializing_if = "is_false")]
	signed_in: bool,
	#[serde(skip_serializing_if = "is_false")]
	first_run: bool,
}

fn is_zero_u32(v: &u32) -> bool {
	*v == 0
}
fn is_zero_u64(v: &u64) -> bool {
	*v == 0
}
fn is_zero_i64(v: &i64) -> bool {
	*v == 0
}
fn is_false(v: &bool) -> bool {
	!*v
}

/// Process-constant context, sent once per batch instead of on every event.
#[derive(Serialize)]
struct Batch<'a> {
	v: u8,
	machine_id: String,
	version: &'static str,
	os: &'static str,
	arch: &'static str,
	/// How this binary got here: `brew` | `cargo` | `docker` | `source` | `binary`.
	install: &'static str,
	events: &'a [Event],
}

/// Everything a finished session reports. Built by the caller from state it
/// already has, so no session internals leak into this module.
pub struct SessionEnd<'a> {
	pub kind: &'a str,
	pub outcome: &'static str,
	pub error_kind: &'static str,
	pub resumed: bool,
	pub sandbox: bool,
	pub mcp_servers: u32,
	pub info: &'a crate::session::SessionInfo,
}

/// Arm telemetry for this process. Must run once, early, before any `record_*`
/// call — everything before it is silently dropped, which is the safe default.
///
/// Prints the one-time disclosure on first run. Deliberately synchronous and
/// local: nothing here touches the network.
pub fn init(config: &crate::config::Config) {
	if opted_out(config) {
		return;
	}
	// Whether this install already had an identity, checked BEFORE machine_id()
	// creates one — that transition is precisely what "first run" means.
	let known = crate::directories::get_config_dir()
		.map(|dir| dir.join("machine-id").exists())
		.unwrap_or(false);
	// A machine id we cannot persist means no stable identity — running without
	// one would inflate every unique-install count, so stay off instead.
	if account::machine_id().is_err() {
		return;
	}
	ENABLED.store(true, Ordering::Relaxed);
	LazyLock::force(&STARTED);
	FIRST_RUN.store(!known, Ordering::Relaxed);
	show_notice_once();
}

fn opted_out(config: &crate::config::Config) -> bool {
	if truthy(DNT_ENV) {
		return true;
	}
	match std::env::var(TELEMETRY_ENV) {
		Ok(v) => matches!(
			v.trim().to_ascii_lowercase().as_str(),
			"0" | "false" | "off" | "no"
		),
		Err(_) => !config.telemetry,
	}
}

fn truthy(var: &str) -> bool {
	std::env::var(var).is_ok_and(|v| {
		let v = v.trim();
		!v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
	})
}

/// The one-time disclosure, marked as shown only once it has actually been
/// printed. An editor launching us over ACP has no terminal to read it on, so
/// the notice waits for a run the user can actually see rather than being
/// silently consumed by a background process.
fn show_notice_once() {
	if !std::io::stderr().is_terminal() {
		return;
	}
	let Ok(dir) = crate::directories::get_config_dir() else {
		return;
	};
	let marker = dir.join("telemetry-notice");
	if marker.exists() {
		return;
	}
	eprintln!(
		"Octomind collects anonymous usage stats (commands, tool names, timings — never your code, \
		 prompts or paths). Turn it off with `{TELEMETRY_ENV}=0` or `telemetry = false` in the config."
	);
	let _ = std::fs::write(&marker, SCHEMA_VERSION.to_string());
}

fn enabled() -> bool {
	ENABLED.load(Ordering::Relaxed)
}

fn now() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| d.as_secs())
		.unwrap_or(0)
}

fn push(event: Event) {
	let mut state = STATE.lock();
	if state.events.len() >= MAX_EVENTS {
		return;
	}
	state.events.push(event);
}

/// A tool name safe to transmit: ours verbatim, anyone else's as the static
/// category [`crate::mcp::utils::guess_tool_category`] already derives. The
/// return type is what guarantees no user-defined string can escape.
fn bucket_tool(name: &str) -> String {
	if BUILTIN_TOOLS.contains(&name) {
		return name.to_string();
	}
	format!("ext:{}", crate::mcp::utils::guess_tool_category(name))
}

/// Every CLI invocation, recorded before the command runs so that crashes and
/// kills still leave a trace — `start` without a matching `session` is exactly
/// the "did it fall over" signal.
pub fn record_start(command: &str, flags: Vec<String>) {
	if !enabled() {
		return;
	}
	push(Event {
		name: "start",
		ts: now(),
		command: command.to_string(),
		flags,
		tty: std::io::stdin().is_terminal(),
		ci: truthy("CI") || std::env::var("GITHUB_ACTIONS").is_ok(),
		signed_in: account::session().is_some(),
		first_run: FIRST_RUN.load(Ordering::Relaxed),
		..Default::default()
	});
}

/// One tool execution. Counted in memory and folded into the session event.
pub fn record_tool(name: &str) {
	count_tool(name, |state| &mut state.tools);
}

/// One tool execution that came back an error.
pub fn record_tool_error(name: &str) {
	count_tool(name, |state| &mut state.tool_errors);
}

fn count_tool(name: &str, pick: fn(&mut State) -> &mut BTreeMap<String, u32>) {
	if !enabled() {
		return;
	}
	let key = bucket_tool(name);
	let mut state = STATE.lock();
	let map = pick(&mut state);
	if map.len() >= MAX_KEYS && !map.contains_key(&key) {
		return;
	}
	*map.entry(key).or_insert(0) += 1;
}

/// One failed call to a model provider, bucketed by [`api_error_kind`].
pub fn record_api_error(e: &anyhow::Error) {
	if !enabled() {
		return;
	}
	let kind = api_error_kind(e);
	let mut state = STATE.lock();
	*state.api_errors.entry(kind.to_string()).or_insert(0) += 1;
}

/// Classify a provider failure. Providers report the same condition in a dozen
/// wordings, so this matches on the text — but it returns `&'static str`, so no
/// part of that text can travel with the classification.
fn api_error_kind(e: &anyhow::Error) -> &'static str {
	let text = e.to_string().to_ascii_lowercase();
	if text.contains("rate limit") || text.contains("429") {
		"rate_limit"
	} else if text.contains("overloaded") || text.contains("529") {
		"overloaded"
	} else if text.contains("context length")
		|| text.contains("context_length")
		|| text.contains("too many tokens")
	{
		"context_length"
	} else if text.contains("401")
		|| text.contains("403")
		|| text.contains("unauthorized")
		|| text.contains("api key")
	{
		"auth"
	} else if text.contains("500")
		|| text.contains("502")
		|| text.contains("503")
		|| text.contains("520")
	{
		"server"
	} else {
		// Not a recognised provider condition — fall back to the transport-level
		// classification, which covers timeouts and connection failures.
		error_kind(e)
	}
}

/// The user interrupted a running operation (first Ctrl+C). A rising rate here
/// means the agent is going somewhere people do not want it to go.
pub fn record_cancel() {
	if enabled() {
		CANCELS.fetch_add(1, Ordering::Relaxed);
	}
}

/// One recognised slash command. Unknown input is chat, not a command, so the
/// caller only reports what actually dispatched.
pub fn record_command(command: &str) {
	if !enabled() {
		return;
	}
	let mut state = STATE.lock();
	if state.commands.len() >= MAX_KEYS && !state.commands.contains_key(command) {
		return;
	}
	*state.commands.entry(command.to_string()).or_insert(0) += 1;
}

/// A finished workflow run. Workflows fan out over several sessions and have no
/// single `SessionInfo`, so they report their own aggregate.
pub struct WorkflowEnd<'a> {
	/// The workflow's declared name — a label authored in the TOML, never the
	/// path it was loaded from.
	pub name: &'a str,
	pub steps: u32,
	pub duration_ms: u64,
	pub cost_usd: f64,
	pub tokens_in: u64,
	pub tokens_out: u64,
	pub tool_calls: u32,
	pub graph: bool,
}

/// A finished workflow, filed as a session of kind `workflow` so one query
/// answers "what ran" across every surface.
pub fn record_workflow(end: WorkflowEnd) {
	if !enabled() {
		return;
	}
	push(Event {
		name: "session",
		ts: now(),
		kind: if end.graph {
			"workflow_graph"
		} else {
			"workflow"
		}
		.to_string(),
		agent: end.name.to_string(),
		outcome: "ok",
		duration_ms: end.duration_ms,
		turns: end.steps,
		tool_calls: end.tool_calls,
		tokens_in: end.tokens_in,
		tokens_out: end.tokens_out,
		cost_micro: (end.cost_usd * 1_000_000.0).round() as i64,
		..Default::default()
	});
}

/// A finished session. Drains the accumulated tool/command counters into it, so
/// a process running several sessions attributes each one correctly.
pub fn record_session(end: SessionEnd) {
	if !enabled() {
		return;
	}
	let (tools, tool_errors, commands, api_errors) = {
		let mut state = STATE.lock();
		(
			std::mem::take(&mut state.tools),
			std::mem::take(&mut state.tool_errors),
			std::mem::take(&mut state.commands),
			std::mem::take(&mut state.api_errors),
		)
	};
	let info = end.info;
	let (provider, model) = split_model(&info.model);
	push(Event {
		name: "session",
		ts: now(),
		kind: end.kind.to_string(),
		agent: info.role.clone(),
		provider,
		model,
		outcome: end.outcome,
		error_kind: end.error_kind,
		duration_ms: STARTED.elapsed().as_millis() as u64,
		turns: info.total_api_calls as u32,
		tool_calls: info.tool_calls as u32,
		tokens_in: info.input_tokens,
		tokens_out: info.output_tokens,
		tokens_cached: info.cache_read_tokens,
		tokens_reasoning: info.reasoning_tokens,
		cost_micro: (info.total_cost * 1_000_000.0).round() as i64,
		compressions: info.compression_stats.total_compressions() as u32,
		mcp_servers: end.mcp_servers,
		cancels: CANCELS.swap(0, Ordering::Relaxed),
		tools,
		tool_errors,
		commands,
		api_errors,
		resumed: end.resumed,
		sandbox: end.sandbox,
		..Default::default()
	});
}

/// A command that failed. The classification is a fixed slug — the message
/// itself can contain paths, hostnames and user text, so it never travels.
pub fn record_error(command: &str, kind: &'static str) {
	if !enabled() {
		return;
	}
	push(Event {
		name: "error",
		ts: now(),
		command: command.to_string(),
		outcome: "error",
		error_kind: kind,
		duration_ms: STARTED.elapsed().as_millis() as u64,
		..Default::default()
	});
}

/// Classify a failure into one of a handful of static buckets. Deliberately
/// coarse: the point is "are network failures rising", not a stack trace.
pub fn error_kind(e: &anyhow::Error) -> &'static str {
	for cause in e.chain() {
		if let Some(req) = cause.downcast_ref::<reqwest::Error>() {
			return if req.is_timeout() {
				"timeout"
			} else {
				"network"
			};
		}
		if cause.downcast_ref::<std::io::Error>().is_some() {
			return "io";
		}
		if cause.downcast_ref::<serde_json::Error>().is_some() {
			return "parse";
		}
	}
	"other"
}

/// `provider:model` → both halves. Model ids are public catalogue names, safe
/// to send; anything without a provider prefix is reported as model only.
fn split_model(full: &str) -> (String, String) {
	match full.split_once(':') {
		Some((p, m)) => (p.to_string(), m.to_string()),
		None => (String::new(), full.to_string()),
	}
}

fn install_source() -> &'static str {
	if std::path::Path::new("/.dockerenv").exists() {
		return "docker";
	}
	let Ok(exe) = std::env::current_exe() else {
		return "unknown";
	};
	let path = exe.to_string_lossy();
	if path.contains("/Cellar/") || path.contains("/homebrew/") {
		"brew"
	} else if path.contains("/.cargo/") {
		"cargo"
	} else if path.contains("/target/debug/") || path.contains("/target/release/") {
		"source"
	} else {
		"binary"
	}
}

/// Ship whatever is buffered and stop. Called once, at process exit, wrapped in
/// a timeout — a slow or unreachable control plane must cost the user nothing.
pub async fn flush() {
	if !enabled() {
		return;
	}
	let events = std::mem::take(&mut STATE.lock().events);
	if events.is_empty() {
		return;
	}
	// Disarm first: a second flush (or a late record) after this point would
	// either double-send or block exit again.
	ENABLED.store(false, Ordering::Relaxed);

	let Ok(machine_id) = account::machine_id() else {
		return;
	};
	let batch = Batch {
		v: SCHEMA_VERSION,
		machine_id,
		version: env!("CARGO_PKG_VERSION"),
		os: std::env::consts::OS,
		arch: std::env::consts::ARCH,
		install: install_source(),
		events: &events,
	};

	let send = async {
		let client = reqwest::Client::builder().timeout(FLUSH_TIMEOUT).build()?;
		let mut req = client
			.post(format!("{}/api/v1/telemetry", account::api_url()))
			.json(&batch);
		// Signed in? Attach the session so events land on the account. A stale
		// jwt is not worth refreshing here — the server files those anonymously.
		if let Some(s) = account::session() {
			req = req.bearer_auth(s.jwt);
		}
		req.send().await?;
		Ok::<(), reqwest::Error>(())
	};

	match tokio::time::timeout(FLUSH_TIMEOUT, send).await {
		Ok(Ok(())) => crate::log_debug!("telemetry: sent {} events", events.len()),
		Ok(Err(e)) => crate::log_debug!("telemetry: send failed: {}", e),
		Err(_) => crate::log_debug!("telemetry: flush timed out"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn builtin_tools_pass_through_and_mcp_tools_collapse_to_a_category() {
		assert_eq!(bucket_tool("shell"), "shell");
		assert_eq!(bucket_tool("text_editor"), "text_editor");
		// The whole point: a user-named MCP tool must not survive as itself.
		assert_eq!(bucket_tool("acme_internal_billing"), "ext:external");
		assert_eq!(bucket_tool("github_create_pr"), "ext:github");
	}

	#[test]
	fn provider_failures_bucket_by_condition_not_by_wording() {
		let kind = |m: &str| api_error_kind(&anyhow::anyhow!("{m}"));
		// Providers word the same condition differently; the bucket must not care.
		assert_eq!(kind("API error 429 <unknown status code>"), "rate_limit");
		assert_eq!(kind("Rate limit exceeded for gpt-5"), "rate_limit");
		assert_eq!(kind("Overloaded: upstream capacity"), "overloaded");
		assert_eq!(
			kind("maximum context length is 200000 tokens"),
			"context_length"
		);
		assert_eq!(kind("API error 401 unauthorized"), "auth");
		assert_eq!(kind("API error 503 <unknown status code>"), "server");
		// Unrecognised text falls through to the transport classification, and in
		// no case does any of the message itself become the bucket.
		assert_eq!(kind("something we have never seen"), "other");
	}

	#[test]
	fn model_splits_into_provider_and_catalogue_id() {
		assert_eq!(
			split_model("openrouter:anthropic/claude-sonnet-4"),
			("openrouter".into(), "anthropic/claude-sonnet-4".into())
		);
		assert_eq!(split_model("gpt-5"), (String::new(), "gpt-5".into()));
	}

	#[test]
	fn events_serialize_without_empty_fields() {
		let json = serde_json::to_string(&Event {
			name: "start",
			ts: 1,
			command: "run".into(),
			..Default::default()
		})
		.unwrap();
		assert_eq!(json, r#"{"name":"start","ts":1,"command":"run"}"#);
	}
}
