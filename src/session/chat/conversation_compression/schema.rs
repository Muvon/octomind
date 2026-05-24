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
use serde::Deserialize;

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

	push_text(&mut out, "next_steps", &summary.next_steps);

	out.trim_end().to_string()
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
pub fn build_compression_schema(force: bool) -> serde_json::Value {
	let should_compress_desc = if force {
		"Compression has been forced by the user. MUST be true."
	} else {
		"True if the transcript contains older exchanges that can be safely compressed without losing information needed to continue. False only if the transcript is already minimal."
	};

	serde_json::json!({
		"type": "object",
		"additionalProperties": false,
		"properties": {
			"should_compress": {
				"type": "boolean",
				"description": should_compress_desc
			},
			"original_request": {
				"type": "string",
				"description": "The user's original task statement. Quote verbatim from the very first user turn in the transcript; OR, if a prior **ORIGINAL REQUEST** exists in a previous summary inside the transcript, carry it forward unchanged. Never paraphrase."
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
				"description": "2–4 sentences: what was completed, what is in progress, the outcome so far. If a prior summary exists in the transcript, extend (do not replace) its progress narrative."
			},
			"analysis_findings": {
				"type": "array",
				"items": { "type": "string" },
				"maxItems": 8,
				"description": "3–6 bullets: conclusions from investigation — root causes, behaviours, code-location-specific discoveries. If a prior summary exists, carry its findings forward and append new ones."
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
				"description": "One short paraphrase per [RECENT]-tagged turn. Keep concrete details and decisions intact."
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
				"maxItems": 5,
				"description": "Survives ALL future compressions. Architectural decisions, hidden constraints, user preferences, root-cause findings. 2–3 sentences each. Include only when truly critical — not routine progress."
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
			"critical_knowledge"
		]
	})
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
	})
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
<original_request>verbatim first user request</original_request>   (required, may be empty when should_compress is false)
<session_context>one sentence</session_context>           (required, may be empty when should_compress is false)
<current_task>1-2 sentences</current_task>                (required, may be empty when should_compress is false)
<progress>2-4 sentences</progress>                        (required, may be empty when should_compress is false)
<analysis_findings>                                       (required container; 0-8 <finding> items)
  <finding>...</finding>
</analysis_findings>
<errors_and_corrections>                                  (required container; 0-10 <entry> items, verbatim feedback/errors)
  <entry>...</entry>
</errors_and_corrections>
<recent_exchanges>                                        (required container; 0-10 <exchange> items, one per [RECENT] turn)
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
<critical_knowledge>                                      (required container; 0-5 <knowledge> items, 2-3 sentences each)
  <knowledge>survives all future compressions</knowledge>
</critical_knowledge>

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
<critical_knowledge><knowledge>arch decision: X</knowledge></critical_knowledge>"#
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
}
