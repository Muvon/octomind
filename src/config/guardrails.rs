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

//! Project-local guardrails — per-call deny rules loaded from
//! `<workdir>/.agents/guardrails.toml`.
//!
//! Rule DSL (used in `match` and inside `when` entries):
//!
//!   capability                       — any call to that capability
//!   capability(regex)                — regex matched against full args JSON
//!   capability(arg_name=regex)       — regex matched against a specific arg
//!
//! Rule file:
//!
//!   [[rule]]
//!   match   = "shell(command=^rm\\s+-rf?)"
//!   message = "rm -rf blocked."
//!
//!   [[rule]]
//!   match   = "shell(command=^ls\\b)"
//!   has     = "filesystem"                    # string or list
//!   when    = ["-filesystem(view)"]           # + = used, - = NOT used
//!   message = "Use view instead of ls."

use anyhow::{anyhow, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const FILE_PATH: &str = ".agents/guardrails.toml";

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

#[derive(Debug, Deserialize)]
struct RawRule {
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
struct RawFile {
	#[serde(default, rename = "rule")]
	rules: Vec<RawRule>,
	#[serde(default, rename = "hook")]
	hooks: Vec<RawHook>,
}

#[derive(Debug, Clone)]
pub struct Target {
	pub capability: String,
	pub arg_name: Option<String>,
	pub regex: Option<Regex>,
}

#[derive(Debug, Clone)]
pub struct CompiledRule {
	pub trigger: Target,
	pub has: Vec<String>,
	pub when_used: Vec<Target>,
	pub when_unused: Vec<Target>,
	pub message: String,
}

#[derive(Debug, Clone)]
pub struct CompiledHook {
	/// Call-side filter; `None` matches any tool call.
	pub trigger: Option<Target>,
	/// Result-text filter; `None` matches any result content (incl. empty).
	pub result_regex: Option<Regex>,
	pub on: HookOn,
	pub script: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct Guardrails {
	pub rules: Vec<CompiledRule>,
	pub hooks: Vec<CompiledHook>,
}

impl Guardrails {
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
					"Loaded {} guardrail rule(s) from {}",
					g.rules.len(),
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
		let mut rules = Vec::with_capacity(raw.rules.len());
		for r in raw.rules {
			let trigger = parse_target(&r.match_)
				.map_err(|e| anyhow!("rule `{}`: invalid match: {}", &r.match_, e))?;
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
			rules.push(CompiledRule {
				trigger,
				has: r.has.into_vec(),
				when_used,
				when_unused,
				message: r.message,
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
			});
		}
		Ok(Self { rules, hooks })
	}
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
	for rule in &rules.rules {
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
		return Some(rule.message.clone());
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn loaded(items: &[&str]) -> HashSet<String> {
		items.iter().map(|s| s.to_string()).collect()
	}

	#[test]
	fn parse_bare_capability() {
		let t = parse_target("shell").unwrap();
		assert_eq!(t.capability, "shell");
		assert!(t.arg_name.is_none());
		assert!(t.regex.is_none());
	}

	#[test]
	fn parse_whole_args_regex() {
		let t = parse_target("shell(rm -rf)").unwrap();
		assert_eq!(t.capability, "shell");
		assert!(t.arg_name.is_none());
		assert!(t.regex.unwrap().is_match("rm -rf"));
	}

	#[test]
	fn parse_arg_targeted() {
		let t = parse_target("shell(command=^ls\\b)").unwrap();
		assert_eq!(t.capability, "shell");
		assert_eq!(t.arg_name.as_deref(), Some("command"));
		assert!(t.regex.unwrap().is_match("ls -lt"));
	}

	#[test]
	fn unconditional_block() {
		let g = Guardrails::parse(
			r#"
			[[rule]]
			match = "shell(command=^rm\\s+-rf?)"
			message = "no"
			"#,
		)
		.unwrap();
		let p = json!({ "command": "rm -rf /tmp/x" });
		assert_eq!(
			check(&g, Some("shell"), &p, &[], &loaded(&[])).as_deref(),
			Some("no"),
		);
		let p_ok = json!({ "command": "ls -lt" });
		assert!(check(&g, Some("shell"), &p_ok, &[], &loaded(&[])).is_none());
	}

	#[test]
	fn has_capability_required() {
		let g = Guardrails::parse(
			r#"
			[[rule]]
			match = "shell(command=^ls\\b)"
			has = "filesystem"
			message = "use view"
			"#,
		)
		.unwrap();
		let p = json!({ "command": "ls -lt" });
		assert!(check(&g, Some("shell"), &p, &[], &loaded(&[])).is_none());
		assert!(check(&g, Some("shell"), &p, &[], &loaded(&["filesystem"])).is_some());
	}

	#[test]
	fn when_unused_lifts_after_use() {
		// `-filesystem` = "no filesystem call in history yet" — fires (blocks)
		// only while the user has not exercised the filesystem capability.
		let g = Guardrails::parse(
			r#"
			[[rule]]
			match = "shell(command=^ls\\b)"
			when = ["-filesystem"]
			message = "use filesystem first"
			"#,
		)
		.unwrap();
		let p = json!({ "command": "ls" });
		// Empty log → unused condition holds → block.
		assert!(check(&g, Some("shell"), &p, &[], &loaded(&[])).is_some());
		// Any filesystem call in history → unused fails → allow.
		let log: Vec<CallRecord> = vec![(
			Some("filesystem".to_string()),
			json!({ "path": "src/main.rs" }),
		)];
		assert!(check(&g, Some("shell"), &p, &log, &loaded(&[])).is_none());
	}

	#[test]
	fn when_used_requires_history() {
		// `+shell(command=git status)` = "rule fires only after git status was
		// already run". A `+` condition gates the rule on prior usage.
		let g = Guardrails::parse(
			r#"
			[[rule]]
			match = "shell(command=git push)"
			when = ["+shell(command=git status)"]
			message = "blocked because you ran git status"
			"#,
		)
		.unwrap();
		let p = json!({ "command": "git push" });
		// Empty log → `+` condition unmet → rule doesn't fire → allow.
		assert!(check(&g, Some("shell"), &p, &[], &loaded(&[])).is_none());
		// History contains git status → `+` met → rule fires → block.
		let log: Vec<CallRecord> = vec![(
			Some("shell".to_string()),
			json!({ "command": "git status" }),
		)];
		assert!(check(&g, Some("shell"), &p, &log, &loaded(&[])).is_some());
	}

	#[test]
	fn arg_array_matches_via_json() {
		let g = Guardrails::parse(
			r#"
			[[rule]]
			match = "filesystem(paths=secret\\.env)"
			message = "no secrets"
			"#,
		)
		.unwrap();
		let p = json!({ "paths": ["src/main.rs", "config/secret.env"] });
		assert_eq!(
			check(&g, Some("filesystem"), &p, &[], &loaded(&[])).as_deref(),
			Some("no secrets"),
		);
		let p_ok = json!({ "paths": ["src/main.rs"] });
		assert!(check(&g, Some("filesystem"), &p_ok, &[], &loaded(&[])).is_none());
	}

	#[test]
	fn arg_string_matched_unquoted() {
		let g = Guardrails::parse(
			r#"
			[[rule]]
			match = "shell(command=^ls$)"
			message = "no bare ls"
			"#,
		)
		.unwrap();
		let p = json!({ "command": "ls" });
		assert!(check(&g, Some("shell"), &p, &[], &loaded(&[])).is_some());
	}

	#[test]
	fn first_match_wins() {
		let g = Guardrails::parse(
			r#"
			[[rule]]
			match = "shell(command=git)"
			message = "first"
			[[rule]]
			match = "shell(command=git push)"
			message = "second"
			"#,
		)
		.unwrap();
		let p = json!({ "command": "git push" });
		assert_eq!(
			check(&g, Some("shell"), &p, &[], &loaded(&[])).as_deref(),
			Some("first"),
		);
	}
}
