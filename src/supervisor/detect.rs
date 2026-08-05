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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfReport {
	Exploring,
	Progressing,
	Blocked,
	NeedInput,
	Done,
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
}

/// One-time system-side instruction that makes the agent self-annotate. Injected
/// out-of-band; the resulting tags are stripped before display.
pub const SELF_REPORT_INSTRUCTION: &str = "\
Finish every response with one status line — the last line, nothing after it:
`<sup>STATE · brief reason</sup>`
Start the tag with one state word, then ` · `, then a few words of reason. Use exactly one of these words (write the word itself, not the placeholder STATE):
- `exploring` — still gathering context, reading code
- `progressing` — actively making changes
- `blocked` — stuck, cannot proceed
- `need_input` — asking the user a question and waiting on them
- `done` — the user's task is fully complete
Examples: `<sup>progressing · wiring store registration</sup>` or `<sup>done · migration verified</sup>`
This line is read by the system and hidden from the user. Emit exactly one, leading with the state word.";

/// One-time system-side instruction enabling evidence-bound claims. The agent
/// backs load-bearing factual claims with a verbatim quote it actually saw in a
/// tool result; [`unverified_citations`] then deterministically checks the
/// quote really occurs in some tool output, catching fabricated citations for
/// free (no model call). The tag is parseable markup: it stays in the message
/// history (so the check and the model's own context keep it) but is stripped
/// from user-facing display by [`strip_evidence`].
pub const EVIDENCE_INSTRUCTION: &str = "\
Before you assert a load-bearing fact about the task material (a path, value, statement, figure, or concrete behavior you were given or found), copy the supporting text you actually saw in a tool result, character-for-character, in this exact form:
<evidence locator=\"source:location\">exact text copied verbatim from the tool output</evidence>
The quote must be a literal copy that string-matches the tool output — rewording, summarizing, or trimming it breaks the match. Quote one line per tag; for a multi-line passage use one tag per line. Tag only load-bearing factual claims, not plans, reasoning, or general knowledge. If you cannot find text that supports a claim, say so and drop it — do not invent a quote. A quote not found in any tool result you received will be flagged to re-ground against real output or retract. Evidence tags are system metadata: they are hidden from the user, so never rely on them to carry your answer — state the substance in plain text as well.";

/// Parse the *last* `<sup>…</sup>` token from a response. Returns the state and
/// an optional short reason. Tolerant of the `·` or `|` reason separator.
pub fn parse_self_report(text: &str) -> Option<(SelfReport, Option<String>)> {
	let end = text.rfind("</sup>")?;
	let start = text[..end].rfind("<sup>")? + "<sup>".len();
	let inner = text[start..end].trim();
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
fn is_self_report_body(inner: &str) -> bool {
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

/// Deterministic evidence check: return the quoted lines inside
/// `<evidence>…</evidence>` tags in `response` that do NOT appear in any of
/// `tool_outputs`. Matching is line-wise: every non-empty line of a quote,
/// whitespace-normalized, must occur in the normalized haystack. Line-wise
/// (not whole-quote) matching is what tolerates tool outputs that inject
/// per-line decoration — e.g. `view` prefixes every line with `NN:` — which a
/// joined multi-line quote can never string-match across. Empty result =
/// every cited line is grounded (or none were cited). No model call.
pub fn unverified_citations(response: &str, tool_outputs: &[String]) -> Vec<String> {
	let quotes = extract_quotes(response);
	if quotes.is_empty() {
		return Vec::new();
	}
	let haystack = tool_outputs
		.iter()
		.map(|o| normalize_ws(o))
		.collect::<Vec<_>>()
		.join("\n");
	let mut bad = Vec::new();
	for q in quotes {
		for line in q.lines().map(normalize_ws).filter(|l| !l.is_empty()) {
			if !haystack.contains(&line) {
				bad.push(line);
			}
		}
	}
	bad
}

/// Extract the quote text inside each `<evidence …>…</evidence>` tag (the
/// contract in [`EVIDENCE_INSTRUCTION`]). The locator attribute is metadata
/// for the reader, not checked here — only the quote body is verified against
/// tool outputs. Literal examples inside Markdown code spans/fences, escaped
/// markup, and longer tag names such as `<evidence-note>` are not citations.
fn extract_quotes(text: &str) -> Vec<String> {
	let mut out = Vec::new();
	let literal_ranges = markdown_code_ranges(text);
	let mut cursor = 0;
	while let Some(tag) = next_evidence_tag(text, cursor, &literal_ranges) {
		let q = text[tag.body_start..tag.body_end].trim();
		if !q.is_empty() {
			out.push(q.to_string());
		}
		cursor = tag.end;
	}
	out
}

/// Remove `<evidence …>…</evidence>` tags for user-facing display. The tags
/// stay in the stored message (the verifier and the model's own context need
/// them); only the screen rendering drops them. Unclosed tags are left
/// untouched — partial markup is a model error the user should see. Literal
/// tag examples in Markdown code or escaped markup remain visible.
pub fn strip_evidence(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let literal_ranges = markdown_code_ranges(text);
	let mut copied_to = 0;
	let mut cursor = 0;
	while let Some(tag) = next_evidence_tag(text, cursor, &literal_ranges) {
		out.push_str(&text[copied_to..tag.start]);
		copied_to = tag.end;
		cursor = tag.end;
	}
	out.push_str(&text[copied_to..]);
	out.trim().to_string()
}

#[derive(Clone, Copy)]
struct EvidenceTag {
	start: usize,
	body_start: usize,
	body_end: usize,
	end: usize,
}

/// Find the next real metadata tag. Markdown code is a literal-content boundary:
/// examples shown to a user must not become supervisor instructions merely
/// because they contain the same bytes as the internal protocol.
fn next_evidence_tag(
	text: &str,
	from: usize,
	literal_ranges: &[std::ops::Range<usize>],
) -> Option<EvidenceTag> {
	const OPEN: &str = "<evidence";
	const CLOSE: &str = "</evidence>";
	let mut search_from = from;
	while let Some(relative) = text[search_from..].find(OPEN) {
		let start = search_from + relative;
		let after_name = start + OPEN.len();
		let boundary = text.as_bytes().get(after_name).copied();
		let valid_name =
			boundary.is_some_and(|b| b == b'>' || b == b'/' || b.is_ascii_whitespace());
		let literal_index = literal_ranges.partition_point(|range| range.end <= start);
		let in_literal = literal_ranges
			.get(literal_index)
			.is_some_and(|range| range.contains(&start));
		if !valid_name || is_backslash_escaped(text.as_bytes(), start) || in_literal {
			search_from = after_name;
			continue;
		}

		let after = &text[after_name..];
		let gt = after.find('>')?;
		if after[..gt].trim_end().ends_with('/') {
			search_from = after_name + gt + 1;
			continue;
		}
		let body_start = after_name + gt + 1;
		let body_end = body_start + text[body_start..].find(CLOSE)?;
		return Some(EvidenceTag {
			start,
			body_start,
			body_end,
			end: body_end + CLOSE.len(),
		});
	}
	None
}

fn is_backslash_escaped(bytes: &[u8], offset: usize) -> bool {
	let mut count = 0;
	let mut cursor = offset;
	while cursor > 0 && bytes[cursor - 1] == b'\\' {
		count += 1;
		cursor -= 1;
	}
	count % 2 == 1
}

/// Byte ranges occupied by Markdown inline code spans or fenced code blocks.
/// This intentionally recognizes only Markdown's structural delimiters; prose,
/// XML, and ordinary source quotes remain eligible evidence bodies.
fn markdown_code_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
	let fenced = fenced_code_ranges(text);
	let mut ranges = Vec::new();
	let bytes = text.as_bytes();
	let mut cursor = 0;
	let mut fence_index = 0;
	while cursor < bytes.len() {
		while fenced
			.get(fence_index)
			.is_some_and(|range| range.end <= cursor)
		{
			fence_index += 1;
		}
		if let Some(range) = fenced
			.get(fence_index)
			.filter(|range| range.contains(&cursor))
		{
			cursor = range.end;
			fence_index += 1;
			continue;
		}
		if bytes[cursor] != b'`' {
			cursor += 1;
			continue;
		}
		let run = delimiter_run(bytes, cursor, b'`');
		let mut close = cursor + run;
		let mut found = None;
		while close < bytes.len() {
			if fenced
				.get(fence_index)
				.is_some_and(|range| close >= range.start)
			{
				break;
			}
			if bytes[close] == b'`' {
				let close_run = delimiter_run(bytes, close, b'`');
				if close_run == run {
					found = Some(close + close_run);
					break;
				}
				close += close_run;
			} else {
				close += 1;
			}
		}
		if let Some(end) = found {
			ranges.push(cursor..end);
			cursor = end;
		} else {
			cursor += run;
		}
	}
	ranges.extend(fenced);
	ranges.sort_by_key(|range| range.start);
	ranges
}

fn fenced_code_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
	let mut ranges = Vec::new();
	let mut fence: Option<(u8, usize, usize)> = None;
	let mut line_start = 0;
	for line in text.split_inclusive('\n') {
		let line_end = line_start + line.len();
		let content = line.strip_suffix('\n').unwrap_or(line);
		let indent = content
			.as_bytes()
			.iter()
			.take_while(|b| **b == b' ')
			.count();
		if indent <= 3 {
			let marker = content.as_bytes().get(indent).copied();
			match fence {
				Some((kind, width, start)) if marker == Some(kind) => {
					let run = delimiter_run(content.as_bytes(), indent, kind);
					if run >= width && content[indent + run..].trim().is_empty() {
						ranges.push(start..line_end);
						fence = None;
					}
				}
				None if matches!(marker, Some(b'`' | b'~')) => {
					let kind = marker.expect("matched marker");
					let run = delimiter_run(content.as_bytes(), indent, kind);
					if run >= 3 {
						fence = Some((kind, run, line_start));
					}
				}
				_ => {}
			}
		}
		line_start = line_end;
	}
	if let Some((_, _, start)) = fence {
		ranges.push(start..text.len());
	}
	ranges
}

fn delimiter_run(bytes: &[u8], start: usize, delimiter: u8) -> usize {
	bytes[start..]
		.iter()
		.take_while(|byte| **byte == delimiter)
		.count()
}

/// Collapse every run of whitespace to a single space and trim — the normal form
/// both sides of the evidence check are compared in.
fn normalize_ws(s: &str) -> String {
	s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Deterministic evidence check: absolute URLs cited in `response` that appear
/// in NONE of `grounds` (tool outputs plus the current user turn — a link the
/// user supplied is a legitimate thing to reference back). A cited source link
/// the agent never fetched and was never given is an invented source. Trailing
/// prose/markdown punctuation is stripped, and a trailing-slash variant is
/// accepted, so only genuinely absent links are flagged. No model call.
pub fn unverified_urls(response: &str, grounds: &[String]) -> Vec<String> {
	static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
	let re =
		RE.get_or_init(|| regex::Regex::new(r#"https?://[^\s<>"')\]]+"#).expect("static pattern"));
	let haystack = grounds
		.iter()
		.map(|o| normalize_ws(o))
		.collect::<Vec<_>>()
		.join("\n");
	let mut seen = std::collections::HashSet::new();
	let mut flagged = Vec::new();
	for m in re.find_iter(response) {
		let url = m
			.as_str()
			.trim_end_matches(['.', ',', ';', ':', '!', '?', '/']);
		if url.is_empty() || !seen.insert(url.to_string()) {
			continue;
		}
		// Grounded when the URL (with or without a trailing slash) appears in
		// anything the agent actually received this turn — OR when it is a
		// permalink CONSTRUCTED from verified pieces: `git remote` + `rev-parse`
		// + diff paths never occur as one concatenated URL in any tool output,
		// but every significant segment (host, repo, commit SHA, file name)
		// does. Require ≥2 significant segments so a bare domain never passes.
		let with_slash = format!("{url}/");
		if haystack.contains(url) || haystack.contains(&with_slash) {
			continue;
		}
		// A `#fragment` is client-side navigation, not a different resource:
		// citing `url#section` when the fetched output carries the bare `url`
		// is grounded.
		let path = url.split('#').next().unwrap_or(url);
		if path != url {
			let path_slash = format!("{path}/");
			if haystack.contains(path) || haystack.contains(&path_slash) {
				continue;
			}
		}
		let segments: Vec<&str> = path.split('/').filter(|s| s.len() >= 8).collect();
		let constructed = segments.len() >= 2 && segments.iter().all(|s| haystack.contains(s));
		if !constructed {
			flagged.push(url.to_string());
		}
	}
	flagged
}

/// Deterministic evidence check: `file:line` references in `response` that do
/// not hold on disk — the file is missing, or the line number is beyond EOF.
/// High-precision by construction: only paths containing a `/` and an extension
/// starting with a letter are checked (bare `x.rs:3` or version-like `1.2:3`
/// never match), URL interiors are excluded by the preceding-char guard, and
/// GitHub-style `#L<n>` anchors count as line references too.
///
/// A reference resolves against every root the turn actually evidences, never
/// the process cwd alone (a task legitimately spans other repos and working
/// directories):
/// - as given (cwd-relative or absolute);
/// - as a suffix of another path cited in full in the same response (prose
///   often abbreviates: "deals/create.php:199" after
///   "app/actions/deals/create.php:199");
/// - as a suffix of a path occurring in `grounds` (tool outputs plus the
///   agent's executed call arguments) that exists on disk — a file the agent
///   actually touched under another root this turn.
/// Every candidate that resolves is line-bound-checked. A path that resolves
/// nowhere but occurs verbatim in `grounds` at a path boundary was received,
/// not fabricated — benefit of the doubt (the line cannot be checked). Only a
/// reference with no on-disk candidate and no occurrence in anything the
/// agent received or executed is flagged. No model call.
pub fn unverified_file_refs(response: &str, grounds: &[String]) -> Vec<String> {
	static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
	let re = RE.get_or_init(|| {
		regex::Regex::new(
			r"(?:^|[^A-Za-z0-9_./:-])(/?(?:[A-Za-z0-9_@~.-]+/)+[A-Za-z0-9_.-]+\.[A-Za-z][A-Za-z0-9]{0,7})(?::|#L)([0-9]+)",
		)
		.expect("static pattern")
	});
	// Bound-check only files we can cheaply read as text; an unreadable or
	// oversized file gets the benefit of the doubt (None = cannot check).
	const MAX_CHECKED_FILE: u64 = 2_000_000;
	fn line_count(path: &str) -> Option<usize> {
		let p = std::path::Path::new(path);
		if p.metadata().ok()?.len() > MAX_CHECKED_FILE {
			return None;
		}
		Some(std::fs::read_to_string(p).ok()?.lines().count())
	}
	let mut refs: Vec<(String, usize)> = Vec::new();
	let mut seen = std::collections::HashSet::new();
	for cap in re.captures_iter(response) {
		let Ok(line) = cap[2].parse::<usize>() else {
			continue;
		};
		if seen.insert(format!("{}:{}", &cap[1], line)) {
			refs.push((cap[1].to_string(), line));
		}
	}
	let existing: Vec<String> = refs
		.iter()
		.map(|(p, _)| p.clone())
		.filter(|p| std::path::Path::new(p).exists())
		.collect();
	let haystack = grounds.join("\n");
	let mut flagged = Vec::new();
	for (path, line) in &refs {
		let key = format!("{path}:{line}");
		let mut candidates: Vec<String> = if std::path::Path::new(path).exists() {
			vec![path.clone()]
		} else {
			let suffix = format!("/{path}");
			existing
				.iter()
				.filter(|full| full.ends_with(&suffix))
				.cloned()
				.collect()
		};
		let (ground_candidates, received) = grounds_path_evidence(path, &haystack);
		candidates.extend(ground_candidates);
		if candidates.is_empty() {
			if !received {
				flagged.push(format!("{key} (file not found)"));
			}
			continue;
		}
		// The reference holds if the line lands inside ANY candidate; an
		// uncheckable candidate also passes (skip, not a flag).
		let mut beyond_eof = None;
		for c in &candidates {
			match line_count(c) {
				Some(count) if *line == 0 || *line > count => beyond_eof = Some(count),
				_ => {
					beyond_eof = None;
					break;
				}
			}
		}
		if let Some(count) = beyond_eof {
			flagged.push(format!("{key} (file has only {count} lines)"));
		}
	}
	flagged
}

/// Path evidence for `path` inside the joined grounds: full on-disk paths that
/// end with `/{path}` (each occurrence expanded backwards over path
/// characters — recovers the absolute path a tool call or output named), and
/// whether the grounds contain `path` at a path boundary at all ("received").
/// Boundaries are enforced both ways: `a/b.ts` inside `a/b.tsx` is a different
/// file, and `zz/a/b.ts` inside `xzz/a/b.ts` is a different path.
fn grounds_path_evidence(path: &str, haystack: &str) -> (Vec<String>, bool) {
	let bytes = haystack.as_bytes();
	let mut candidates = Vec::new();
	let mut received = false;
	let mut from = 0;
	while let Some(rel) = haystack[from..].find(path) {
		let start = from + rel;
		let end = start + path.len();
		from = start + 1;
		if bytes.get(end).is_some_and(|b| b.is_ascii_alphanumeric()) {
			continue;
		}
		let boundary = start == 0 || bytes[start - 1] == b'/' || !is_path_byte(bytes[start - 1]);
		if !boundary {
			continue;
		}
		received = true;
		let mut s = start;
		while s > 0 && is_path_byte(bytes[s - 1]) {
			s -= 1;
		}
		if s < start {
			let full = &haystack[s..end];
			if !candidates.iter().any(|c| c == full) && std::path::Path::new(full).exists() {
				candidates.push(full.to_string());
			}
		}
	}
	(candidates, received)
}

/// Chars that form a path token in surrounding text — the component charset of
/// the reference regex plus the separator.
fn is_path_byte(b: u8) -> bool {
	b.is_ascii_alphanumeric() || matches!(b, b'_' | b'@' | b'~' | b'.' | b'-' | b'/')
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
	if is_mutation_tool(tool) {
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

/// Does this call already carry range/limit parameters — i.e. the model IS
/// narrowing (paging a large result with start/end, capping with limit)? The
/// truncation steer diagnoses "re-querying without narrowing"; a paged call's
/// truncation is the display cap, not ignored advice, so it must not feed the
/// streak.
pub fn has_narrowing_params(parameters: &serde_json::Value) -> bool {
	const NARROWING: [&str; 6] = [
		"start",
		"end",
		"offset",
		"limit",
		"max_results",
		"max_count",
	];
	NARROWING.iter().any(|k| parameters.get(k).is_some())
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

/// Heuristic: does this tool change state, so a success is inherently progress?
/// (Reads/searches only count as progress when they surface *new* content.)
pub fn is_mutation_tool(tool: &str) -> bool {
	let t = tool.to_ascii_lowercase();
	[
		"write",
		"edit",
		"create",
		"str_replace",
		"apply",
		"insert",
		"delete",
		"remove",
		"patch",
		"mkdir",
		"rename",
		"move",
	]
	.iter()
	.any(|k| t.contains(k))
}

const SEEN_CAP: usize = 128;

/// Cap on remembered agent-mutated paths (read-back verification candidates).
/// Oldest evicted — a task touching more artifacts than this verifies via the
/// most recent ones, which is where the read-back lands anyway.
const MUTATED_PATHS_CAP: usize = 32;

/// Calls used to seed the working-set centroid before drift scoring begins —
/// too few and the first off-task call would define the baseline.
const CENTROID_WARMUP: usize = 3;
/// EMA weight for folding a new on-task call into the centroid. Higher tracks
/// topic shifts faster but is noisier; lower is stabler but lags.
const CENTROID_ALPHA: f32 = 0.3;

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
	/// Truncated tool results in a row. Reset by any non-truncated result.
	consecutive_truncations: usize,
	/// Deduplicated results in a row. Reset by any non-dedup result.
	consecutive_dedups: usize,
	/// Off-task results in a row (drift). Reset by an on-task result or new task.
	consecutive_drift: usize,
	/// Single-tool-call rounds in a row. A round is a full AI turn's tool batch;
	/// when it carries exactly one call and the model could have batched independent
	/// calls, the streak grows. Reset by any multi-call (parallel) round.
	consecutive_singletons: usize,
	/// The current round carried at least one errored call (set by [`Detectors::note_call`],
	/// consumed by [`Detectors::record_round_arity`]). An errored round — and the
	/// singleton retry that follows it — is recovery, not a batchable drip-feed,
	/// so neither may grow the singleton streak.
	round_had_error: bool,
	/// The PREVIOUS completed round carried an errored call.
	prev_round_had_error: bool,
	/// EMA centroid of recent ON-TASK result embeddings — the "working set". A
	/// result far from it (low cosine) is drift. Empty until the first one seeds it.
	centroid: Vec<f32>,
	/// Calls folded into `centroid` since the last reset (cold-start gate + EMA).
	centroid_count: usize,
	/// Hash of the user task the `centroid` belongs to; a change resets it so the
	/// working set never carries across turns (see [`Detectors::note_task`]).
	task_hash: Option<u64>,
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
	/// drift between rounds (the user editing their tree mid-session) never
	/// arms it: an agent that changed nothing has nothing to verify.
	agent_dirty: bool,
	/// Paths the agent's own successful mutation-shaped calls touched since the
	/// last clean verification — the artifacts a later read-back can verify.
	/// Normalized ([`normalize_path`]), deduped, capped at [`MUTATED_PATHS_CAP`]
	/// (oldest evicted). Cleared with `agent_dirty`: once a round verifies, the
	/// artifacts are accepted state and a fresh mutation restarts the set.
	mutated_paths: Vec<String>,
	/// Sticky latch: the user explicitly forbade running checks ("don't run
	/// tests — I'll run it myself"). The resolver judges one turn at a time, so
	/// without the latch a later turn loses the prohibition and the pre-gate
	/// nudges the agent to violate it. Standing rule: persists for the session
	/// (an incidental check run must not erase the user's prohibition; the
	/// latch only suppresses the pre-gate nudge — it never blocks the agent
	/// from running a check the user directly asks for, and the LLM gate still
	/// judges prohibitions on its own).
	forbids_verification: bool,
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
	/// `truncation_threshold` truncated results in a row — the model is ignoring
	/// the truncation notice and re-querying without narrowing. Tool-agnostic:
	/// the model varies args so each truncated chunk differs (defeating Loop) and
	/// reads as fresh (defeating NoProgress); the only invariant is truncation.
	Truncation,
	/// `dedup_threshold` deduplicated results in a row — the model is re-issuing
	/// calls whose output it already received this session (the body was elided to
	/// an error placeholder). The most precise loop there is: the exact same call.
	Dedup,
	/// `distraction_threshold` off-task RESULTS in a row — results whose cosine to
	/// the working-set centroid (recent on-task results) fell below `drift_floor`.
	/// Self-referential, so no task anchor is needed (robust to abstract requests).
	/// We score the result, not the call: short call strings are format-dominated
	/// and don't separate by topic (measured); results carry real content and do.
	Distraction,
	/// `sequential_threshold` single-tool-call ROUNDS in a row — the model is
	/// issuing one call per turn where independent calls could be batched into one
	/// parallel round. Round-level (not per-call); recorded once per turn via
	/// [`Detectors::record_round_arity`]. Off by default — single calls are often
	/// legitimate, so this is the softest, most conservative signal.
	Sequential,
}

impl DetectorSignal {
	/// Severity rank — higher wins when merging signals from a parallel batch.
	/// Mirrors the priority in `record_round_signals`'s return cascade.
	fn priority(self) -> u8 {
		match self {
			Self::None => 0,
			Self::Sequential => 1,
			Self::Distraction => 2,
			Self::NoProgress => 3,
			Self::Loop => 4,
			Self::Truncation => 5,
			Self::Dedup => 6,
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
		self.round_had_error |= is_error;
		(rhash, novel)
	}

	/// Decide the deterministic signal for ONE completed tool round. A parallel batch
	/// is ONE model decision, so the whole round is observed as a single unit — N
	/// identical / truncated / deduped / off-task calls in one shot count once, not N
	/// (the model has not yet seen the notices it is being asked to act on). Inputs
	/// are aggregated across the round by the caller: `call_hashes` are the per-call
	/// result hashes (from [`Detectors::note_call`]); the booleans are OR-folded over
	/// the round. Returns the highest-priority fired signal.
	#[allow(clippy::too_many_arguments)]
	pub fn record_round_signals(
		&mut self,
		call_hashes: &[u64],
		round_novel: bool,
		round_truncated: bool,
		round_dedup: bool,
		round_drift: bool,
		loop_threshold: usize,
		no_progress_window: usize,
		truncation_threshold: usize,
		dedup_threshold: usize,
		distraction_threshold: usize,
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

		// Truncation streak: consecutive ROUNDS hitting a capped output. The most
		// specific, most actionable diagnosis — the model keeps re-querying without
		// narrowing, which slips past Loop and NoProgress.
		if round_truncated {
			self.consecutive_truncations += 1;
		} else {
			self.consecutive_truncations = 0;
		}

		// Dedup streak: consecutive ROUNDS re-issuing a call whose output the model
		// already has — the most precise loop there is, so it fires on its own (low)
		// threshold.
		if round_dedup {
			self.consecutive_dedups += 1;
		} else {
			self.consecutive_dedups = 0;
		}

		// Drift streak: consecutive off-task ROUNDS. Scored upstream against the
		// working-set centroid (see `note_result`); the softest signal — context
		// quality, not an active loop — so it is ranked last.
		if round_drift {
			self.consecutive_drift += 1;
		} else {
			self.consecutive_drift = 0;
		}

		let deduping = dedup_threshold > 0 && self.consecutive_dedups >= dedup_threshold;
		let truncating =
			truncation_threshold > 0 && self.consecutive_truncations >= truncation_threshold;
		let distracting =
			distraction_threshold > 0 && self.consecutive_drift >= distraction_threshold;

		// Priority cascade — mirrors DetectorSignal::priority (Dedup > Truncation >
		// Loop > NoProgress > Distraction).
		if deduping {
			DetectorSignal::Dedup
		} else if truncating {
			DetectorSignal::Truncation
		} else if looping {
			DetectorSignal::Loop
		} else if stalled {
			DetectorSignal::NoProgress
		} else if distracting {
			DetectorSignal::Distraction
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
			if n.is_empty() || self.mutated_paths.contains(&n) {
				continue;
			}
			if self.mutated_paths.len() >= MUTATED_PATHS_CAP {
				self.mutated_paths.remove(0);
			}
			self.mutated_paths.push(n);
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
	pub fn note_round_verification(
		&mut self,
		fp_before: Option<u64>,
		fp_after: Option<u64>,
		verifier_ok: bool,
		readback_ok: bool,
		mutation_ok: bool,
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
		if (verifier_ok || readback_ok) && tree_unchanged {
			if let Some(a) = fp_after {
				self.verified_fp = Some(a);
			}
			self.agent_dirty = false;
			self.mutated_paths.clear();
		} else if !tree_unchanged {
			self.agent_dirty = true;
		}
		crate::log_debug!(
			"round verification: tree_unchanged={} verifier={} readback={} -> verified_fp={:?} agent_dirty={}",
			tree_unchanged,
			verifier_ok,
			readback_ok,
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
		self.consecutive_truncations = 0;
		self.consecutive_dedups = 0;
		self.consecutive_drift = 0;
		self.consecutive_singletons = 0;
		self.round_had_error = false;
		self.prev_round_had_error = false;
		self.agent_dirty = false;
		self.mutated_paths.clear();
	}

	/// Latch that the user forbade running checks ("don't run tests — I'll run
	/// it myself"). The resolver judges one turn at a time, and the prohibition
	/// is a standing rule, not a per-turn property — so the latch persists
	/// across turns like `verified_fp` (a later turn that merely fails to
	/// repeat the rule must not re-arm the pre-gate). It never auto-expires:
	/// an incidental check run does not mean the user revoked the rule, and
	/// the latch only suppresses the pre-gate nudge — checks the user directly
	/// requests still run, and the LLM gate judges prohibitions on its own.
	pub fn note_forbids_verification(&mut self) {
		self.forbids_verification = true;
	}

	/// Whether the user has forbidden running checks (standing rule for the
	/// session — see [`Detectors::note_forbids_verification`]).
	pub fn check_run_forbidden(&self) -> bool {
		self.forbids_verification
	}

	/// Record the arity of a completed tool round (one AI turn's batch) and return
	/// the sequential-batching signal. `call_count` is how many tool calls the round
	/// carried; a round of exactly one grows the singleton streak, any parallel round
	/// (>= 2) resets it. Fires `Sequential` once the streak reaches `threshold`.
	/// `threshold == 0` disables the signal entirely (the default). Round-level, so
	/// it is recorded once per turn — separately from the per-round
	/// [`Detectors::record_round_signals`].
	pub fn record_round_arity(&mut self, call_count: usize, threshold: usize) -> DetectorSignal {
		// A round containing an errored call — or the singleton RETRY right after
		// one — is recovery, not a silent drip-feed of independent calls: it must
		// not grow the streak (but does not reset a legitimately accumulated one).
		let recovery = self.round_had_error || self.prev_round_had_error;
		if call_count > 1 {
			self.consecutive_singletons = 0;
		} else if call_count == 1 && !recovery {
			self.consecutive_singletons += 1;
		}
		self.prev_round_had_error = self.round_had_error;
		self.round_had_error = false;
		if threshold > 0 && self.consecutive_singletons >= threshold {
			DetectorSignal::Sequential
		} else {
			DetectorSignal::None
		}
	}

	/// Reset the single-call streak. Called after a `Sequential` steer — so the
	/// advisory nudge waits a full `sequential_threshold` single-call rounds before
	/// firing again instead of every turn (spam) — and on a final message to the user
	/// (need_input / done): a hand-back is not a silent drip-feed of independent calls.
	pub fn reset_sequential_streak(&mut self) {
		self.consecutive_singletons = 0;
	}

	/// Update the working-set centroid with this result's embedding and return
	/// whether the result drifted off it (cosine below `floor`). Self-referential:
	/// it scores the result against what the agent has recently worked with, so it
	/// needs no task anchor and is robust to abstract requests. Only NON-drift
	/// results are folded in, so a sustained wander can't pull the centroid with it;
	/// the first `CENTROID_WARMUP` results seed it and never count as drift.
	pub fn note_result(&mut self, emb: &[f32], floor: f32) -> bool {
		if self.centroid.len() != emb.len() {
			// First result since a reset (or a dimension change): seed, never drift.
			self.centroid = emb.to_vec();
			self.centroid_count = 1;
			return false;
		}
		let sim = crate::embeddings::cosine(&self.centroid, emb);
		let warming = self.centroid_count < CENTROID_WARMUP;
		let is_drift = !warming && sim < floor;
		if !is_drift {
			for (c, &e) in self.centroid.iter_mut().zip(emb) {
				*c = (1.0 - CENTROID_ALPHA) * *c + CENTROID_ALPHA * e;
			}
			self.centroid_count = self.centroid_count.saturating_add(1);
		}
		is_drift
	}

	/// Reset the working-set centroid when the user's task changes (a new turn):
	/// the centroid is the CURRENT task's calls and must not carry across. `h` is a
	/// cheap hash of the latest real user message — NOT an embedding, so this never
	/// hits the abstract-request problem the scoring side avoids by construction.
	pub fn note_task(&mut self, h: u64) {
		if self.task_hash != Some(h) {
			self.task_hash = Some(h);
			self.reset_working_set();
		}
	}

	/// Drop the working set so the next calls re-seed it. Called on a task change
	/// and after a Distraction steer — a legit pivot then re-seeds instead of being
	/// flagged forever (drift results are never folded into the stale centroid).
	pub fn reset_working_set(&mut self) {
		self.centroid.clear();
		self.centroid_count = 0;
		self.consecutive_drift = 0;
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
}

/// Fuse the deterministic signal with the agent's free self-report (no model
/// call). The decision table:
/// - any `done`                          → defer to the verify-gate (no steer)
/// - no-progress / distraction while `exploring` → wait (legitimate exploration)
/// - dedup, truncation, loop, no-progress, distraction → steer
///
/// Truncation steers even while `exploring`: re-hitting a capped output is waste
/// regardless of intent, unlike a no-progress window which can be legitimate
/// exploration.
pub fn should_steer(signal: DetectorSignal, report: Option<SelfReport>) -> bool {
	if signal == DetectorSignal::None {
		return false;
	}
	match report {
		Some(SelfReport::Done) => false,
		// No-progress and distraction can be legitimate while exploring; every
		// other signal steers regardless of intent. Sequential is NOT suppressed
		// here: serializing independent calls is never "legitimate exploring" —
		// the detector already gates on N consecutive single-call rounds, so the
		// exploring excuse double-gates and leaves Opus unsteered.
		Some(SelfReport::Exploring)
			if matches!(
				signal,
				DetectorSignal::NoProgress | DetectorSignal::Distraction
			) =>
		{
			false
		}
		_ => true,
	}
}

/// Short human description of a fired signal — for the user-facing
/// `· Supervisor: steering — …` notice.
pub fn signal_description(signal: DetectorSignal) -> &'static str {
	match signal {
		DetectorSignal::Loop => "repeated action without new results",
		DetectorSignal::NoProgress => "no new information in recent steps",
		DetectorSignal::Truncation => "repeated truncated results — narrowing args ignored",
		DetectorSignal::Dedup => "repeated identical results — re-fetching output already received",
		DetectorSignal::Distraction => "off-task results — drifting from the current line of work",
		DetectorSignal::Sequential => {
			"single tool calls in a row — independent calls could be batched"
		}
		DetectorSignal::None => "",
	}
}

/// Shared persistent-failure frame: the model has been steered through the full
/// 0→1→2 ladder on a *stuck* signal and still has not broken out, so small tweaks are
/// clearly not working. Signal-agnostic and held on clamp. Only `Sequential` (advisory,
/// false-positive-prone) never reaches it.
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
		DetectorSignal::Truncation => &[
			"<pay-attention>\nYour recent tool results were truncated — the output is capped. Re-running the same broad call returns no new content, only more wasted context.\n</pay-attention>",
			"<pay-attention>\nThe output is capped — broadening the call adds nothing. First, what are you trying to find in it? Then narrow smart, not small — fewer, better-targeted calls:\n  • Prefer a specific tool over raw reads: signatures, structural search, semantic search, or grep.\n  • Need several parts? Request them in one parallel batch, not one chunk per turn.\n  • Need one part? Target it with the tool's parameters (line range, limit, offset, filter, query/pattern).\n</pay-attention>",
			"<pay-attention>\nThese broad calls keep truncating and will not return more. Switch now to a specific tool (signatures, structural/semantic search, grep) or target the exact span with parameters (line range, limit, offset, filter). If you cannot, report `blocked`.\n</pay-attention>",
		],
		DetectorSignal::Dedup => &[
			"<pay-attention>\nThese call(s) returned output you already received this session — the body was elided as a duplicate, so you already have it in context.\n</pay-attention>",
			"<pay-attention>\nThese calls keep returning output you already hold — re-fetching adds no new information. Ask yourself what you are still missing, then act on the result already in context, or change the tool or arguments to get something genuinely new.\n</pay-attention>",
			"<pay-attention>\nThis is a loop: the same call(s), the same output you already hold, no new information. Act on what is already in context, or switch to a different tool or arguments that returns something new. If neither moves the task, report `blocked`.\n</pay-attention>",
		],
		DetectorSignal::Distraction => &[
			"<pay-attention>\nYour recent results have drifted off the work you were pursuing — they no longer serve the current goal.\n</pay-attention>",
			"<pay-attention>\nPull back to the goal. In one line: what does the task actually need, and do your recent calls serve it? If not, make your next calls target exactly that — the specific files, symbols, or behavior the goal involves. If you deliberately moved on to a new sub-task, ignore this.\n</pay-attention>",
			"<pay-attention>\nRe-anchor now: state the goal in one line and the single next step it needs, then make your next calls hit exactly that — and nothing unrelated. If you cannot tie the next step to the goal, report `blocked`. If you deliberately moved on to a new sub-task, ignore this.\n</pay-attention>",
		],
		DetectorSignal::Sequential => &[
			"<pay-attention>\nYou have made several single-call turns in a row. For maximum efficiency, when your next operations are independent (none needs another's result), invoke them all in one parallel batch rather than one per turn — e.g. reading 3 files is 3 calls in one batch. It is faster and uses less context.\n</pay-attention>",
			"<pay-attention>\nYou keep issuing one tool call per turn. Name the calls you need next, then send every one that does not depend on a prior result together in a single parallel batch — three independent reads go out as three calls at once. Only chain calls whose arguments genuinely depend on an earlier result.\n</pay-attention>",
			"<pay-attention>\nStill one call per turn — stop serializing independent work. Name your next 2+ calls and send every independent one in a single parallel batch this turn. If each call truly depends on the previous result, serial is correct — keep it.\n</pay-attention>",
		],
		DetectorSignal::None => return "",
	};
	variants[attempt.min(variants.len() - 1)]
}

/// The "stuck" signal class — every real-waste failure mode (loop / no-progress /
/// truncation / dedup / distraction), i.e. everything except the advisory `Sequential`.
/// These escalate to [`PERSISTENT_VARIANTS`]; factored so the steer loop and the
/// escalation ladder classify signals the same way.
pub fn is_stuck(signal: DetectorSignal) -> bool {
	matches!(
		signal,
		DetectorSignal::Loop
			| DetectorSignal::NoProgress
			| DetectorSignal::Truncation
			| DetectorSignal::Dedup
			| DetectorSignal::Distraction
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
			is_truncated: bool,
			is_dedup: bool,
			is_drift: bool,
			loop_threshold: usize,
			no_progress_window: usize,
			truncation_threshold: usize,
			dedup_threshold: usize,
			distraction_threshold: usize,
		) -> DetectorSignal {
			let (rhash, novel) = self.note_call(tool, result, is_error, is_mutation);
			self.record_round_signals(
				&[rhash],
				novel,
				is_truncated,
				is_dedup,
				is_drift,
				loop_threshold,
				no_progress_window,
				truncation_threshold,
				dedup_threshold,
				distraction_threshold,
			)
		}
	}

	#[test]
	fn parallel_batch_counts_as_one_round() {
		// A single parallel round of THREE truncated calls is ONE model decision:
		// it must NOT trip truncation_threshold=3 on its own — the model has not yet
		// seen the truncation notices it is being asked to act on.
		let mut d = Detectors::default();
		let hashes: Vec<u64> = ["chunk A", "chunk B", "chunk C"]
			.iter()
			.map(|r| hash2("view", r))
			.collect();
		let sig = d.record_round_signals(&hashes, true, true, false, false, 9, 9, 3, 0, 0);
		assert_eq!(
			sig,
			DetectorSignal::None,
			"one round counts once, not thrice"
		);
		assert_eq!(d.consecutive_truncations, 1);
		// Two further truncated ROUNDS are what actually reach the streak.
		d.record_round_signals(
			&[hash2("view", "chunk D")],
			true,
			true,
			false,
			false,
			9,
			9,
			3,
			0,
			0,
		);
		let sig = d.record_round_signals(
			&[hash2("view", "chunk E")],
			true,
			true,
			false,
			false,
			9,
			9,
			3,
			0,
			0,
		);
		assert_eq!(
			sig,
			DetectorSignal::Truncation,
			"three truncated ROUNDS trip it"
		);
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
			d.record_action("grep", "same", false, false, false, false, false, 3, 9, 0, 0, 0),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("grep", "same", false, false, false, false, false, 3, 9, 0, 0, 0),
			DetectorSignal::None
		);
		// Third identical RESULT → loop.
		assert_eq!(
			d.record_action("grep", "same", false, false, false, false, false, 3, 9, 0, 0, 0),
			DetectorSignal::Loop
		);
	}

	#[test]
	fn no_progress_fires_on_zero_novelty_window() {
		let mut d = Detectors::default();
		d.record_action("a", "r", false, false, false, false, false, 9, 3, 0, 0, 0); // first "r" → novel
		d.record_action("a", "r", false, false, false, false, false, 9, 3, 0, 0, 0); // seen → not novel
		d.record_action("a", "r", false, false, false, false, false, 9, 3, 0, 0, 0); // not novel
		assert_eq!(
			d.record_action("a", "r", false, false, false, false, false, 9, 3, 0, 0, 0),
			DetectorSignal::NoProgress
		);
	}

	#[test]
	fn mutation_counts_as_progress() {
		let mut d = Detectors::default();
		d.record_action(
			"read", "same", false, false, false, false, false, 9, 2, 0, 0, 0,
		);
		d.record_action(
			"read", "same", false, false, false, false, false, 9, 2, 0, 0, 0,
		);
		// An edit always advances state → breaks the stall.
		assert_eq!(
			d.record_action("edit", "ok", false, true, false, false, false, 9, 2, 0, 0, 0),
			DetectorSignal::None
		);
	}

	#[test]
	fn truncation_fires_on_consecutive_truncated_results() {
		let mut d = Detectors::default();
		// Different content each time (model tweaks args) — defeats Loop/NoProgress,
		// but both carry the truncation flag. Threshold 2 → fires on the second.
		assert_eq!(
			d.record_action("view", "chunk A", false, false, true, false, false, 9, 9, 2, 0, 0),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("view", "chunk B", false, false, true, false, false, 9, 9, 2, 0, 0),
			DetectorSignal::Truncation
		);
	}

	#[test]
	fn clean_result_resets_truncation_streak() {
		let mut d = Detectors::default();
		d.record_action(
			"view", "chunk A", false, false, true, false, false, 9, 9, 2, 0, 0,
		);
		// Model narrowed and got a full result → streak resets.
		assert_eq!(
			d.record_action("view", "full", false, false, false, false, false, 9, 9, 2, 0, 0),
			DetectorSignal::None
		);
		// One truncation again is not yet at threshold.
		assert_eq!(
			d.record_action("view", "chunk B", false, false, true, false, false, 9, 9, 2, 0, 0),
			DetectorSignal::None
		);
	}

	#[test]
	fn truncation_outranks_loop() {
		let mut d = Detectors::default();
		// Identical truncated result repeated: both Loop and Truncation conditions
		// hold; the more actionable Truncation signal wins.
		d.record_action(
			"view", "same", false, false, true, false, false, 2, 9, 2, 0, 0,
		);
		assert_eq!(
			d.record_action("view", "same", false, false, true, false, false, 2, 9, 2, 0, 0),
			DetectorSignal::Truncation
		);
	}

	#[test]
	fn verification_shape_fallback_without_fingerprints() {
		let mut d = Detectors::default();
		assert!(!d.needs_verification(None));
		// Mutation-shaped round, no verifier → unverified.
		d.note_round_verification(None, None, false, false, true);
		assert!(d.needs_verification(None));
		// A read-only round changes nothing — looking is not verifying.
		d.note_round_verification(None, None, false, false, false);
		assert!(d.needs_verification(None));
		// A round where the verifier ran alongside a mutation proves nothing.
		d.note_round_verification(None, None, true, false, true);
		assert!(d.needs_verification(None));
		// A clean verifier round clears it.
		d.note_round_verification(None, None, true, false, false);
		assert!(!d.needs_verification(None));
	}

	#[test]
	fn verification_tracks_tree_fingerprint() {
		let mut d = Detectors::default();
		// Round 1 seeds the baseline (10 = task-start tree); the round's edit
		// moved the tree to 11 → unverified.
		d.note_round_verification(Some(10), Some(11), false, false, true);
		assert!(d.needs_verification(Some(11)));
		// Verifier ran but the same round dirtied the tree (11→12): ambiguous
		// state, proves nothing.
		d.note_round_verification(Some(11), Some(12), true, false, true);
		assert!(d.needs_verification(Some(12)));
		// Clean verifier on an unchanged tree → verified at 12.
		d.note_round_verification(Some(12), Some(12), true, false, false);
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
		d.note_round_verification(Some(10), Some(10), false, false, false);
		assert!(!d.needs_verification(Some(11)));
		// A round that itself moved the tree arms it, even when no call was
		// mutation-shaped (an edit hidden inside a shell command).
		d.note_round_verification(Some(11), Some(12), false, false, false);
		assert!(d.needs_verification(Some(12)));
	}

	#[test]
	fn readback_of_mutated_path_verifies_artifact_work() {
		use serde_json::json;
		let mut d = Detectors::default();
		// Round 1: agent edits a doc — mutation round, tree moves, dirty.
		d.note_mutated_paths(&json!({"path": "blog/post/index.md"}));
		d.note_round_verification(Some(10), Some(11), false, false, true);
		assert!(d.needs_verification(Some(11)));
		// Round 2: agent re-reads the exact artifact it changed — that IS the
		// verification for work with no command to run.
		let readback = d.is_readback_call(
			&json!({"path": "blog/post/index.md", "start": 85}),
			false,
			false,
		);
		assert!(readback);
		d.note_round_verification(Some(11), Some(11), false, readback, false);
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
		assert!(is_verifier_shaped(
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
	fn reset_streak_clears_previous_task_verification_latch() {
		let mut d = Detectors::default();
		d.note_round_verification(None, None, false, false, true);
		assert!(d.needs_verification(None));
		// A new genuine task must not inherit an earlier task's mutation.
		d.reset_streak();
		assert!(!d.needs_verification(None));
	}

	#[test]
	fn forbids_verification_is_a_standing_rule() {
		let mut d = Detectors::default();
		assert!(!d.check_run_forbidden());
		d.note_forbids_verification();
		// Standing rule: a new genuine turn must not re-arm the pre-gate.
		d.reset_streak();
		assert!(d.check_run_forbidden());
		// An incidental clean verification round does NOT erase the user's
		// prohibition — the latch never auto-expires.
		d.note_round_verification(None, None, true, false, false);
		assert!(d.check_run_forbidden());
	}

	#[test]
	fn evidence_grounded_quote_passes() {
		let outputs = vec!["274:\t\tif (!in_array($deal_data['status'], ...))".to_string()];
		// Quote is a contiguous (whitespace-normalized) substring of the output.
		let resp = "The guard is here <evidence locator=\"PayoutTaskService.php:274\">if (!in_array($deal_data['status'], ...))</evidence>.";
		assert!(unverified_citations(resp, &outputs).is_empty());
	}

	#[test]
	fn evidence_fabricated_quote_flagged() {
		let outputs = vec!["fn record_action(&mut self) -> DetectorSignal".to_string()];
		let resp =
			"It does <evidence locator=\"detect.rs\">fn totally_made_up_symbol()</evidence>.";
		let bad = unverified_citations(resp, &outputs);
		assert_eq!(bad, vec!["fn totally_made_up_symbol()"]);
	}

	#[test]
	fn evidence_multiline_quote_spanning_numbered_view_lines_passes() {
		// Regression: `view` prefixes every line with `NN:`, so a joined
		// multi-line quote can never string-match across the injected numbers.
		// Line-wise matching must accept it — this was the false positive that
		// re-ran grounded answers.
		let outputs = vec![
			"36:\t\t'week' => $midnight - ((int)gmdate('N', $now) - 1) * 86400,\n37:\t\t'month' => (int)gmmktime(0, 0, 0),"
				.to_string(),
		];
		let resp = "Both anchors <evidence locator=\"Meter.php:36-37\">'week' => $midnight - ((int)gmdate('N', $now) - 1) * 86400,\n'month' => (int)gmmktime(0, 0, 0),</evidence>.";
		assert!(unverified_citations(resp, &outputs).is_empty());
	}

	#[test]
	fn evidence_multiline_quote_with_one_fabricated_line_flagged() {
		let outputs = vec!["36:\t\t'week' => $midnight,".to_string()];
		let resp = "<evidence locator=\"Meter.php:36-37\">'week' => $midnight,\n'month' => invented()</evidence>";
		let bad = unverified_citations(resp, &outputs);
		assert_eq!(bad, vec!["'month' => invented()"]);
	}

	#[test]
	fn evidence_self_closing_and_unclosed_tags_ignored() {
		let outputs = vec!["x".to_string()];
		assert!(unverified_citations("a <evidence/> b", &outputs).is_empty());
		assert!(unverified_citations("a <evidence locator=\"f\" b", &outputs).is_empty());
	}

	#[test]
	fn evidence_examples_in_markdown_code_are_not_citations() {
		let inline =
			"Use `<evidence locator=\"source:line\">verbatim text</evidence>` for citations.";
		let fenced =
			"Example:\n```xml\n<evidence locator=\"source:line\">verbatim text</evidence>\n```";
		assert!(unverified_citations(inline, &[]).is_empty());
		assert!(unverified_citations(fenced, &[]).is_empty());
		assert_eq!(strip_evidence(inline), inline);
		assert_eq!(strip_evidence(fenced), fenced);
	}

	#[test]
	fn escaped_and_longer_evidence_markup_is_literal() {
		let escaped = r#"Show \<evidence locator="source:line">text</evidence> literally."#;
		let longer = "The <evidence-note>text</evidence> element is unrelated.";
		assert!(unverified_citations(escaped, &[]).is_empty());
		assert!(unverified_citations(longer, &[]).is_empty());
		assert_eq!(strip_evidence(escaped), escaped);
		assert_eq!(strip_evidence(longer), longer);
	}

	#[test]
	fn real_evidence_next_to_literal_example_is_still_checked_and_stripped() {
		let response = "Syntax: `<evidence locator=\"x\">quote</evidence>`. Claim: <evidence locator=\"real\">actual quote</evidence>.";
		assert_eq!(
			unverified_citations(response, &[]),
			["actual quote".to_string()]
		);
		assert_eq!(
			strip_evidence(response),
			"Syntax: `<evidence locator=\"x\">quote</evidence>`. Claim: ."
		);
	}

	#[test]
	fn strip_evidence_removes_tags_keeps_prose() {
		let resp = "The math is correct. <evidence locator=\"Meter.php:36\">'week' => $midnight,</evidence> Done.";
		assert_eq!(strip_evidence(resp), "The math is correct.  Done.");
	}

	#[test]
	fn strip_evidence_keeps_unclosed_tag() {
		let resp = "text <evidence locator=\"f\">dangling quote";
		assert_eq!(strip_evidence(resp), resp);
	}

	#[test]
	fn strip_evidence_handles_multiple_and_none() {
		assert_eq!(
			strip_evidence("a <evidence>q1</evidence> b <evidence>x</evidence> c"),
			"a  b  c"
		);
		assert_eq!(strip_evidence("plain"), "plain");
	}

	#[test]
	fn url_fetched_this_turn_passes() {
		let grounds = vec!["See https://example.com/study for details.".to_string()];
		let resp = "According to https://example.com/study, the effect holds.";
		assert!(unverified_urls(resp, &grounds).is_empty());
	}

	#[test]
	fn url_from_user_message_passes() {
		let grounds = vec!["user: check https://example.com/spec please".to_string()];
		let resp = "The spec at https://example.com/spec says so.";
		assert!(unverified_urls(resp, &grounds).is_empty());
	}

	#[test]
	fn url_never_received_flagged() {
		let grounds = vec!["some unrelated tool output".to_string()];
		let resp = "Source: https://invented.example/paper.pdf — see also https://invented.example/paper.pdf.";
		let bad = unverified_urls(resp, &grounds);
		assert_eq!(bad, vec!["https://invented.example/paper.pdf"]);
	}

	#[test]
	fn url_trailing_punctuation_and_slash_tolerated() {
		let grounds = vec!["fetched https://example.com/".to_string()];
		let resp = "From (https://example.com), we learn.";
		assert!(unverified_urls(resp, &grounds).is_empty());
	}

	#[test]
	fn url_fragment_grounded_by_bare_url() {
		// A `#fragment` is client-side navigation — citing `url#L10` when the
		// tool output carried the bare `url` is grounded, not invented.
		let grounds = vec!["fetched https://example.com/repo/file.rs".to_string()];
		let resp = "See https://example.com/repo/file.rs#L10 for the guard.";
		assert!(unverified_urls(resp, &grounds).is_empty());
		// Trailing-slash ground also accepted for the fragment-stripped form.
		let grounds = vec!["fetched https://example.com/docs/".to_string()];
		let resp = "See https://example.com/docs#install.";
		assert!(unverified_urls(resp, &grounds).is_empty());
	}

	#[test]
	fn evidence_tolerates_reflowed_whitespace() {
		let outputs = vec!["let signal = detectors.record_action(tool, result);".to_string()];
		// Model reflowed the quote with extra indentation — normalization
		// makes the single logical line match.
		let resp = "see <evidence locator=\"x\">let signal = detectors.record_action(tool, result);</evidence>";
		assert!(unverified_citations(resp, &outputs).is_empty());
	}

	#[test]
	fn no_citations_is_clean() {
		assert!(unverified_citations("plain answer, no tags", &["x".to_string()]).is_empty());
	}

	#[test]
	fn file_refs_existing_line_is_clean() {
		// cargo test runs with the crate root as cwd, so this very file resolves.
		assert!(unverified_file_refs("see src/supervisor/detect.rs:1 for it", &[]).is_empty());
	}

	#[test]
	fn file_refs_missing_file_flagged() {
		let bad = unverified_file_refs("fixed in src/supervisor/zz_no_such_file.rs:5 now", &[]);
		assert_eq!(
			bad,
			vec!["src/supervisor/zz_no_such_file.rs:5 (file not found)"]
		);
	}

	#[test]
	fn file_refs_line_beyond_eof_flagged() {
		let bad = unverified_file_refs("look at src/supervisor/mod.rs:999999", &[]);
		assert_eq!(bad.len(), 1);
		assert!(bad[0].starts_with("src/supervisor/mod.rs:999999 (file has only "));
	}

	#[test]
	fn file_refs_urls_and_versions_not_matched() {
		assert!(
			unverified_file_refs("see https://x.com/a/b.rs:12 and v1.2/3.4:56", &[]).is_empty()
		);
	}

	#[test]
	fn file_refs_deduplicated() {
		let bad = unverified_file_refs("a/missing.rs:1 and again a/missing.rs:1 twice", &[]);
		assert_eq!(bad.len(), 1);
	}

	#[test]
	fn file_refs_github_anchor_matched() {
		assert!(unverified_file_refs("see src/supervisor/detect.rs#L1 for it", &[]).is_empty());
		let bad = unverified_file_refs("see src/supervisor/zz_no_such_file.rs#L5 now", &[]);
		assert_eq!(
			bad,
			vec!["src/supervisor/zz_no_such_file.rs:5 (file not found)"]
		);
	}

	#[test]
	fn file_refs_suffix_shorthand_resolves_against_cited_full_path() {
		// Prose abbreviation of a path cited in full elsewhere is not fabrication.
		assert!(unverified_file_refs(
			"comment at supervisor/detect.rs:1 (full: src/supervisor/detect.rs:1)",
			&[]
		)
		.is_empty());
		// Also when the full path was cited GitHub-style.
		assert!(unverified_file_refs(
			"comment at supervisor/detect.rs:1 (full: src/supervisor/detect.rs#L1-L20)",
			&[]
		)
		.is_empty());
	}

	#[test]
	fn file_refs_suffix_shorthand_still_bound_checked() {
		let bad = unverified_file_refs(
			"comment at supervisor/mod.rs:999999 (full: src/supervisor/mod.rs:1)",
			&[],
		);
		assert_eq!(bad.len(), 1);
		assert!(bad[0].starts_with("supervisor/mod.rs:999999 (file has only "));
	}

	#[test]
	fn file_refs_resolve_against_grounds_from_other_roots() {
		// A ref that resolves at no cwd path but whose full path appears in the
		// turn's executed call args (another repo root) is real — recovered by
		// backwards expansion and still line-bound-checked against that root.
		let grounds = vec![format!(
			r#"{{"path":"{}/src/supervisor/detect.rs","start":1}}"#,
			env!("CARGO_MANIFEST_DIR")
		)];
		assert!(unverified_file_refs("see supervisor/detect.rs:1", &grounds).is_empty());
		let bad = unverified_file_refs("see supervisor/detect.rs:999999", &grounds);
		assert_eq!(bad.len(), 1);
		assert!(bad[0].starts_with("supervisor/detect.rs:999999 (file has only "));
	}

	#[test]
	fn file_refs_received_in_grounds_get_benefit_of_doubt() {
		// A path echoed by a real tool output (e.g. `wc -l` run in another cwd)
		// was received, not fabricated — even when nothing resolves on disk.
		let grounds = vec!["     164 zz/qq/other_root.ts".to_string()];
		assert!(unverified_file_refs("see zz/qq/other_root.ts:86", &grounds).is_empty());
		// Absent from grounds → still flagged.
		assert_eq!(
			unverified_file_refs("see zz/qq/other_root.ts:86", &[]),
			vec!["zz/qq/other_root.ts:86 (file not found)"]
		);
	}

	#[test]
	fn file_refs_grounds_boundaries_respected() {
		// A prefix of a longer filename or the interior of a different path is
		// not receipt of this path.
		let grounds = vec!["zz/qq/other_root.tsx and xzz/qq/other_root.ts".to_string()];
		assert_eq!(
			unverified_file_refs("see zz/qq/other_root.ts:1", &grounds),
			vec!["zz/qq/other_root.ts:1 (file not found)"]
		);
	}

	#[test]
	fn truncation_steers_even_while_exploring() {
		assert!(should_steer(
			DetectorSignal::Truncation,
			Some(SelfReport::Exploring)
		));
		// But still defers to the gate on done.
		assert!(!should_steer(
			DetectorSignal::Truncation,
			Some(SelfReport::Done)
		));
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
	fn dedup_fires_on_consecutive_dedup_results() {
		let mut d = Detectors::default();
		// is_dedup=true, threshold 2 → fires on the second in a row.
		assert_eq!(
			d.record_action("view", "[dup A]", true, false, false, true, false, 9, 9, 0, 2, 0),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("view", "[dup B]", true, false, false, true, false, 9, 9, 0, 2, 0),
			DetectorSignal::Dedup
		);
	}

	#[test]
	fn clean_result_resets_dedup_streak() {
		let mut d = Detectors::default();
		d.record_action(
			"view", "[dup A]", true, false, false, true, false, 9, 9, 0, 2, 0,
		);
		// A fresh, non-dedup result breaks the streak.
		assert_eq!(
			d.record_action("view", "full", false, false, false, false, false, 9, 9, 0, 2, 0),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("view", "[dup B]", true, false, false, true, false, 9, 9, 0, 2, 0),
			DetectorSignal::None
		);
	}

	#[test]
	fn dedup_outranks_loop() {
		let mut d = Detectors::default();
		// Identical dedup placeholder repeated satisfies both Loop and Dedup;
		// the more precise Dedup signal wins.
		d.record_action(
			"view", "[dup]", true, false, false, true, false, 2, 9, 0, 2, 0,
		);
		assert_eq!(
			d.record_action("view", "[dup]", true, false, false, true, false, 2, 9, 0, 2, 0),
			DetectorSignal::Dedup
		);
	}

	#[test]
	fn dedup_steers_even_while_exploring() {
		assert!(should_steer(
			DetectorSignal::Dedup,
			Some(SelfReport::Exploring)
		));
		assert!(!should_steer(DetectorSignal::Dedup, Some(SelfReport::Done)));
	}

	#[test]
	fn distraction_fires_on_consecutive_drift() {
		let mut d = Detectors::default();
		// is_drift=true, threshold 2 → fires on the second in a row.
		assert_eq!(
			d.record_action("view", "off", false, false, false, false, true, 9, 9, 0, 0, 2),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("view", "off2", false, false, false, false, true, 9, 9, 0, 0, 2),
			DetectorSignal::Distraction
		);
	}

	#[test]
	fn on_task_result_resets_distraction_streak() {
		let mut d = Detectors::default();
		d.record_action(
			"view", "off", false, false, false, false, true, 9, 9, 0, 0, 2,
		);
		// An on-task call breaks the streak.
		assert_eq!(
			d.record_action("view", "rel", false, false, false, false, false, 9, 9, 0, 0, 2),
			DetectorSignal::None
		);
		assert_eq!(
			d.record_action("view", "off2", false, false, false, false, true, 9, 9, 0, 0, 2),
			DetectorSignal::None
		);
	}

	#[test]
	fn distraction_waits_while_exploring_but_fires_when_progressing() {
		assert!(!should_steer(
			DetectorSignal::Distraction,
			Some(SelfReport::Exploring)
		));
		assert!(should_steer(
			DetectorSignal::Distraction,
			Some(SelfReport::Progressing)
		));
	}

	#[test]
	fn note_result_seeds_then_flags_drift() {
		let mut d = Detectors::default();
		let a = vec![1.0, 0.0, 0.0];
		let b = vec![0.9, 0.1, 0.0];
		// Warmup seeds the centroid with similar results — never drift.
		assert!(!d.note_result(&a, 0.5));
		assert!(!d.note_result(&b, 0.5));
		assert!(!d.note_result(&b, 0.5));
		// A call orthogonal to the working set is drift; an aligned one is not.
		let off = vec![0.0, 0.0, 1.0];
		assert!(d.note_result(&off, 0.5));
		assert!(!d.note_result(&a, 0.5));
	}

	#[test]
	fn drift_calls_do_not_pull_the_centroid() {
		let mut d = Detectors::default();
		let on = vec![1.0, 0.0, 0.0];
		for _ in 0..3 {
			d.note_result(&on, 0.5);
		}
		// Repeated drift is never folded in, so it keeps reading as drift.
		let off = vec![0.0, 0.0, 1.0];
		assert!(d.note_result(&off, 0.5));
		assert!(d.note_result(&off, 0.5));
		// And an on-task call is still recognised.
		assert!(!d.note_result(&on, 0.5));
	}

	#[test]
	fn note_task_change_resets_working_set() {
		let mut d = Detectors::default();
		let on = vec![1.0, 0.0, 0.0];
		for _ in 0..3 {
			d.note_result(&on, 0.5);
		}
		// New task → working set cleared → next call re-seeds (never drift).
		d.note_task(42);
		let off = vec![0.0, 0.0, 1.0];
		assert!(!d.note_result(&off, 0.5));
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
	fn sequential_steers_even_while_exploring() {
		// Serializing independent calls is never "legitimate exploring" — the
		// detector already gates on N consecutive single-call rounds, so the
		// exploring excuse must not suppress the Sequential steer.
		assert!(should_steer(
			DetectorSignal::Sequential,
			Some(SelfReport::Exploring)
		));
		assert!(should_steer(
			DetectorSignal::Sequential,
			Some(SelfReport::Progressing)
		));
		// Still defers to the gate on done.
		assert!(!should_steer(
			DetectorSignal::Sequential,
			Some(SelfReport::Done)
		));
	}

	#[test]
	fn sequential_streak_resets_after_steer() {
		let mut d = Detectors::default();
		// threshold 2: two single-call rounds in a row → Sequential.
		assert_eq!(d.record_round_arity(1, 2), DetectorSignal::None);
		assert_eq!(d.record_round_arity(1, 2), DetectorSignal::Sequential);
		// Reset on steer → it must re-accumulate, so the very next single-call round
		// is silent instead of nudging again every turn (the spam being fixed).
		d.reset_sequential_streak();
		assert_eq!(d.record_round_arity(1, 2), DetectorSignal::None);
		assert_eq!(d.record_round_arity(1, 2), DetectorSignal::Sequential);
	}

	#[test]
	fn sequential_streak_ignores_error_recovery_rounds() {
		let mut d = Detectors::default();
		// One legit singleton accumulates.
		assert_eq!(d.record_round_arity(1, 2), DetectorSignal::None);
		// An errored singleton round (e.g. git_diff ref not resolving) must not
		// grow the streak — it is a failure, not a batching choice.
		d.note_call("git_diff", "ref did not resolve", true, false);
		assert_eq!(d.record_round_arity(1, 2), DetectorSignal::None);
		// The singleton RETRY right after the errored round is recovery, not a
		// drip-feed — still no growth, and the earlier streak is preserved.
		d.note_call("shell", "65ab1db…", false, false);
		assert_eq!(d.record_round_arity(1, 2), DetectorSignal::None);
		// Recovery over: the next ordinary singleton resumes the streak and fires.
		assert_eq!(d.record_round_arity(1, 2), DetectorSignal::Sequential);
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
		// Advisory Sequential never escalates to the persistent frame.
		assert!(!steer_note(DetectorSignal::Sequential, None, 5).contains("blocked"));
	}
}
