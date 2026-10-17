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

//! Project-local guardrails — `.agents/guardrails.toml`.
//!
//! Four section types, evaluated at different phases:
//!
//!   [[pipe]]      — pre-model input transform.  Runs before the model sees user input.
//!   [[guard]]      — pre-call deny rule.       Blocks the tool from running.
//!   [[hook]]       — post-result script.        Runs after each tool result.
//!   [[validator]]  — end-of-turn script.        Runs after the assistant turn.
//!
//! Shared DSL (used in `match` for guards/hooks and inside `when` entries):
//!
//!   capability                       — any call to that capability
//!   capability(regex)                — regex matched against full args JSON
//!   capability(arg_name=regex)       — regex matched against a specific arg
//!
//! Example:
//!
//!   [[guard]]
//!   match   = "shell(command=^rm\\s+-rf?)"
//!   message = "rm -rf blocked."
//!
//!   [[guard]]
//!   match   = "shell(command=^ls\\b)"
//!   has     = "filesystem-read"               # string or list of capabilities
//!   when    = ["-filesystem-read"]            # + = used since session start
//!   message = "Use view instead of ls."        # - = NOT used since session start

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const FILE_PATH: &str = ".agents/guardrails.toml";

/// A freshly written snapshot copy can briefly fail to exec with ETXTBSY while
/// a child forked concurrently by another thread still holds its write fd (the
/// fd closes when that child execs). Retry instead of losing the rule run.
const SPAWN_BUSY_RETRIES: u32 = 5;
const SPAWN_BUSY_BACKOFF: Duration = Duration::from_millis(20);

/// Uniquifies temp names so concurrent runs never write the same temp file.
static COPY_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(untagged)]
enum HasField {
	#[default]
	None,
	One(String),
	Many(Vec<String>),
}

impl HasField {
	fn into_vec(self) -> Vec<String> {
		match self {
			HasField::None => Vec::new(),
			HasField::One(s) => vec![s],
			HasField::Many(v) => v,
		}
	}
}

#[derive(Debug, Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
pub enum PipeWhen {
	First,
	#[default]
	Any,
}

#[derive(Debug, Deserialize)]
struct RawPipe {
	name: String,
	command: String,
	#[serde(rename = "match", default)]
	match_: Option<String>,
	#[serde(default)]
	when: PipeWhen,
	#[serde(default)]
	roles: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawGuard {
	#[serde(rename = "match")]
	match_: String,
	#[serde(default)]
	has: HasField,
	#[serde(default)]
	when: Vec<String>,
	message: String,
}

#[derive(Debug, Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
pub enum HookOn {
	Success,
	Error,
	#[default]
	Any,
}

#[derive(Debug, Deserialize)]
struct RawHook {
	#[serde(rename = "match", default)]
	match_: Option<String>,
	#[serde(default)]
	result: Option<String>,
	#[serde(default)]
	on: HookOn,
	script: String,
}

#[derive(Debug, Deserialize)]
struct RawValidator {
	name: String,
	#[serde(rename = "match", default)]
	match_: Option<String>,
	#[serde(default)]
	when: Vec<String>,
	#[serde(default)]
	roles: Vec<String>,
	script: String,
}

#[derive(Debug, Deserialize)]
struct RawFile {
	#[serde(default, rename = "pipe")]
	pipes: Vec<RawPipe>,
	#[serde(default, rename = "guard")]
	guards: Vec<RawGuard>,
	#[serde(default, rename = "hook")]
	hooks: Vec<RawHook>,
	#[serde(default, rename = "validator")]
	validators: Vec<RawValidator>,
}

#[derive(Debug, Clone)]
pub struct EvolutionBinding {
	pub id: String,
	pub shadow: bool,
}

/// Script content captured when rules are installed for a session. Rules are a
/// session snapshot, so the scripts they run must be too: `.agents/` lives in
/// the sandbox-writable workdir, and running the file live would let an edit
/// made mid-session (by the agent itself) silently change or disable a rule.
/// Runs execute the captured bytes; edits apply on the next rules reload.
#[derive(Debug, Clone, Default)]
pub enum ScriptSnapshot {
	/// Not installed for a session yet.
	#[default]
	Untaken,
	/// Unreadable at install time. Never falls back to the live path, which
	/// could by now hold a script created mid-session.
	Missing(PathBuf),
	Taken { source: PathBuf, content: Arc<[u8]> },
}

impl ScriptSnapshot {
	fn take(source: PathBuf) -> Self {
		match std::fs::read(&source) {
			Ok(bytes) => Self::Taken {
				source,
				content: bytes.into(),
			},
			Err(_) => Self::Missing(source),
		}
	}

	/// Build the command that runs the snapshot: a private content-addressed
	/// copy outside the workdir, rewritten from memory whenever it no longer
	/// matches. `cwd` stays the workdir; `OCTOMIND_SCRIPT_DIR` is the original
	/// script's directory, since `$0` now points at the copy.
	pub fn command(&self, workdir: &Path) -> Result<tokio::process::Command> {
		let (source, content) = match self {
			Self::Untaken => return Err(anyhow!("script was not snapshotted for this session")),
			Self::Missing(source) => {
				return Err(anyhow!(
					"script `{}` was not found when the session started",
					source.display()
				))
			}
			Self::Taken { source, content } => (source, content),
		};
		let copy = materialize(source, content)?;
		let mut cmd = tokio::process::Command::new(copy);
		cmd.current_dir(workdir);
		cmd.env("OCTOMIND_WORKDIR", workdir.display().to_string());
		if let Some(dir) = source.parent() {
			cmd.env("OCTOMIND_SCRIPT_DIR", dir);
		}
		Ok(cmd)
	}
}

/// Ensure `<data>/guardrails/<sha256>.<ext>` holds `content` and return it.
/// The extension is kept because Windows picks the runner (`.cmd`) from it.
fn materialize(source: &Path, content: &[u8]) -> Result<PathBuf> {
	let dir = crate::directories::get_guardrail_scripts_dir()?;
	let hash = hex::encode(Sha256::digest(content));
	let mut name = OsString::from(&hash);
	if let Some(ext) = source.extension() {
		name.push(".");
		name.push(ext);
	}
	let copy = dir.join(name);
	if std::fs::read(&copy).is_ok_and(|on_disk| on_disk == content) {
		return Ok(copy);
	}
	// Write aside and rename into place so a concurrent run never execs a
	// half-written copy.
	let tmp = dir.join(format!(
		"{}.{}.{}.tmp",
		hash,
		std::process::id(),
		COPY_SEQ.fetch_add(1, Ordering::Relaxed)
	));
	std::fs::write(&tmp, content).with_context(|| format!("write script copy {}", tmp.display()))?;
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700))
			.with_context(|| format!("chmod script copy {}", tmp.display()))?;
	}
	std::fs::rename(&tmp, &copy).with_context(|| format!("install script copy {}", copy.display()))?;
	Ok(copy)
}

/// Spawn a rule script, retrying the transient ETXTBSY of a fresh copy.
pub async fn spawn_script(cmd: &mut tokio::process::Command) -> std::io::Result<tokio::process::Child> {
	let mut attempt = 0;
	loop {
		match cmd.spawn() {
			Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < SPAWN_BUSY_RETRIES => {
				attempt += 1;
				tokio::time::sleep(SPAWN_BUSY_BACKOFF * attempt).await;
			}
			result => return result,
		}
	}
}

/// Resolve a rule's script path against the session workdir.
fn resolve_script(workdir: &Path, script: &Path) -> PathBuf {
	if script.is_absolute() {
		script.to_path_buf()
	} else {
		workdir.join(script)
	}
}

#[derive(Debug, Clone)]
pub struct CompiledPipe {
	pub name: String,
	pub command: PathBuf,
	pub snapshot: ScriptSnapshot,
	/// Regex on user message text; `None` matches all messages.
	pub match_regex: Option<Regex>,
	/// `when = "first"` restricts to first message; `when = "any"` (default) applies to all.
	pub when: PipeWhen,
	/// Role filter; entries match exact role (`developer:general`) or domain
	/// prefix (`developer` matches `developer:general` via `<name>:` check).
	pub roles: Vec<String>,
	pub evolution: Option<EvolutionBinding>,
}

#[derive(Debug, Clone)]
pub struct Target {
	pub capability: String,
	pub arg_name: Option<String>,
	pub regex: Option<Regex>,
}

#[derive(Debug, Clone)]
pub struct CompiledGuard {
	pub trigger: Target,
	pub has: Vec<String>,
	pub when_used: Vec<Target>,
	pub when_unused: Vec<Target>,
	pub message: String,
	pub evolution: Option<EvolutionBinding>,
}

#[derive(Debug, Clone)]
pub struct CompiledHook {
	/// Call-side filter; `None` matches any tool call.
	pub trigger: Option<Target>,
	/// Result-text filter; `None` matches any result content (incl. empty).
	pub result_regex: Option<Regex>,
	pub on: HookOn,
	pub script: PathBuf,
	pub snapshot: ScriptSnapshot,
	pub evolution: Option<EvolutionBinding>,
}

#[derive(Debug, Clone)]
pub struct CompiledValidator {
	pub name: String,
	/// Regex on assistant final message; `None` matches any message.
	pub match_regex: Option<Regex>,
	pub when_used: Vec<Target>,
	pub when_unused: Vec<Target>,
	/// Role filter; entries match exact role (`developer:general`) or domain
	/// prefix (`developer` matches `developer:general` via `<name>:` check).
	pub roles: Vec<String>,
	pub script: PathBuf,
	pub snapshot: ScriptSnapshot,
	pub evolution: Option<EvolutionBinding>,
}

#[derive(Debug, Clone, Default)]
pub struct Guardrails {
	pub pipes: Vec<CompiledPipe>,
	pub guards: Vec<CompiledGuard>,
	pub hooks: Vec<CompiledHook>,
	pub validators: Vec<CompiledValidator>,
}

impl Guardrails {
	pub fn append_compiled(&mut self, generated: Self) {
		self.pipes.extend(generated.pipes);
		self.guards.extend(generated.guards);
		self.hooks.extend(generated.hooks);
		self.validators.extend(generated.validators);
	}

	/// Snapshot every pipe, hook and validator script not snapshotted yet.
	/// Entries that already carry one keep it, so rules copied into a delegated
	/// agent's scope cannot pick up a script edited after session start.
	pub fn snapshot_scripts(&mut self, workdir: &Path) {
		for p in &mut self.pipes {
			if matches!(p.snapshot, ScriptSnapshot::Untaken) {
				p.snapshot = ScriptSnapshot::take(resolve_script(workdir, &p.command));
			}
		}
		for h in &mut self.hooks {
			if matches!(h.snapshot, ScriptSnapshot::Untaken) {
				h.snapshot = ScriptSnapshot::take(resolve_script(workdir, &h.script));
			}
		}
		for v in &mut self.validators {
			if matches!(v.snapshot, ScriptSnapshot::Untaken) {
				v.snapshot = ScriptSnapshot::take(resolve_script(workdir, &v.script));
			}
		}
	}

	/// Append a generated native artifact after user-authored rules. The existing
	/// runtime remains the sole executor; the binding only carries lifecycle
	/// attribution and shadow behavior.
	pub fn append_generated(&mut self, mut generated: Self, id: &str, shadow: bool) {
		let binding = || EvolutionBinding {
			id: id.to_string(),
			shadow,
		};
		for item in &mut generated.pipes {
			item.evolution = Some(binding());
		}
		for item in &mut generated.guards {
			item.evolution = Some(binding());
		}
		for item in &mut generated.hooks {
			item.evolution = Some(binding());
		}
		for item in &mut generated.validators {
			item.evolution = Some(binding());
		}
		self.append_compiled(generated);
	}

	/// Load `.agents/guardrails.toml` from the given workdir.
	/// Missing file = empty guardrails (silent). Parse errors are logged
	/// and treated as empty so a broken file never crashes the session.
	pub fn load_from_workdir(workdir: &Path) -> Self {
		let path = workdir.join(FILE_PATH);
		let Ok(text) = std::fs::read_to_string(&path) else {
			return Self::default();
		};
		match Self::parse(&text) {
			Ok(g) => {
				crate::log_debug!(
					"Loaded guardrails: {} pipe(s), {} guard(s), {} hook(s), {} validator(s) from {}",
					g.pipes.len(),
					g.guards.len(),
					g.hooks.len(),
					g.validators.len(),
					path.display()
				);
				g
			}
			Err(e) => {
				eprintln!("guardrails: failed to parse {}: {}", path.display(), e);
				Self::default()
			}
		}
	}

	pub fn parse(toml_str: &str) -> Result<Self> {
		let raw: RawFile = toml::from_str(toml_str)?;

		let mut pipes = Vec::with_capacity(raw.pipes.len());
		// Names key per-session run-count state (PIPE_RUN_COUNTS); duplicates would
		// silently share/clobber it. Reject them loudly, like the schema's other checks.
		let mut pipe_names = std::collections::HashSet::new();
		for p in raw.pipes {
			if p.name.trim().is_empty() {
				return Err(anyhow!("pipe missing `name`"));
			}
			if !pipe_names.insert(p.name.clone()) {
				return Err(anyhow!("duplicate pipe `name`: `{}`", p.name));
			}
			if p.command.trim().is_empty() {
				return Err(anyhow!("pipe `{}` missing `command`", p.name));
			}
			let match_regex = match p.match_.as_deref() {
				Some(s) if !s.is_empty() => Some(Regex::new(s).map_err(|e| {
					anyhow!("pipe `{}`: invalid match regex `{}`: {}", p.name, s, e)
				})?),
				_ => None,
			};
			pipes.push(CompiledPipe {
				name: p.name,
				command: PathBuf::from(p.command),
				snapshot: ScriptSnapshot::default(),
				match_regex,
				when: p.when,
				roles: p.roles,
				evolution: None,
			});
		}

		let mut guards = Vec::with_capacity(raw.guards.len());
		for r in raw.guards {
			let trigger = parse_target(&r.match_)
				.map_err(|e| anyhow!("guard `{}`: invalid match: {}", r.match_, e))?;
			let mut when_used = Vec::new();
			let mut when_unused = Vec::new();
			for item in r.when {
				let trimmed = item.trim();
				let mut chars = trimmed.chars();
				let sign = chars.next();
				let rest: &str = chars.as_str();
				match sign {
					Some('+') => when_used
						.push(parse_target(rest).map_err(|e| anyhow!("when `{}`: {}", item, e))?),
					Some('-') => when_unused
						.push(parse_target(rest).map_err(|e| anyhow!("when `{}`: {}", item, e))?),
					_ => {
						return Err(anyhow!("when entry must start with `+` or `-`: {}", item));
					}
				}
			}
			guards.push(CompiledGuard {
				trigger,
				has: r.has.into_vec(),
				when_used,
				when_unused,
				message: r.message,
				evolution: None,
			});
		}
		let mut hooks = Vec::with_capacity(raw.hooks.len());
		for h in raw.hooks {
			let trigger = match h.match_.as_deref() {
				Some(s) if !s.trim().is_empty() => Some(
					parse_target(s).map_err(|e| anyhow!("hook `{}`: invalid match: {}", s, e))?,
				),
				_ => None,
			};
			let result_regex = match h.result.as_deref() {
				Some(s) => Some(
					Regex::new(s)
						.map_err(|e| anyhow!("hook: invalid result regex `{}`: {}", s, e))?,
				),
				None => None,
			};
			if h.script.trim().is_empty() {
				return Err(anyhow!("hook missing `script`"));
			}
			hooks.push(CompiledHook {
				trigger,
				result_regex,
				on: h.on,
				script: PathBuf::from(h.script),
				snapshot: ScriptSnapshot::default(),
				evolution: None,
			});
		}
		let mut validators = Vec::with_capacity(raw.validators.len());
		// Names key per-session cursor state (VALIDATOR_CURSORS); duplicates would
		// share one cursor so a validator silently never fires. Reject them loudly.
		let mut validator_names = std::collections::HashSet::new();
		for v in raw.validators {
			if v.name.trim().is_empty() {
				return Err(anyhow!("validator missing `name`"));
			}
			if !validator_names.insert(v.name.clone()) {
				return Err(anyhow!("duplicate validator `name`: `{}`", v.name));
			}
			if v.script.trim().is_empty() {
				return Err(anyhow!("validator `{}` missing `script`", v.name));
			}
			let match_regex = match v.match_.as_deref() {
				Some(s) if !s.is_empty() => Some(Regex::new(s).map_err(|e| {
					anyhow!("validator `{}`: invalid match regex `{}`: {}", v.name, s, e)
				})?),
				_ => None,
			};
			let mut when_used = Vec::new();
			let mut when_unused = Vec::new();
			for item in v.when {
				let trimmed = item.trim();
				let mut chars = trimmed.chars();
				let sign = chars.next();
				let rest: &str = chars.as_str();
				match sign {
					Some('+') => {
						when_used.push(parse_target(rest).map_err(|e| {
							anyhow!("validator `{}` when `{}`: {}", v.name, item, e)
						})?)
					}
					Some('-') => {
						when_unused.push(parse_target(rest).map_err(|e| {
							anyhow!("validator `{}` when `{}`: {}", v.name, item, e)
						})?)
					}
					_ => {
						return Err(anyhow!(
							"validator `{}`: when entry must start with `+` or `-`: {}",
							v.name,
							item
						));
					}
				}
			}
			validators.push(CompiledValidator {
				name: v.name,
				match_regex,
				when_used,
				when_unused,
				roles: v.roles,
				script: PathBuf::from(v.script),
				snapshot: ScriptSnapshot::default(),
				evolution: None,
			});
		}
		Ok(Self {
			pipes,
			guards,
			hooks,
			validators,
		})
	}
}

/// Cheap role filter: empty filter = always pass; otherwise pass when the
/// current role is either an exact match or a domain prefix (e.g. filter
/// `developer` passes for current `developer:general`).
pub fn role_matches(filter: &[String], current: &str) -> bool {
	if filter.is_empty() {
		return true;
	}
	for f in filter {
		if f == current {
			return true;
		}
		// Domain prefix: filter "developer" passes "developer:..." but not
		// "developer-foo" — require the `:` separator.
		if current.len() > f.len()
			&& current.starts_with(f.as_str())
			&& current.as_bytes()[f.len()] == b':'
		{
			return true;
		}
	}
	false
}

/// Parse one target. Forms:
///   "cap"
///   "cap(regex)"
///   "cap(arg=regex)"   — arg-targeted iff the inner string starts with `\w+=`
fn parse_target(s: &str) -> Result<Target> {
	let s = s.trim();
	if s.is_empty() {
		return Err(anyhow!("empty target"));
	}
	let Some(open) = s.find('(') else {
		return Ok(Target {
			capability: s.to_string(),
			arg_name: None,
			regex: None,
		});
	};
	if !s.ends_with(')') {
		return Err(anyhow!("missing closing `)` in `{}`", s));
	}
	let capability = s[..open].trim().to_string();
	if capability.is_empty() {
		return Err(anyhow!("empty capability in `{}`", s));
	}
	let inner = &s[open + 1..s.len() - 1];
	let (arg_name, regex_src) = split_arg(inner);
	let regex =
		Regex::new(regex_src).map_err(|e| anyhow!("invalid regex `{}`: {}", regex_src, e))?;
	Ok(Target {
		capability,
		arg_name,
		regex: Some(regex),
	})
}

/// If `inner` starts with `<word>=`, split into (Some(word), rest).
/// Otherwise return (None, inner).
fn split_arg(inner: &str) -> (Option<String>, &str) {
	let Some(eq) = inner.find('=') else {
		return (None, inner);
	};
	let head = &inner[..eq];
	if !head.is_empty() && head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
		(Some(head.to_string()), &inner[eq + 1..])
	} else {
		(None, inner)
	}
}

/// One recorded call: `(capability, params)`. `capability` is the resolved
/// capability name (the logical grouping a tool belongs to, e.g. `shell`,
/// `filesystem-read`); may be `None` for tools not registered by any
/// capability.
pub type CallRecord = (Option<String>, Value);

/// Match a target against a recorded or current call.
///
/// `target.capability` must equal the resolved capability name for the call.
pub fn target_matches(target: &Target, capability: Option<&str>, params: &Value) -> bool {
	let Some(cap) = capability else {
		return false;
	};
	if target.capability != cap {
		return false;
	}
	let Some(re) = &target.regex else {
		return true;
	};
	// Match against the raw JSON form of either one specific arg or the
	// whole params object. Strings are matched without their surrounding
	// quotes (so `arg=^foo` works on `{"arg":"foo"}`); arrays/objects/
	// numbers/bools are matched against their JSON serialization
	// (so `paths=file` matches `["a/file.rs","b.rs"]`).
	let haystack: String = match &target.arg_name {
		Some(name) => match params.get(name) {
			Some(serde_json::Value::String(s)) => s.clone(),
			Some(v) => v.to_string(),
			None => String::new(),
		},
		None => serde_json::to_string(params).unwrap_or_default(),
	};
	re.is_match(&haystack)
}

/// Evaluate rules against the current call. Returns `Some(message)` to deny.
pub fn check(
	rules: &Guardrails,
	capability: Option<&str>,
	params: &Value,
	call_log: &[CallRecord],
	loaded: &HashSet<String>,
) -> Option<String> {
	evaluate_guards(rules, capability, params, call_log, loaded)
		.blocked
		.map(|(message, _)| message)
}

pub struct GuardEvaluation {
	pub blocked: Option<(String, Option<EvolutionBinding>)>,
	pub shadow_ids: Vec<String>,
}

pub fn evaluate_guards(
	rules: &Guardrails,
	capability: Option<&str>,
	params: &Value,
	call_log: &[CallRecord],
	loaded: &HashSet<String>,
) -> GuardEvaluation {
	let mut shadow_ids = Vec::new();
	for rule in &rules.guards {
		if !target_matches(&rule.trigger, capability, params) {
			continue;
		}
		if !rule.has.iter().all(|c| loaded.contains(c.as_str())) {
			continue;
		}
		let used_ok = rule.when_used.iter().all(|t| {
			call_log
				.iter()
				.any(|(c, p)| target_matches(t, c.as_deref(), p))
		});
		if !used_ok {
			continue;
		}
		let unused_ok = rule.when_unused.iter().all(|t| {
			!call_log
				.iter()
				.any(|(c, p)| target_matches(t, c.as_deref(), p))
		});
		if !unused_ok {
			continue;
		}
		if let Some(binding) = &rule.evolution {
			if crate::supervisor::learning::evolution::binding_is_shadow(
				&binding.id,
				binding.shadow,
			) {
				shadow_ids.push(binding.id.clone());
				continue;
			}
		}
		return GuardEvaluation {
			blocked: Some((rule.message.clone(), rule.evolution.clone())),
			shadow_ids,
		};
	}
	GuardEvaluation {
		blocked: None,
		shadow_ids,
	}
}

#[cfg(test)]
#[path = "guardrails_tests.rs"]
mod tests;
