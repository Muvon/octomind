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

//! Unit tests for the pure parsing/budgeting helpers in `extract.rs`.
//! Complements the inline `mod tests`: covers the branches that module leaves
//! unexercised and deliberately does not repeat its assertions.

use super::*;

fn msg(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.into(),
		content: content.into(),
		..Default::default()
	}
}

// --- head_tail ---------------------------------------------------------------

#[test]
fn head_tail_empty_and_exact_budget_pass_through() {
	assert_eq!(head_tail("", 500), "");
	// Exactly at budget: no truncation marker, byte-identical output.
	let exact = "x".repeat(500);
	assert_eq!(head_tail(&exact, 500), exact);
	assert!(!head_tail(&exact, 500).contains("...[middle truncated]..."));
}

#[test]
fn head_tail_four_byte_emoji_boundary_is_safe() {
	// Odd budget → half lands inside 4-byte chars; both cuts must still find a
	// char boundary or the slices panic.
	let long = "😀".repeat(300);
	let out = head_tail(&long, 501);
	assert!(out.contains("...[middle truncated]..."));
	assert!(out.len() < long.len());
	assert!(out.starts_with('😀'));
	assert!(out.ends_with('😀'));
}

// --- is_transcript_evidence --------------------------------------------------

#[test]
fn transcript_evidence_classifies_roles() {
	assert!(is_transcript_evidence(&msg("user", "fix the auth bug")));
	assert!(is_transcript_evidence(&msg("assistant", "I'll fix it")));
	assert!(is_transcript_evidence(&msg("tool", "{\"ok\":true}")));
	assert!(!is_transcript_evidence(&msg("system", "You are helpful")));
}

#[test]
fn transcript_evidence_rejects_non_real_user_turns() {
	// System-managed injections are not genuine user turns.
	let wrapped = msg(
		"user",
		&crate::session::ensure_system_managed("recalled instruction"),
	);
	assert!(!is_transcript_evidence(&wrapped));
	assert!(!is_transcript_evidence(&msg(
		"user",
		"<system-note>\ninjected\n</system-note>"
	)));
	// Empty user content carries no task.
	assert!(!is_transcript_evidence(&msg("user", "   ")));
}

// --- parse_supersedes --------------------------------------------------------

#[test]
fn parse_supersedes_lowercase_padding_and_edges() {
	assert_eq!(parse_supersedes(r#" supersedes="l2""#, 5), Some(1));
	// Value padded with spaces inside the quotes still parses after trim.
	assert_eq!(parse_supersedes(r#" supersedes=" L2 ""#, 5), Some(1));
	// Boundary ids: first and last offered candidate are both valid.
	assert_eq!(parse_supersedes(r#" supersedes="L1""#, 1), Some(0));
	assert_eq!(parse_supersedes(r#" supersedes="L5""#, 5), Some(4));
	// Far out of range is rejected like any other unoffered id.
	assert_eq!(parse_supersedes(r#" supersedes="L99""#, 5), None);
}

// --- parse_lessons_with_evidence ---------------------------------------------

#[test]
fn lessons_without_evidence_or_content_are_dropped() {
	let no_attr = r#"<lesson confidence="high">rule without evidence</lesson>"#;
	assert!(parse_lessons_with_evidence(no_attr, "dev", "proj", "src", 0).is_empty());
	let blank = r#"<lesson evidence="   ">rule with blank evidence</lesson>"#;
	assert!(parse_lessons_with_evidence(blank, "dev", "proj", "src", 0).is_empty());
	let empty_body = r#"<lesson evidence="quote">
</lesson>"#;
	assert!(parse_lessons_with_evidence(empty_body, "dev", "proj", "src", 0).is_empty());
	// Unterminated tag: parsing stops rather than inventing a lesson.
	let unclosed = r#"<lesson evidence="quote">never closed"#;
	assert!(parse_lessons_with_evidence(unclosed, "dev", "proj", "src", 0).is_empty());
}

#[test]
fn lessons_parse_attributes_and_provenance() {
	let response = r#"<lesson evidence="always use bearer tokens" confidence="high" scope="global" tags=" auth , networking ,">
Bearer tokens are mandatory
</lesson>"#;
	let parsed = parse_lessons_with_evidence(response, "developer", "octomind", "session-a", 0);
	assert_eq!(parsed.len(), 1);
	let candidate = &parsed[0];
	assert_eq!(candidate.evidence, "always use bearer tokens");
	let lesson = &candidate.lesson;
	assert_eq!(lesson.content, "Bearer tokens are mandatory");
	assert_eq!(lesson.memory_type, "learning");
	assert_eq!(lesson.confidence, "high");
	assert_eq!(lesson.importance, 0.9);
	assert_eq!(lesson.scope, "global");
	assert_eq!(
		lesson.tags,
		vec!["auth".to_string(), "networking".to_string()]
	);
	assert_eq!(lesson.role, "developer");
	assert_eq!(lesson.project, "octomind");
	assert_eq!(lesson.source, "session-a");
	assert!(!lesson.created.is_empty());
	assert!(lesson.evidence.is_empty());
	assert_eq!(
		lesson.outcome,
		crate::supervisor::learning::TrajectoryOutcome::Unknown
	);
}

#[test]
fn lessons_default_confidence_and_scope_fallbacks() {
	let response = r#"<lesson evidence="q1" scope="GLOBAL">uppercase scope is not global</lesson>
<lesson evidence="q2" confidence="low">low confidence is not high</lesson>
<lesson evidence="q3">all defaults</lesson>"#;
	let parsed = parse_lessons_with_evidence(response, "dev", "proj", "src", 0);
	assert_eq!(parsed.len(), 3);
	// scope must be the exact string "global"; anything else falls back.
	assert_eq!(parsed[0].lesson.scope, "scoped");
	// Only "high" earns 0.9; every other confidence value maps to 0.6.
	assert_eq!(parsed[1].lesson.confidence, "low");
	assert_eq!(parsed[1].lesson.importance, 0.6);
	assert_eq!(parsed[2].lesson.confidence, "medium");
	assert_eq!(parsed[2].lesson.importance, 0.6);
	assert_eq!(parsed[2].lesson.scope, "scoped");
}

#[test]
fn lessons_title_truncates_at_eighty_chars() {
	// Short content: title is the content verbatim.
	let short = r#"<lesson evidence="q">short rule</lesson>"#;
	let parsed = parse_lessons_with_evidence(short, "dev", "proj", "src", 0);
	assert_eq!(parsed[0].lesson.title, "short rule");

	// Long content with an early space: title trims back to the last word
	// boundary before byte 80.
	let spaced = format!(r#"<lesson evidence="q">intro {}</lesson>"#, "x".repeat(200));
	let parsed = parse_lessons_with_evidence(&spaced, "dev", "proj", "src", 0);
	assert_eq!(parsed[0].lesson.title, "intro...");

	// Long content with no spaces at all: hard cut at 80 bytes plus ellipsis.
	let unbroken = format!(r#"<lesson evidence="q">{}</lesson>"#, "a".repeat(200));
	let parsed = parse_lessons_with_evidence(&unbroken, "dev", "proj", "src", 0);
	assert_eq!(parsed[0].lesson.title, format!("{}...", "a".repeat(80)));
}

#[test]
fn lessons_parse_multiple_and_supersedes_index() {
	let response = r#"<lesson evidence="q1" supersedes="L2">replacement rule</lesson>
<lesson evidence="q2">independent rule</lesson>"#;
	let parsed = parse_lessons_with_evidence(response, "dev", "proj", "src", 3);
	assert_eq!(parsed.len(), 2);
	assert_eq!(parsed[0].lesson.content, "replacement rule");
	assert_eq!(parsed[0].supersedes, Some(1));
	assert_eq!(parsed[1].lesson.content, "independent rule");
	assert_eq!(parsed[1].supersedes, None);
}

// --- should_extract_experience -----------------------------------------------

#[test]
fn experience_gate_requires_user_and_tool_messages() {
	let tools_only: Vec<_> = (0..8)
		.map(|i| msg("tool", &format!("evidence {i}")))
		.collect();
	let big = "distinct durable evidence ".repeat(4_000);
	assert!(!should_extract_experience(
		&tools_only,
		&big,
		crate::supervisor::learning::TrajectoryOutcome::Unknown
	));

	let users_only = vec![msg("user", "do the work"), msg("user", "and this")];
	assert!(!should_extract_experience(
		&users_only,
		&big,
		crate::supervisor::learning::TrajectoryOutcome::Verified
	));
}

#[test]
fn experience_gate_outcome_thresholds() {
	let mut messages = vec![msg("user", "investigate the failure")];
	messages.extend((0..7).map(|i| msg("tool", &format!("evidence {i}"))));

	// Labelled outcomes still need a non-trivial transcript.
	assert!(!should_extract_experience(
		&messages,
		"tiny",
		crate::supervisor::learning::TrajectoryOutcome::Verified
	));
	assert!(should_extract_experience(
		&messages,
		&"verified evidence ".repeat(80),
		crate::supervisor::learning::TrajectoryOutcome::Failed
	));
	// Unknown demands 8 tools regardless of transcript size.
	assert!(!should_extract_experience(
		&messages,
		&"distinct durable evidence ".repeat(4_000),
		crate::supervisor::learning::TrajectoryOutcome::Unknown
	));
}

// --- build_transcript --------------------------------------------------------

#[test]
fn transcript_applies_per_role_char_budgets() {
	let messages = vec![
		msg("user", &format!("UHEAD {} UTAIL", "u".repeat(3_000))),
		msg("assistant", &format!("AHEAD {} ATAIL", "a".repeat(1_000))),
		msg("tool", "short tool result"),
	];
	let transcript = build_transcript(&messages);
	// Both over-budget turns keep head and tail around the marker.
	assert!(transcript.contains("UHEAD"));
	assert!(transcript.contains("UTAIL"));
	assert!(transcript.contains("AHEAD"));
	assert!(transcript.contains("ATAIL"));
	assert_eq!(transcript.matches("...[middle truncated]...").count(), 2);
	// Short tool turn passes through under its own label.
	assert!(transcript.contains("[M3 TOOL]: short tool result"));
}

#[test]
fn transcript_empty_input_yields_empty_string() {
	assert_eq!(build_transcript(&[]), "");
}

// --- extract_attr ------------------------------------------------------------

#[test]
fn extract_attr_empty_value_order_and_spaced_values() {
	assert_eq!(extract_attr(r#" key="""#, "key"), Some(String::new()));
	// Attribute need not be first in the string.
	assert_eq!(extract_attr(r#" b="2" a="1""#, "a"), Some("1".into()));
	// Spaces inside the quoted value are preserved verbatim.
	assert_eq!(
		extract_attr(r#" evidence="use bearer tokens now""#, "evidence"),
		Some("use bearer tokens now".into())
	);
}

// --- parse_orientation_tags --------------------------------------------------

fn parse_orientations(
	response: &str,
	messages: &[crate::session::Message],
) -> Vec<crate::supervisor::learning::Lesson> {
	let transcript = build_transcript(messages);
	parse_orientation_tags(
		response,
		&OrientationParseContext {
			messages,
			transcript: &transcript,
			role: "developer",
			project: "octomind",
			source: "session-a",
		},
	)
}

#[test]
fn orientation_parses_attributes_and_provenance() {
	let messages = vec![msg("tool", "octolib owns provider authentication")];
	let response = r#"<orientation confidence="high" tags=" arch , rust " evidence="M1">
Auth is delegated to octolib
</orientation>"#;
	let parsed = parse_orientations(response, &messages);
	assert_eq!(parsed.len(), 1);
	let lesson = &parsed[0];
	assert_eq!(lesson.content, "Auth is delegated to octolib");
	assert_eq!(lesson.memory_type, "orientation");
	assert_eq!(lesson.confidence, "high");
	assert_eq!(lesson.importance, 0.8);
	// Orientation is always scoped, even though lessons can be global.
	assert_eq!(lesson.scope, "scoped");
	assert_eq!(lesson.tags, vec!["arch".to_string(), "rust".to_string()]);
	assert_eq!(lesson.role, "developer");
	assert_eq!(lesson.project, "octomind");
	assert_eq!(lesson.source, "session-a");
	assert_eq!(
		lesson.evidence,
		vec!["session://session-a/message/1".to_string()]
	);
	assert!(!lesson.created.is_empty());
}

#[test]
fn orientation_defaults_and_multiple_tags() {
	let messages = vec![msg("user", "first subject"), msg("tool", "second subject")];
	let response = r#"<orientation evidence="M1">first subject</orientation>
<orientation confidence="medium" tags="t" evidence="M2">second subject</orientation>"#;
	let parsed = parse_orientations(response, &messages);
	assert_eq!(parsed.len(), 2);
	// Missing confidence defaults to medium with the lower importance.
	assert_eq!(parsed[0].confidence, "medium");
	assert_eq!(parsed[0].importance, 0.55);
	assert_eq!(parsed[0].tags, Vec::<String>::new());
	assert_eq!(parsed[1].importance, 0.55);
	assert_eq!(parsed[1].tags, vec!["t".to_string()]);

	assert!(parse_orientations("no tags here", &messages).is_empty());
}

#[test]
fn orientation_skips_empty_content_and_truncates_title() {
	let messages = vec![msg("tool", "durable evidence")];
	let empty = "<orientation confidence=\"high\" evidence=\"M1\">\n</orientation>";
	let parsed = parse_orientations(empty, &messages);
	assert!(parsed.is_empty());

	// ASCII long content: hard cut at 80 bytes plus ellipsis (no word trim).
	let long = format!(
		"<orientation evidence=\"M1\">{}</orientation>",
		"b".repeat(100)
	);
	let parsed = parse_orientations(&long, &messages);
	assert_eq!(parsed[0].title, format!("{}...", "b".repeat(80)));

	// Multibyte content: the cut floors to a char boundary.
	let cjk = format!(
		"<orientation evidence=\"M1\">{}</orientation>",
		"日".repeat(100)
	);
	let parsed = parse_orientations(&cjk, &messages);
	let title = &parsed[0].title;
	assert!(title.ends_with("..."));
	assert_eq!(title.chars().count(), 26 + 3);
}

#[test]
fn orientation_rejects_missing_invalid_or_untrusted_evidence() {
	let messages = vec![
		msg("user", "real project fact"),
		msg("assistant", "unsupported self-report"),
		msg("tool", "observed project fact"),
		msg(
			"user",
			"<instructions>synthetic control-plane note</instructions>",
		),
	];
	for evidence in [
		"",
		"M2",
		"M4",
		"M9",
		"M1,M1",
		"M1,M2",
		"M1,M3,M1",
		"M1,M3,M1,M3,M1",
	] {
		let attr = if evidence.is_empty() {
			String::new()
		} else {
			format!(" evidence=\"{evidence}\"")
		};
		let response = format!("<orientation{attr}>unsupported orientation</orientation>");
		assert!(
			parse_orientations(&response, &messages).is_empty(),
			"evidence {evidence:?} must fail closed"
		);
	}
}

#[test]
fn orientation_rejects_evidence_hidden_by_transcript_budget() {
	let messages = (0..400)
		.map(|index| {
			// High-entropy fixture, deliberately: cl100k collapses repeated
			// characters ("x".repeat(1000) → a handful of tokens), which kept the
			// 400-message transcript UNDER TRANSCRIPT_MAX_TOKENS (32k) and made
			// the omission below impossible. Deterministic pseudo-random hex
			// (LCG over u64, no rand) tokenizes at ~2-3 chars/token, so the
			// ~500 rendered chars per message × 400 messages ≈ 80k tokens —
			// comfortably past the cap. Do not "simplify" back to repeated
			// characters; the test would silently stop covering the budget path.
			let mut state = (index as u64).wrapping_mul(0x9E3779B97F4A7C15);
			let mut block = String::with_capacity(512);
			for _ in 0..32 {
				state = state
					.wrapping_mul(0x9E3779B97F4A7C15)
					.wrapping_add(0xBF58476D1CE4E5B9);
				block.push_str(&format!("{state:016x}"));
			}
			msg("tool", &format!("tool evidence {index} {block}"))
		})
		.collect::<Vec<_>>();
	let transcript = build_transcript(&messages);
	let hidden = (1..=messages.len())
		.find(|number| !transcript.contains(&format!("[M{number} ")))
		.expect("bounded transcript omits at least one middle message");
	let response = format!("<orientation evidence=\"M{hidden}\">hidden evidence</orientation>");
	assert!(parse_orientations(&response, &messages).is_empty());
}

// --- word_overlap / best_overlap ---------------------------------------------

#[test]
fn word_overlap_ratio_is_case_insensitive_and_bounded() {
	assert_eq!(word_overlap("", "anything"), 0.0);
	assert_eq!(word_overlap("Alpha", "alpha"), 1.0);
	assert_eq!(word_overlap("alpha beta", "alpha"), 0.5);
}

#[test]
fn best_overlap_picks_strongest_above_threshold() {
	let existing = vec![
		Lesson {
			content: "alpha beta".into(),
			..Default::default()
		},
		Lesson {
			content: "alpha beta gamma".into(),
			..Default::default()
		},
	];
	let best = best_overlap("alpha beta gamma", &existing).expect("overlap above threshold");
	assert_eq!(best.content, "alpha beta gamma");

	// Exactly 0.6 is below the strictly-greater threshold.
	let boundary = vec![Lesson {
		content: "a b c".into(),
		..Default::default()
	}];
	assert!(best_overlap("a b c d e", &boundary).is_none());
}

// ---------------------------------------------------------------------------
// build_transcript: role and emptiness edges.
// ---------------------------------------------------------------------------

#[test]
fn build_transcript_skips_unknown_roles_and_empty_content() {
	let messages = vec![
		crate::session::Message {
			role: "system".to_string(),
			content: "system prompt never enters the transcript".to_string(),
			..Default::default()
		},
		crate::session::Message {
			role: "user".to_string(),
			content: "   ".to_string(),
			..Default::default()
		},
		crate::session::Message {
			role: "user".to_string(),
			content: "real turn".to_string(),
			..Default::default()
		},
	];
	let transcript = build_transcript(&messages);
	assert!(!transcript.contains("system prompt"));
	assert_eq!(
		transcript.matches("USER]:").count(),
		1,
		"the whitespace-only turn is skipped, the real one kept"
	);
	assert!(transcript.contains("USER]: real turn"));
}

// ---------------------------------------------------------------------------
// Lesson tag scanner: malformed markup stops the scan.
// ---------------------------------------------------------------------------

#[test]
fn lesson_scanner_stops_at_unterminated_open_tags() {
	let no_bracket = r#"<lesson evidence="quote" never closed"#;
	assert!(parse_lessons_with_evidence(no_bracket, "r", "p", "s", 0).is_empty());
	let attrs_no_close = r#"<lesson evidence="quote">body without end tag"#;
	assert!(parse_lessons_with_evidence(attrs_no_close, "r", "p", "s", 0).is_empty());
}

// ---------------------------------------------------------------------------
// Orientation tag scanner: same malformed-markup contract.
// ---------------------------------------------------------------------------

#[test]
fn orientation_scanner_stops_at_malformed_tags() {
	let context = OrientationParseContext {
		messages: &[crate::session::Message {
			role: "user".to_string(),
			content: "grounding text".to_string(),
			..Default::default()
		}],
		transcript: "grounding text",
		role: "r",
		project: "p",
		source: "s",
	};
	let no_bracket = "<orientation never closed";
	assert!(parse_orientation_tags(no_bracket, &context).is_empty());
	let no_end = "<orientation>body without end";
	assert!(parse_orientation_tags(no_end, &context).is_empty());
}

// ---------------------------------------------------------------------------
// Experience parser: structural rejections and the importance ladder.
// ---------------------------------------------------------------------------

fn experience_context<'a>(
	messages: &'a [crate::session::Message],
	transcript: &'a str,
	outcome: crate::supervisor::learning::TrajectoryOutcome,
) -> ExperienceParseContext<'a> {
	ExperienceParseContext {
		messages,
		transcript,
		reconcile: &[],
		role: "r",
		project: "p",
		source: "s",
		outcome,
	}
}

fn grounded_messages() -> Vec<crate::session::Message> {
	vec![
		crate::session::Message {
			role: "user".to_string(),
			content: "never silently switch the resolved model".to_string(),
			..Default::default()
		},
		crate::session::Message {
			role: "tool".to_string(),
			content: "provider error: invalid continuation id".to_string(),
			..Default::default()
		},
	]
}

#[test]
fn experience_body_missing_a_required_heading_is_rejected() {
	let messages = grounded_messages();
	let transcript = build_transcript(&messages);
	let context = experience_context(
		&messages,
		&transcript,
		crate::supervisor::learning::TrajectoryOutcome::Verified,
	);
	let body = experience_body().replace("## Reuse conditions", "## Notes");
	let tag =
		format!(r#"<experience title="T" confidence="high" evidence="M1,M2">{body}</experience>"#);
	assert!(
		parse_experience_tag(&tag, &context).is_none(),
		"a body without every required section is not an experience"
	);
}

#[test]
fn experience_with_an_empty_title_is_rejected() {
	let messages = grounded_messages();
	let transcript = build_transcript(&messages);
	let context = experience_context(
		&messages,
		&transcript,
		crate::supervisor::learning::TrajectoryOutcome::Verified,
	);
	let tag = format!(
		r#"<experience title="   " confidence="high" evidence="M1,M2">{}</experience>"#,
		experience_body()
	);
	assert!(parse_experience_tag(&tag, &context).is_none());
}

#[test]
fn experience_importance_follows_the_outcome_and_confidence_ladder() {
	let messages = grounded_messages();
	let transcript = build_transcript(&messages);
	let cases = [
		(
			crate::supervisor::learning::TrajectoryOutcome::Verified,
			"high",
			0.85,
		),
		(
			crate::supervisor::learning::TrajectoryOutcome::Verified,
			"medium",
			0.75,
		),
		(
			crate::supervisor::learning::TrajectoryOutcome::Failed,
			"high",
			0.7,
		),
		(
			crate::supervisor::learning::TrajectoryOutcome::Unknown,
			"high",
			0.55,
		),
	];
	for (outcome, confidence, expected) in cases {
		let context = experience_context(&messages, &transcript, outcome);
		let tag = format!(
			r#"<experience title="T" confidence="{confidence}" evidence="M1,M2">{}</experience>"#,
			experience_body()
		);
		let parsed = parse_experience_tag(&tag, &context)
			.unwrap_or_else(|| panic!("parses for {outcome:?}/{confidence}"));
		assert!(
			(parsed.lesson.importance - expected).abs() < 1e-9,
			"{outcome:?}/{confidence}: {} != {expected}",
			parsed.lesson.importance
		);
	}
}

#[test]
fn experience_verdict_support_helper_parses_and_rejects() {
	assert_eq!(
		parse_experience_supported(r#"{"supported":true}"#),
		Some(true)
	);
	assert_eq!(
		parse_experience_supported(r#"{"supported":false}"#),
		Some(false)
	);
	assert_eq!(parse_experience_supported("not json"), None);
}

#[test]
fn experience_evidence_render_bails_on_an_absent_citation() {
	let messages = grounded_messages();
	let transcript = build_transcript(&messages);
	let context = experience_context(
		&messages,
		&transcript,
		crate::supervisor::learning::TrajectoryOutcome::Verified,
	);
	let tag = format!(
		r#"<experience title="T" confidence="high" evidence="M1,M9">{}</experience>"#,
		experience_body()
	);
	assert!(
		parse_experience_tag(&tag, &context).is_none(),
		"an absent citation is rejected at parse time"
	);
	let ok_tag = format!(
		r#"<experience title="T" confidence="high" evidence="M1,M2">{}</experience>"#,
		experience_body()
	);
	let ok =
		parse_experience_tag(&ok_tag, &context).expect("M1,M2 parse against the real transcript");
	let error = render_experience_evidence(&ok, &messages[..1])
		.expect_err("M2 is absent from the truncated slice");
	assert!(
		error.to_string().contains("M2"),
		"the absent citation is named: {error}"
	);
}

#[test]
fn experience_evidence_render_labels_each_cited_role() {
	let mut messages = grounded_messages();
	messages.push(crate::session::Message {
		role: "assistant".to_string(),
		content: "I preserved the resolved model identity".to_string(),
		..Default::default()
	});
	let transcript = build_transcript(&messages);
	let context = experience_context(
		&messages,
		&transcript,
		crate::supervisor::learning::TrajectoryOutcome::Verified,
	);
	let tag = format!(
		r#"<experience title="T" confidence="high" evidence="M1,M2,M3">{}</experience>"#,
		experience_body()
	);
	let parsed = parse_experience_tag(&tag, &context).expect("three valid citations");
	let rendered = render_experience_evidence(&parsed, &messages).expect("all citations exist");
	assert!(rendered.contains("[M1 USER]"));
	assert!(rendered.contains("[M2 TOOL]"));
	assert!(
		!rendered.contains("[M3 ASSISTANT]"),
		"assistant citations are validated but never rendered"
	);
}

fn experience_body() -> String {
	format!(
			"## Objective\nDiagnose why an authenticated request repeatedly failed across the provider boundary.\n\n## Durable knowledge\n{}\n\n## Outcome and evidence\nThe tool result established that the provider rejects a stale continuation identifier, while the user confirmed that fallback to another resolved model is forbidden. The verified recovery preserves the resolved model and clears only the invalid continuation.\n\n## Reuse conditions\nApply this when a resumed request fails before tool execution with an invalid continuation identifier. Re-check the current provider contract because external APIs may change.",
			"The continuation belongs to the exact resolved provider and model identity. Recovery must keep that identity stable, distinguish transport failure from task failure, and avoid silent fallback. ".repeat(3)
		)
}
