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

use serde::Deserialize;

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
