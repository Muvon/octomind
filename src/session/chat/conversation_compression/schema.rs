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

//! Schema + typed view of the compression response.
//!
//! The compression LLM call is invoked with a strict JSON schema (octolib
//! `StructuredOutputRequest::json_schema(..).with_strict_mode()`). The model
//! returns one well-typed object; we deserialize it into `CompressionSummary`
//! and render it deterministically to markdown for insertion into the session
//! and re-feed into the next compression cycle.
//!
//! Rationale (vs. free-form markdown):
//!   - Zero format drift: schema validation guarantees every required field
//!     is present and correctly typed.
//!   - YES/NO gate becomes a `should_compress: bool` field — no first-line
//!     parsing, no "AI said YES but summary is empty" failure mode.
//!   - `file_context` line numbers validated at schema level (1..=10000).
//!   - System prompt shrinks ~65% (no more long format spec embedded in it),
//!     which is cached and amortised across every compression call.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Maximum allowed value for `start_line` / `end_line` in `file_context`.
/// Mirrors the JSON schema bound so JSON and XML paths validate identically.
const FILE_CONTEXT_LINE_MAX: usize = 10_000;

/// Typed deserialization target for the model's structured response.
///
/// `#[serde(default)]` on every field is defensive — the schema is strict, so
/// in practice every field is always present, but we never want a stray
/// deserialization error to abort compression. A near-empty summary is caught
/// downstream by the substantive-length check in `is_summary_substantive`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CompressionSummary {
	pub should_compress: bool,
	pub original_request: String,
	pub session_context: String,
	pub current_task: String,
	pub progress: String,
	pub analysis_findings: Vec<String>,
	pub errors_and_corrections: Vec<String>,
	pub recent_exchanges: Vec<String>,
	pub key_entities: KeyEntities,
	pub next_steps: String,
	pub file_context: Vec<FileContextEntry>,
	pub critical_knowledge: Vec<String>,
	pub open_loops: Vec<String>,
	pub file_states: Vec<String>,
	/// Atomic completed-state claims with platform-verifiable source IDs.
	/// Empty only on the legacy compression path or when there is no completed
	/// evidence to fold.
	pub folded_units: Vec<FoldedUnit>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct FoldedUnit {
	pub text: String,
	pub kind: String,
	pub status: String,
	pub refs: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct KeyEntities {
	pub files: Vec<String>,
	pub names: Vec<String>,
	pub decisions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileContextEntry {
	pub filepath: String,
	pub start_line: usize,
	pub end_line: usize,
}

/// Heuristic substantive-content check.
///
/// Replaces the old `MIN_SUMMARY_LEN` byte-count gate. With schema validation
/// the *shape* is guaranteed; what we still need to defend against is the
/// model emitting `should_compress: true` with all string fields empty — that
/// would wipe the session with a header-only summary. Require at least one
/// of the core narrative sections to carry signal.
pub fn is_summary_substantive(summary: &CompressionSummary) -> bool {
	!summary.current_task.trim().is_empty()
		|| !summary.progress.trim().is_empty()
		|| !summary.session_context.trim().is_empty()
		|| !summary.analysis_findings.is_empty()
		|| !summary.recent_exchanges.is_empty()
		|| !summary.folded_units.is_empty()
}

/// Render a structured summary to the XML body that will be inserted into
/// the session as the compressed assistant turn and re-fed into the next
/// compression cycle as transcript input.
///
/// XML over markdown: Claude is tuned to attend to XML-delimited sections
/// (`<finding>…</finding>` is parsed more reliably than `- finding`), and
/// `**HEADER**:` text is just bytes to the model whereas `<header>…</header>`
/// is a structural boundary. The structured tags survive paraphrase decay
/// across compressions; markdown headers don't.
///
/// Sections appear only when they carry signal — the body stays terse on
/// early or sparse compressions. Order matches priority (original request
/// first → next steps last).
pub fn render_summary(summary: &CompressionSummary) -> String {
	let mut out = String::new();

	let push_text = |out: &mut String, tag: &str, value: &str| {
		if !value.trim().is_empty() {
			out.push_str(&format!("<{tag}>{}</{tag}>\n", value.trim(), tag = tag));
		}
	};

	let push_list = |out: &mut String, outer: &str, item: &str, values: &[String]| {
		let non_empty: Vec<&String> = values.iter().filter(|s| !s.trim().is_empty()).collect();
		if non_empty.is_empty() {
			return;
		}
		out.push_str(&format!("<{outer}>\n"));
		for v in non_empty {
			out.push_str(&format!("<{item}>{}</{item}>\n", v.trim(), item = item));
		}
		out.push_str(&format!("</{outer}>\n"));
	};

	push_text(&mut out, "original_request", &summary.original_request);
	push_text(&mut out, "session_context", &summary.session_context);
	push_text(&mut out, "current_task", &summary.current_task);
	if !summary.folded_units.is_empty() {
		out.push_str(&render_folded_state(&summary.folded_units));
		out.push('\n');
	}
	push_text(&mut out, "progress", &summary.progress);
	push_list(
		&mut out,
		"analysis_findings",
		"finding",
		&summary.analysis_findings,
	);
	push_list(
		&mut out,
		"errors_and_corrections",
		"entry",
		&summary.errors_and_corrections,
	);
	push_list(
		&mut out,
		"recent_exchanges",
		"exchange",
		&summary.recent_exchanges,
	);

	let ke = &summary.key_entities;
	if !ke.files.is_empty() || !ke.names.is_empty() || !ke.decisions.is_empty() {
		out.push_str("<key_entities>\n");
		push_list(&mut out, "files", "file", &ke.files);
		push_list(&mut out, "names", "name", &ke.names);
		push_list(&mut out, "decisions", "decision", &ke.decisions);
		out.push_str("</key_entities>\n");
	}

	push_list(&mut out, "open_loops", "open_loop", &summary.open_loops);
	push_list(&mut out, "file_states", "state", &summary.file_states);

	push_text(&mut out, "next_steps", &summary.next_steps);

	out.trim_end().to_string()
}

/// PACT live state admits only source-attributed model-authored claims. The
/// runtime supplies task/governance, exact frontier, and recall separately, so
/// re-rendering legacy narrative fields here would create a second,
/// unvalidated authority channel that can contradict those bands.
pub fn render_pact_summary(summary: &CompressionSummary) -> String {
	render_folded_state(&summary.folded_units)
}

fn render_folded_state(units: &[FoldedUnit]) -> String {
	let mut out = String::from("<folded_state>\n");
	for unit in units {
		let refs = unit.refs.join(" ");
		let id = super::attention::folded_unit_id(unit);
		let text = if unit.status == "superseded" {
			"Superseded state is archived and must not be treated as current."
		} else {
			unit.text.trim()
		};
		out.push_str(&format!(
			"<unit id=\"{}\" kind=\"{}\" status=\"{}\" refs=\"{}\">{}</unit>\n",
			xml_escape(&id),
			xml_escape(&unit.kind),
			xml_escape(&unit.status),
			xml_escape(&refs),
			xml_escape(text)
		));
	}
	out.push_str("</folded_state>");
	out
}

fn xml_escape(value: &str) -> String {
	value
		.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
		.replace('\'', "&apos;")
}

/// Build the JSON Schema sent to the provider via `with_schema(..)`.
///
/// `force=true`: model has no veto. `should_compress` MUST be `true`; the
/// schema description nails it down so the model doesn't return `false` and
/// stall a forced compression.
///
/// `force=false`: model may return `should_compress: false` when the
/// transcript is already minimal. Other fields are still required by the
/// schema (strict mode); they're expected to be empty strings / empty arrays
/// when `should_compress` is false.
pub fn build_compression_schema(force: bool, pact: bool) -> serde_json::Value {
	let should_compress_desc = if force {
		"Compression has been forced by the user. MUST be true."
	} else {
		"True if the transcript contains older exchanges that can be safely compressed without losing information needed to continue. WHEN a fold happens matters as much as what it keeps: folding while a step is half-finished blunts the newest exchange and leaves the agent unable to tell which actions it has already taken, so it repeats them. Answer true at a natural seam — a sub-task just resolved, a check passed, or the work is converging on its answer. Answer false when the agent is mid-derivation (a build or test is in flight, an edit is started but unverified, a hypothesis is being chased) or is stuck and re-reading to recover its footing, and false when the transcript is already minimal. Deferring is a short reprieve, not a veto: when the context nears its limit this decision is forced and the fold happens regardless, so defer only for a genuinely unfinished step, never as a general preference."
	};

	let mut schema = serde_json::json!({
		"type": "object",
		"additionalProperties": false,
		"properties": {
			"should_compress": {
				"type": "boolean",
				"description": should_compress_desc
			},
			"original_request": {
				"type": "string",
				"description": "The ACTIVE task: quote verbatim from the MOST RECENT real user turn in the transcript. This field is what the next model turn treats as its instruction, so a stale value makes it abandon the live task and redo old work. Only when the transcript contains no real user turn at all, carry forward the prior summary's request unchanged. Never paraphrase, and never prefer an earlier turn over a later one."
			},
			"session_context": {
				"type": "string",
				"description": "One sentence describing what brought the session to this point."
			},
			"current_task": {
				"type": "string",
				"description": "1–2 sentences: the user's most recent active request. If the user pivoted, the new topic IS the current task."
			},
			"progress": {
				"type": "string",
				"description": "Lead with a DONE list — one line per action already carried out and confirmed (file written, command run and its result, check that passed), each phrased so it is unambiguous the work exists and must NOT be repeated. Then 1-2 sentences on what is in progress and the outcome so far. This list is the only record the next turn has of what it already did: anything omitted will be done a second time, and work still described as pending will be resumed even when it is finished. If a prior summary exists in the transcript, carry its DONE list forward and add to it. Keep at most 20 lines: when the list would exceed that, merge the oldest entries into a single summarising line rather than dropping them, so the record stays bounded while nothing already done silently disappears."
			},
			"analysis_findings": {
				"type": "array",
				"items": { "type": "string" },
				"maxItems": 30,
				"description": "3–6 bullets: conclusions THIS transcript established that a prior summary did not — root causes, behaviours, code-location-specific discoveries. Record NEGATIVE conclusions too: a hypothesis investigated and excluded, with the reason it was excluded ('X is not the cause / is out of scope because Y'). Do NOT restate findings from a prior summary: they are retained outside your output and re-attached automatically, so repeating them in new words creates duplicates rather than preserving them. An empty array is correct when this transcript established nothing new."
			},
			"errors_and_corrections": {
				"type": "array",
				"items": { "type": "string" },
				"maxItems": 10,
				"description": "Highest-priority preservation. Verbatim user negative feedback ('don't do X', 'stop doing Y'), error strings encountered, and failed approaches with why they failed. Carry forward across compressions."
			},
			"recent_exchanges": {
				"type": "array",
				"items": { "type": "string" },
				"maxItems": 10,
				"description": "Faithful paraphrases covering the [RECENT]-tagged span. Combine causally related turns when needed to fit the array while keeping concrete details and decisions intact."
			},
			"key_entities": {
				"type": "object",
				"additionalProperties": false,
				"properties": {
					"files": {
						"type": "array",
						"items": { "type": "string" },
						"description": "Exact file paths with line numbers, verbatim."
					},
					"names": {
						"type": "array",
						"items": { "type": "string" },
						"description": "Identifiers, function names, variable names, config keys, verbatim."
					},
					"decisions": {
						"type": "array",
						"items": { "type": "string" },
						"description": "Choices made with their reasoning."
					}
				},
				"required": ["files", "names", "decisions"]
			},
			"next_steps": {
				"type": "string",
				"description": "1–2 sentences: the concrete action that advances the current task next."
			},
			"file_context": {
				"type": "array",
				"maxItems": 5,
				"items": {
					"type": "object",
					"additionalProperties": false,
					"properties": {
						"filepath": {
							"type": "string",
							"description": "Path from project root."
						},
						"start_line": {
							"type": "integer",
							"minimum": 1,
							"maximum": 10000
						},
						"end_line": {
							"type": "integer",
							"minimum": 1,
							"maximum": 10000
						}
					},
					"required": ["filepath", "start_line", "end_line"]
				},
				"description": "Up to 5 file ranges the next turn will need. Auto-loaded from disk and re-injected after the summary. Prioritise files being actively edited or analysed."
			},
			"critical_knowledge": {
				"type": "array",
				"items": { "type": "string" },
				"maxItems": 15,
				"description": "Survives ALL future compressions. Durable execution protocol (procedure, resources, cadence, constraints, checkpoints, completion condition), architectural decisions, hidden constraints, user preferences, and root-cause findings. Generic across task domains, not only programming. 2–3 sentences each. Include only when future turns must retain it — not routine progress or transient tool payloads."
			},
			"open_loops": {
				"type": "array",
				"items": { "type": "string" },
				"maxItems": 8,
				"description": "Unresolved questions, pending user decisions, blocked items, open TODOs — anything still waiting on an answer or action. Carry forward across compressions until resolved."
			},
			"file_states": {
				"type": "array",
				"items": { "type": "string" },
				"maxItems": 10,
				"description": "Files created or edited with their last-known state: 'path — what changed / current status'. Prevents re-doing completed edits after compression."
			},
			"folded_units": {
				"type": "array",
				"maxItems": 40,
				"items": {
					"type": "object",
					"additionalProperties": false,
					"properties": {
						"text": { "type": "string", "maxLength": 2000 },
						"kind": {
							"type": "string",
							"enum": ["observation", "decision", "action", "outcome", "correction", "open_loop", "next_action", "reference", "synthesis"]
						},
						"status": {
							"type": "string",
							"enum": ["established", "tentative", "superseded", "failed", "pending", "unknown"]
						},
						"refs": {
							"type": "array",
							"items": { "type": "string" },
							"minItems": 1,
							"maxItems": 16
						}
					},
					"required": ["text", "kind", "status", "refs"]
				},
				"description": "PACT only: atomic completed-state claims. Every claim cites all and only supplied evidence block IDs. Hard rules: never cite archive_reference packet IDs (recall pointers, not evidence); a unit citing any keep_exact packet may only use status pending/tentative/unknown; every summarize-lane packet ID must be cited by at least one unit. Include every consequential completed outcome, correction, durable protocol, open loop, and next action not already exact in pinned/frontier state; legacy narrative fields are not rendered as PACT authority. Return [] only when no such state exists."
			}
		},
		"required": [
			"should_compress",
			"original_request",
			"session_context",
			"current_task",
			"progress",
			"analysis_findings",
			"errors_and_corrections",
			"recent_exchanges",
			"key_entities",
			"next_steps",
			"file_context",
			"critical_knowledge",
			"open_loops",
			"file_states",
			"folded_units"
		]
	});
	if pact {
		let properties = schema
			.get_mut("properties")
			.and_then(serde_json::Value::as_object_mut)
			.expect("compression schema properties are an object");
		for field in [
			"original_request",
			"session_context",
			"current_task",
			"progress",
			"next_steps",
		] {
			let property = properties[field]
				.as_object_mut()
				.expect("PACT legacy string field is an object");
			property.insert("maxLength".into(), 0.into());
			property.insert(
				"description".into(),
				"PACT wire-compatibility field. Return an empty string; runtime-owned bands and attributed folds carry live state."
					.into(),
			);
		}
		for field in [
			"analysis_findings",
			"errors_and_corrections",
			"recent_exchanges",
			"critical_knowledge",
			"open_loops",
			"file_states",
		] {
			let property = properties[field]
				.as_object_mut()
				.expect("PACT legacy array field is an object");
			property.insert("maxItems".into(), 0.into());
			property.insert(
				"description".into(),
				"PACT wire-compatibility field. Return []; only source-attributed folded_units are committed."
					.into(),
			);
		}
		for field in ["files", "names", "decisions"] {
			properties["key_entities"]["properties"][field]["maxItems"] = 0.into();
		}
	} else {
		if let Some(properties) = schema
			.get_mut("properties")
			.and_then(serde_json::Value::as_object_mut)
		{
			properties.remove("folded_units");
		}
		if let Some(required) = schema.get_mut("required").and_then(|v| v.as_array_mut()) {
			required.retain(|v| v.as_str() != Some("folded_units"));
		}
	}
	schema
}

/// Parse the XML-formatted compression response (used when the provider
/// does not support structured output) into a `CompressionSummary`.
///
/// Tag shape mirrors the rendered summary plus the meta fields the JSON
/// schema carries on the wire — see `XML_OUTPUT_SPEC` in `prompt.rs` for
/// the exact contract sent to the model.
///
/// Tolerant of surrounding prose, code fences, and partial truncation —
/// extracts known tag bodies anywhere in the text. Unknown tags are
/// ignored. Missing tags map to defaults (empty string / empty vec).
///
/// Validation: structural sanity only (filepath non-empty, line bounds,
/// start <= end). The substantive-content gate runs downstream against
/// the parsed struct, matching the JSON path.
pub fn parse_xml_summary(text: &str) -> Result<CompressionSummary> {
	let body = strip_optional_envelope(text);

	let should_compress = extract_text(body, "should_compress")
		.map(|s| parse_bool(&s))
		.ok_or_else(|| anyhow!("compression XML response missing <should_compress> tag"))??;

	let key_entities = extract_text(body, "key_entities")
		.map(|inner| KeyEntities {
			files: extract_items(&inner, "files", "file"),
			names: extract_items(&inner, "names", "name"),
			decisions: extract_items(&inner, "decisions", "decision"),
		})
		.unwrap_or_default();

	let file_context = extract_file_context(body)?;

	Ok(CompressionSummary {
		should_compress,
		original_request: extract_text(body, "original_request").unwrap_or_default(),
		session_context: extract_text(body, "session_context").unwrap_or_default(),
		current_task: extract_text(body, "current_task").unwrap_or_default(),
		progress: extract_text(body, "progress").unwrap_or_default(),
		analysis_findings: extract_items(body, "analysis_findings", "finding"),
		errors_and_corrections: extract_items(body, "errors_and_corrections", "entry"),
		recent_exchanges: extract_items(body, "recent_exchanges", "exchange"),
		key_entities,
		next_steps: extract_text(body, "next_steps").unwrap_or_default(),
		file_context,
		critical_knowledge: extract_items(body, "critical_knowledge", "knowledge"),
		open_loops: extract_items(body, "open_loops", "open_loop"),
		file_states: extract_items(body, "file_states", "state"),
		folded_units: extract_folded_units(body),
	})
}

fn extract_folded_units(body: &str) -> Vec<FoldedUnit> {
	let Some(inner) = extract_text(body, "folded_units") else {
		return Vec::new();
	};
	let mut units = Vec::new();
	let mut cursor = inner.as_str();
	while let Some(start) = cursor.find("<unit>") {
		let rest = &cursor[start + "<unit>".len()..];
		let Some(end) = rest.find("</unit>") else {
			break;
		};
		let unit = &rest[..end];
		let folded = FoldedUnit {
			text: extract_text(unit, "text").unwrap_or_default(),
			kind: extract_text(unit, "kind").unwrap_or_default(),
			status: extract_text(unit, "status").unwrap_or_default(),
			refs: extract_items(unit, "refs", "ref"),
		};
		if !folded.text.trim().is_empty() {
			units.push(folded);
		}
		cursor = &rest[end + "</unit>".len()..];
	}
	units
}

/// Strip an outer markdown code fence if the whole payload is wrapped in
/// one. The model is told to emit raw XML, but some chat providers will
/// re-wrap any tag-heavy response in ```xml … ``` regardless.
fn strip_optional_envelope(text: &str) -> &str {
	let trimmed = text.trim();
	if let Some(after_open) = trimmed.strip_prefix("```") {
		let body = match after_open.find('\n') {
			Some(nl) => &after_open[nl + 1..],
			None => after_open,
		};
		if let Some(inner) = body.strip_suffix("```") {
			return inner.trim();
		}
	}
	trimmed
}

/// Extract the inner text of the first `<tag>…</tag>` occurrence.
/// Whitespace around the body is trimmed. Returns `None` when the tag
/// is absent or unbalanced.
fn extract_text(body: &str, tag: &str) -> Option<String> {
	let open = format!("<{tag}>");
	let close = format!("</{tag}>");
	let start = body.find(&open)? + open.len();
	let end = body[start..].find(&close)? + start;
	Some(body[start..end].trim().to_string())
}

/// Extract repeated `<item>` bodies from inside `<container>…</container>`.
/// Empty items are dropped. Returns an empty vec when the container is
/// absent — callers treat that as "no entries", matching the JSON-schema
/// default of an empty array.
fn extract_items(body: &str, container: &str, item: &str) -> Vec<String> {
	let Some(inner) = extract_text(body, container) else {
		return Vec::new();
	};
	let open = format!("<{item}>");
	let close = format!("</{item}>");
	let mut out = Vec::new();
	let mut cursor = 0usize;
	while let Some(start_rel) = inner[cursor..].find(&open) {
		let start = cursor + start_rel + open.len();
		let Some(end_rel) = inner[start..].find(&close) else {
			break;
		};
		let end = start + end_rel;
		let value = inner[start..end].trim();
		if !value.is_empty() {
			out.push(value.to_string());
		}
		cursor = end + close.len();
	}
	out
}

/// Extract `<range filepath="…" start_line="N" end_line="M"/>` entries
/// from the `<file_context>` block and validate them.
///
/// Validation rules (mirror the JSON schema):
///   - `filepath` non-empty after trimming
///   - `start_line` and `end_line` in `1..=FILE_CONTEXT_LINE_MAX`
///   - `start_line <= end_line`
///
/// An invalid entry fails the whole parse — same strictness as the JSON
/// path's `additionalProperties: false` + range bounds.
fn extract_file_context(body: &str) -> Result<Vec<FileContextEntry>> {
	let Some(inner) = extract_text(body, "file_context") else {
		return Ok(Vec::new());
	};

	let re = regex::Regex::new(
		r#"(?s)<range\s+filepath="([^"]*)"\s+start_line="(\d+)"\s+end_line="(\d+)"\s*/?>"#,
	)
	.expect("static regex compiles");

	let mut entries = Vec::new();
	for caps in re.captures_iter(&inner) {
		let filepath = caps[1].trim().to_string();
		if filepath.is_empty() {
			return Err(anyhow!("compression XML: <range> entry has empty filepath"));
		}
		let start_line: usize = caps[2]
			.parse()
			.map_err(|e| anyhow!("compression XML: invalid start_line: {e}"))?;
		let end_line: usize = caps[3]
			.parse()
			.map_err(|e| anyhow!("compression XML: invalid end_line: {e}"))?;
		if !(1..=FILE_CONTEXT_LINE_MAX).contains(&start_line)
			|| !(1..=FILE_CONTEXT_LINE_MAX).contains(&end_line)
		{
			return Err(anyhow!(
				"compression XML: line numbers out of range (1..={FILE_CONTEXT_LINE_MAX}) for {filepath}: {start_line}-{end_line}"
			));
		}
		if start_line > end_line {
			return Err(anyhow!(
				"compression XML: start_line > end_line for {filepath}: {start_line}-{end_line}"
			));
		}
		entries.push(FileContextEntry {
			filepath,
			start_line,
			end_line,
		});
	}
	Ok(entries)
}

fn parse_bool(s: &str) -> Result<bool> {
	match s.trim().to_ascii_lowercase().as_str() {
		"true" | "yes" | "1" => Ok(true),
		"false" | "no" | "0" => Ok(false),
		other => Err(anyhow!(
			"compression XML: <should_compress> must be true/false, got '{other}'"
		)),
	}
}

/// Inline XML output specification embedded in the system prompt when the
/// provider does not support structured output. The exact tag shape that
/// `parse_xml_summary` understands. Keep this in sync with the parser.
pub const XML_OUTPUT_SPEC: &str = r#"<output_format>
Emit ONE single XML document with the following tags, in this order. Every required tag MUST be present. Use the exact tag names below. Do not add additional tags or attributes.

<should_compress>true|false</should_compress>             (required, exactly true or false)
<original_request>verbatim MOST RECENT user request</original_request>   (required, may be empty when should_compress is false)
<session_context>one sentence</session_context>           (required, may be empty when should_compress is false)
<current_task>1-2 sentences</current_task>                (required, may be empty when should_compress is false)
<progress>2-4 sentences</progress>                        (required, may be empty when should_compress is false)
<analysis_findings>                                       (required container; 0-30 <finding> items)
  <finding>...</finding>
</analysis_findings>
<errors_and_corrections>                                  (required container; 0-10 <entry> items, verbatim feedback/errors)
  <entry>...</entry>
</errors_and_corrections>
<recent_exchanges>                                        (required container; 0-10 <exchange> items covering the [RECENT] span; combine related turns as needed)
  <exchange>...</exchange>
</recent_exchanges>
<key_entities>                                            (required container)
  <files>
    <file>path/to/file.rs:42-58</file>
  </files>
  <names>
    <name>identifier_or_symbol</name>
  </names>
  <decisions>
    <decision>choice with reasoning</decision>
  </decisions>
</key_entities>
<next_steps>1-2 sentences</next_steps>                    (required, may be empty when should_compress is false)
<file_context>                                            (required container; 0-5 entries, self-closing)
  <range filepath="path/from/project/root.rs" start_line="N" end_line="M"/>
</file_context>
<critical_knowledge>                                      (required container; 0-15 <knowledge> items, 2-3 sentences each)
  <knowledge>survives all future compressions</knowledge>
</critical_knowledge>
<open_loops>                                             (required container; 0-8 <open_loop> items, unresolved questions/blockers)
  <open_loop>...</open_loop>
</open_loops>
<file_states>                                            (required container; 0-10 <state> items, 'path — last-known state')
  <state>...</state>
</file_states>
<folded_units>                                           (required container on PACT; 0-40 atomic units)
  <unit>
    <text>one independently supported completed-state claim</text>
    <kind>observation|decision|action|outcome|correction|open_loop|next_action|reference|synthesis</kind>
    <status>established|tentative|superseded|failed|pending|unknown</status>
    <refs><ref>b:source-id</ref></refs>
  </unit>
  Citation rules: never cite archive_reference packet IDs; units citing keep_exact packets may only use status pending/tentative/unknown; every summarize-lane packet ID must be cited by at least one unit.
</folded_units>

Output ONLY the XML. No prose, no code fences, no markdown headers — the response is parsed by exact tag boundaries.
</output_format>"#;

#[cfg(test)]
mod xml_parser_tests {
	use super::*;

	fn minimal_ok_xml() -> String {
		r#"<should_compress>true</should_compress>
<original_request>do the thing</original_request>
<session_context>session brought to here</session_context>
<current_task>finish the thing</current_task>
<progress>started it</progress>
<analysis_findings><finding>root cause is X</finding></analysis_findings>
<errors_and_corrections><entry>don't do Y</entry></errors_and_corrections>
<recent_exchanges><exchange>user asked Z</exchange></recent_exchanges>
<key_entities>
  <files><file>a.rs:1-10</file></files>
  <names><name>foo_fn</name></names>
  <decisions><decision>chose A over B</decision></decisions>
</key_entities>
<next_steps>do the next thing</next_steps>
<file_context><range filepath="a.rs" start_line="1" end_line="10"/></file_context>
<critical_knowledge><knowledge>arch decision: X</knowledge></critical_knowledge>
<open_loops><open_loop>awaiting user decision on Y</open_loop></open_loops>
<file_states><state>a.rs — added foo_fn, compiles</state></file_states>"#
			.to_string()
	}

	#[test]
	fn parses_full_happy_path() {
		let s = parse_xml_summary(&minimal_ok_xml()).unwrap();
		assert!(s.should_compress);
		assert_eq!(s.original_request, "do the thing");
		assert_eq!(s.current_task, "finish the thing");
		assert_eq!(s.analysis_findings, vec!["root cause is X"]);
		assert_eq!(s.errors_and_corrections, vec!["don't do Y"]);
		assert_eq!(s.recent_exchanges, vec!["user asked Z"]);
		assert_eq!(s.key_entities.files, vec!["a.rs:1-10"]);
		assert_eq!(s.key_entities.names, vec!["foo_fn"]);
		assert_eq!(s.key_entities.decisions, vec!["chose A over B"]);
		assert_eq!(s.next_steps, "do the next thing");
		assert_eq!(s.file_context.len(), 1);
		assert_eq!(s.file_context[0].filepath, "a.rs");
		assert_eq!(s.file_context[0].start_line, 1);
		assert_eq!(s.file_context[0].end_line, 10);
		assert_eq!(s.critical_knowledge, vec!["arch decision: X"]);
		assert_eq!(s.open_loops, vec!["awaiting user decision on Y"]);
		assert_eq!(s.file_states, vec!["a.rs — added foo_fn, compiles"]);
	}

	#[test]
	fn parses_should_compress_false_with_empty_fields() {
		let xml = r#"<should_compress>false</should_compress>
<original_request></original_request>
<session_context></session_context>
<current_task></current_task>
<progress></progress>
<analysis_findings></analysis_findings>
<errors_and_corrections></errors_and_corrections>
<recent_exchanges></recent_exchanges>
<key_entities><files></files><names></names><decisions></decisions></key_entities>
<next_steps></next_steps>
<file_context></file_context>
<critical_knowledge></critical_knowledge>"#;
		let s = parse_xml_summary(xml).unwrap();
		assert!(!s.should_compress);
		assert!(s.analysis_findings.is_empty());
		assert!(s.file_context.is_empty());
	}

	#[test]
	fn strips_code_fence_envelope() {
		let xml = format!("```xml\n{}\n```", minimal_ok_xml());
		let s = parse_xml_summary(&xml).unwrap();
		assert!(s.should_compress);
	}

	#[test]
	fn rejects_missing_should_compress() {
		let xml = "<original_request>x</original_request>";
		let err = parse_xml_summary(xml).unwrap_err().to_string();
		assert!(err.contains("should_compress"), "got: {err}");
	}

	#[test]
	fn rejects_invalid_bool() {
		let xml = "<should_compress>maybe</should_compress>";
		let err = parse_xml_summary(xml).unwrap_err().to_string();
		assert!(err.contains("true/false"), "got: {err}");
	}

	#[test]
	fn rejects_inverted_line_range() {
		let xml = r#"<should_compress>true</should_compress>
<file_context><range filepath="a.rs" start_line="20" end_line="10"/></file_context>"#;
		let err = parse_xml_summary(xml).unwrap_err().to_string();
		assert!(err.contains("start_line > end_line"), "got: {err}");
	}

	#[test]
	fn rejects_out_of_range_line() {
		let xml = r#"<should_compress>true</should_compress>
<file_context><range filepath="a.rs" start_line="0" end_line="10"/></file_context>"#;
		let err = parse_xml_summary(xml).unwrap_err().to_string();
		assert!(err.contains("out of range"), "got: {err}");
	}

	#[test]
	fn rejects_empty_filepath() {
		let xml = r#"<should_compress>true</should_compress>
<file_context><range filepath="" start_line="1" end_line="10"/></file_context>"#;
		let err = parse_xml_summary(xml).unwrap_err().to_string();
		assert!(err.contains("empty filepath"), "got: {err}");
	}

	#[test]
	fn parses_multiple_items_and_drops_empties() {
		let xml = r#"<should_compress>true</should_compress>
<analysis_findings>
  <finding>a</finding>
  <finding>  </finding>
  <finding>b</finding>
</analysis_findings>"#;
		let s = parse_xml_summary(xml).unwrap();
		assert_eq!(s.analysis_findings, vec!["a", "b"]);
	}

	#[test]
	fn tolerates_prose_before_and_after() {
		let xml = format!(
			"Sure, here is the output:\n\n{}\n\nLet me know if you need more.",
			minimal_ok_xml()
		);
		let s = parse_xml_summary(&xml).unwrap();
		assert!(s.should_compress);
	}

	#[test]
	fn pact_schema_requires_folds_while_legacy_schema_does_not_expose_them() {
		let pact = build_compression_schema(false, true);
		assert!(pact["properties"].get("folded_units").is_some());
		assert!(pact["required"]
			.as_array()
			.unwrap()
			.iter()
			.any(|field| field == "folded_units"));
		assert_eq!(pact["properties"]["current_task"]["maxLength"], 0);
		assert_eq!(pact["properties"]["critical_knowledge"]["maxItems"], 0);
		assert_eq!(
			pact["properties"]["key_entities"]["properties"]["files"]["maxItems"],
			0
		);

		let legacy = build_compression_schema(false, false);
		assert!(legacy["properties"].get("folded_units").is_none());
		assert!(!legacy["required"]
			.as_array()
			.unwrap()
			.iter()
			.any(|field| field == "folded_units"));
		assert!(legacy["properties"]["current_task"]
			.get("maxLength")
			.is_none());
		assert_eq!(legacy["properties"]["critical_knowledge"]["maxItems"], 15);
	}

	#[test]
	fn parses_and_renders_attributed_fold_with_stable_escaped_id() {
		let xml = format!(
			"{}\n<folded_units><unit><text>A & B</text><kind>outcome</kind><status>established</status><refs><ref>b:one</ref></refs></unit></folded_units>",
			minimal_ok_xml()
		);
		let parsed = parse_xml_summary(&xml).unwrap();
		assert_eq!(parsed.folded_units.len(), 1);
		let rendered = render_summary(&parsed);
		let expected_id = super::super::attention::folded_unit_id(&parsed.folded_units[0]);
		assert!(rendered.contains(&format!("id=\"{expected_id}\"")));
		assert!(rendered.contains("A &amp; B"));
	}

	#[test]
	fn superseded_fold_renders_only_a_runtime_tombstone() {
		let stale = "obsolete state that must not interfere";
		let rendered = render_pact_summary(&CompressionSummary {
			folded_units: vec![FoldedUnit {
				text: stale.into(),
				kind: "observation".into(),
				status: "superseded".into(),
				refs: vec!["b:old".into()],
			}],
			..Default::default()
		});
		assert!(!rendered.contains(stale));
		assert!(rendered.contains("status=\"superseded\""));
		assert!(rendered.contains("must not be treated as current"));
		assert!(rendered.contains("refs=\"b:old\""));
	}
}
