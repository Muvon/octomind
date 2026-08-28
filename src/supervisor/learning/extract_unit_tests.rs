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

#[test]
fn orientation_parses_attributes_and_provenance() {
	let response = r#"<orientation confidence="high" tags=" arch , rust ">
Auth is delegated to octolib
</orientation>"#;
	let parsed = parse_orientation_tags(response, "developer", "octomind", "session-a");
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
	assert!(!lesson.created.is_empty());
}

#[test]
fn orientation_defaults_and_multiple_tags() {
	let response = r#"<orientation>first subject</orientation>
<orientation confidence="medium" tags="t">second subject</orientation>"#;
	let parsed = parse_orientation_tags(response, "dev", "proj", "src");
	assert_eq!(parsed.len(), 2);
	// Missing confidence defaults to medium with the lower importance.
	assert_eq!(parsed[0].confidence, "medium");
	assert_eq!(parsed[0].importance, 0.55);
	assert_eq!(parsed[0].tags, Vec::<String>::new());
	assert_eq!(parsed[1].importance, 0.55);
	assert_eq!(parsed[1].tags, vec!["t".to_string()]);

	assert!(parse_orientation_tags("no tags here", "dev", "proj", "src").is_empty());
}

#[test]
fn orientation_skips_empty_content_and_truncates_title() {
	let empty = "<orientation confidence=\"high\">\n</orientation>";
	let parsed = parse_orientation_tags(empty, "dev", "proj", "src");
	assert!(parsed.is_empty());

	// ASCII long content: hard cut at 80 bytes plus ellipsis (no word trim).
	let long = format!("<orientation>{}</orientation>", "b".repeat(100));
	let parsed = parse_orientation_tags(&long, "dev", "proj", "src");
	assert_eq!(parsed[0].title, format!("{}...", "b".repeat(80)));

	// Multibyte content: the cut floors to a char boundary.
	let cjk = format!("<orientation>{}</orientation>", "日".repeat(100));
	let parsed = parse_orientation_tags(&cjk, "dev", "proj", "src");
	let title = &parsed[0].title;
	assert!(title.ends_with("..."));
	assert_eq!(title.chars().count(), 26 + 3);
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
