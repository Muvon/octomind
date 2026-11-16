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

//! The status line the agent ends every response with: the instruction that
//! asks for it, the parser that reads it and the stripper that hides it.

use std::ops::Range;

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

/// The `plan` value a status line may carry. Octomind accepts it so a line
/// written by an agent that still emits it stays valid, and ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanSignal {
	Request,
	PhaseComplete,
	Reassess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSelfReport {
	pub state: SelfReport,
	pub handoff: SelfReportHandoff,
	/// Pack-local memory IDs that materially affected this response/action.
	pub used_memories: Vec<String>,
	/// Values of the status fields plugins registered, by field name.
	pub fields: std::collections::BTreeMap<String, serde_json::Value>,
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

const STATUS_HEAD: &str = "Finish every response with one compact JSON status line — the last line, nothing after it:\n\t`<sup>{\"state\":\"STATE\",\"focus\":\"current subgoal and why\",\"next\":\"next action\",\"carry\":[\"minimum fact or opaque reference needed after context loss\"]";

const STATUS_RULES_START: &str = "Use valid single-line JSON with exactly those fields. ";

const STATUS_RULES_END: &str = " Keep only information genuinely needed to resume. Never copy credentials or secret values into the report — retain only an opaque pointer, name, or location used to obtain them. Avoid generic text such as \"working\" or \"continuing\". STATE must be exactly one of:";

const STATUS_STATES: &str = "
- `exploring` — still gathering context, reading code
- `progressing` — actively making changes, or a background job you started is running: never poll it (`monitor list`, `ps`, `tail`, sleep, re-run, a variant, viewing its resource) and keep working on anything that does not need it (read or review code, write tests, prepare the next edit); only when nothing like that is left, reply with a short status and no tool call: its result arrives as your next user message
- `blocked` — stuck, cannot proceed
- `need_input` — asking the user a question and waiting on them
- `done` — the user's task is fully complete";

const STATUS_EXAMPLE_HEAD: &str = "Example: `<sup>{\"state\":\"progressing\",\"focus\":\"fixing tab handling in the fence parser\",\"next\":\"run the parser tests for the edited path\",\"carry\":[\"regression input is tests/fixtures/tabs.md\"]";

const STATUS_TAIL: &str = "This line is read by the system and hidden from the user. Emit exactly one.";

/// The status-line instruction octomind composes when plugins registered extra
/// fields. Without fields it is the base text.
///
/// A field whose default is an array is a list that may be empty: it joins the
/// sentence about empty values. Any other field gets its own paragraph.
pub fn status_instruction(fields: &[crate::plugins::PluginStatusField]) -> String {
	let (list_fields, scalar_fields): (Vec<_>, Vec<_>) = fields.iter().partition(|field| field.default.is_array());

	let mut template_extras = String::new();
	let mut example_extras = String::new();
	for field in fields {
		let name = serde_json::Value::from(field.name.as_str());
		template_extras.push_str(&format!(",{name}:{}", field.default));
		let example = field.example.as_ref().unwrap_or(&field.default);
		example_extras.push_str(&format!(",{name}:{example}"));
	}

	let mut emptiable: Vec<String> = vec!["`carry`".to_string()];
	emptiable.extend(list_fields.iter().map(|field| format!("`{}`", field.name)));
	let (last, earlier) = emptiable.split_last().expect("`carry` is always listed");
	let emptiable = if earlier.is_empty() {
		last.clone()
	} else {
		format!("{} and {last}", earlier.join(", "))
	};

	let mut text = format!("{STATUS_HEAD}{template_extras}}}</sup>`\n{STATUS_RULES_START}{emptiable} may be empty and `next` is `null` when nothing remains to do.");
	for field in &list_fields {
		text.push(' ');
		text.push_str(&field.instruction);
	}
	text.push_str(STATUS_RULES_END);
	text.push_str(STATUS_STATES);
	for field in &scalar_fields {
		text.push_str("\n\n");
		text.push_str(&field.instruction);
	}
	text.push_str(&format!(
		"\n{STATUS_EXAMPLE_HEAD}{example_extras}}}</sup>`\n{STATUS_TAIL}"
	));
	text
}

// Unknown keys are rejected by `is_report_key` before this is built: the allowed
// set includes the fields plugins registered, which a derive cannot know.
#[derive(serde::Deserialize)]
struct WireSelfReport {
	state: String,
	focus: String,
	// Null at `done`: a finished turn has no next action, and the schema has to
	// say so — a rejected parse loses the terminal state the gate runs on.
	next: Option<String>,
	carry: Vec<String>,
	// Accepted and ignored: the plan signal belonged to the built-in plan
	// manager, but the status line still carries it and unknown fields are
	// rejected.
	#[serde(default, rename = "plan")]
	_plan: Option<PlanSignal>,
	#[serde(default)]
	memories: Vec<String>,
	// Accepted and ignored: reports written before evolved-skill credit moved
	// to trigger exposure still carry it, and unknown fields are rejected.
	#[serde(default, rename = "behaviors")]
	_behaviors: Vec<String>,
}

/// Field names a structured self-report may carry ([`WireSelfReport`]).
const SELF_REPORT_FIELDS: &[&str] = &["state", "focus", "next", "carry", "plan", "memories", "behaviors"];

/// Names of the status fields plugins registered for the current session; none
/// outside a session.
fn registered_field_names() -> Vec<String> {
	crate::session::context::current_session_id()
		.map(|id| {
			crate::plugins::status_fields(&id)
				.into_iter()
				.map(|field| field.name)
				.collect()
		})
		.unwrap_or_default()
}

fn is_report_key(key: &str, registered: &[String]) -> bool {
	SELF_REPORT_FIELDS.contains(&key) || registered.iter().any(|name| name == key)
}

/// A bare JSON report on the last line, identified by its own fields when the
/// model omits the framing. Other JSON objects must remain visible.
fn untagged_trailing_report(text: &str) -> Option<(usize, &str)> {
	let trimmed = text.trim_end();
	let start = trimmed.rfind('\n').map_or(0, |newline| newline + 1);
	let body = trimmed[start..].trim();
	let fields: serde_json::Map<String, serde_json::Value> = serde_json::from_str(body).ok()?;
	if !(fields.contains_key("state") && fields.contains_key("focus")) {
		return None;
	}
	let registered = registered_field_names();
	let is_report = fields.keys().all(|key| is_report_key(key, &registered));
	is_report.then_some((start, body))
}

const SELF_REPORT_OPEN_TAGS: &[&str] = &["<sup>", "<sub>"];
const SELF_REPORT_CLOSE_TAGS: &[&str] = &["</sup>", "</sub>"];

fn find_self_report_tag(text: &str, tags: &[&str]) -> Option<(usize, usize)> {
	tags.iter()
		.filter_map(|tag| text.find(tag).map(|start| (start, tag.len())))
		.min_by_key(|(start, _)| *start)
}

/// Parsing and display use the same report boundaries. Models can mismatch
/// superscript/subscript tags or truncate a structured report; neither should
/// expose runtime notes. Ordinary HTML and prose mentions of the tags survive.
fn tagged_self_reports(text: &str) -> impl Iterator<Item = (Range<usize>, &str)> {
	let mut offset = 0;
	std::iter::from_fn(move || {
		while let Some((start, len)) = find_self_report_tag(&text[offset..], SELF_REPORT_OPEN_TAGS) {
			let start = offset + start;
			let body_start = start + len;
			offset = body_start;
			let closing = find_self_report_tag(&text[body_start..], SELF_REPORT_CLOSE_TAGS);
			let (body_end, end) = match closing {
				Some((relative, len)) => (body_start + relative, body_start + relative + len),
				None => (text.len(), text.len()),
			};
			let inner = text[body_start..body_end].trim();
			// An earlier prose mention must not consume a later report's closing tag.
			if find_self_report_tag(inner, SELF_REPORT_OPEN_TAGS).is_some()
				|| !is_self_report_body(inner)
				|| (closing.is_none() && !inner.starts_with('{'))
			{
				continue;
			}
			offset = end;
			return Some((start..end, inner));
		}
		None
	})
}

fn last_tagged_report(text: &str) -> Option<&str> {
	tagged_self_reports(text).last().map(|(_, body)| body)
}

pub fn parse_self_report_handoff(text: &str) -> Option<ParsedSelfReport> {
	// The untagged form only matches on the last line, so it wins over an
	// earlier tagged pair, which may be superscript the user wrote.
	let inner = match untagged_trailing_report(text) {
		Some((_, body)) => body,
		None => last_tagged_report(text)?,
	};
	if inner.starts_with('{') {
		let raw: serde_json::Map<String, serde_json::Value> = serde_json::from_str(inner).ok()?;
		let registered = registered_field_names();
		if !raw.keys().all(|key| is_report_key(key, &registered)) {
			return None;
		}
		let fields = registered
			.iter()
			.filter_map(|name| raw.get(name).map(|value| (name.clone(), value.clone())))
			.collect();
		let wire: WireSelfReport = serde_json::from_value(serde_json::Value::Object(raw)).ok()?;
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
			used_memories: wire
				.memories
				.into_iter()
				.map(|id| id.trim().to_string())
				.filter(|id| !id.is_empty())
				.collect(),
			fields,
		});
	}

	let (state, reason) = parse_legacy_self_report_inner(inner)?;
	Some(ParsedSelfReport {
		state,
		handoff: SelfReportHandoff {
			focus: reason.unwrap_or_default(),
			..Default::default()
		},
		used_memories: Vec::new(),
		fields: std::collections::BTreeMap::new(),
	})
}

/// Parse the *last* `<sup>…</sup>` token from a response. Returns the state and
/// an optional short reason. Tolerant of the `·` or `|` reason separator.
/// Test-only harness for the legacy parse path; the runtime reaches it through
/// [`parse_self_report_handoff`]'s fallback.
#[cfg(test)]
fn parse_self_report(text: &str) -> Option<(SelfReport, Option<String>)> {
	let inner = last_tagged_report(text)?;
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
	let reason = after.trim_start_matches([' ', '·', '|', ':', '-', '\t']).trim();
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

/// Remove internal reports while leaving legitimate HTML untouched. A trailing
/// bare report is recognized by its fields instead of its framing.
pub fn strip_self_report(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut offset = 0;
	for (range, _) in tagged_self_reports(text) {
		out.push_str(&text[offset..range.start]);
		offset = range.end;
	}
	out.push_str(&text[offset..]);
	let out = out.trim_end();
	match untagged_trailing_report(out) {
		Some((start, _)) => out[..start].trim_end().to_string(),
		None => out.to_string(),
	}
}

#[cfg(test)]
#[path = "self_report_tests.rs"]
pub(crate) mod tests;
