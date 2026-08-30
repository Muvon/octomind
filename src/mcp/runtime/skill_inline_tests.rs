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

use super::*;

fn kinds(line: &str) -> Vec<String> {
	parse_rule_line(line)
		.iter()
		.map(|c| c.to_string())
		.collect()
}

#[test]
fn rule_line_parses_several_checks() {
	assert_eq!(
		kinds("file(Cargo.toml) content(rust) bin(cargo)"),
		["file(Cargo.toml)", "content(rust)", "bin(cargo)"]
	);
}

#[test]
fn rule_line_keeps_nested_parens_inside_one_check() {
	// A regex with a group must survive intact — cutting at the first `)`
	// produced `match(\b(deploy|ship)` which compiles to nothing.
	let checks = parse_rule_line(r"match(\b(deploy|ship)\b) file(Cargo.toml)");
	assert_eq!(checks.len(), 2);
	match &checks[0] {
		ActivateCheck::Match(p) => {
			assert_eq!(p, r"\b(deploy|ship)\b");
			assert!(regex::Regex::new(p).is_ok(), "must be a valid regex");
		}
		other => panic!("expected Match, got {other}"),
	}
	assert_eq!(checks[1].to_string(), "file(Cargo.toml)");
}

#[test]
fn rule_line_stops_at_an_unclosed_check() {
	let checks = parse_rule_line("file(Cargo.toml) match(\\b(oops");
	assert_eq!(checks.len(), 1);
	assert_eq!(checks[0].to_string(), "file(Cargo.toml)");
}

#[test]
fn rule_line_of_junk_yields_nothing() {
	assert!(parse_rule_line("").is_empty());
	assert!(parse_rule_line("no parens here").is_empty());
	assert!(parse_rule_line("unknown(thing)").is_empty());
}

#[test]
fn grep_and_env_checks_split_their_arguments() {
	let checks = parse_rule_line("grep(fn main, *.rs) env(CI=true) env(HOME)");
	assert_eq!(checks.len(), 3);
	assert!(matches!(
		&checks[0],
		ActivateCheck::Grep { pattern, path }
			if pattern == "fn main" && path.as_deref() == Some("*.rs")
	));
	assert!(matches!(
		&checks[1],
		ActivateCheck::Env { var, value }
			if var == "CI" && value.as_deref() == Some("true")
	));
	assert!(matches!(
		&checks[2],
		ActivateCheck::Env { var, value } if var == "HOME" && value.is_none()
	));
}

#[test]
fn semantic_threshold_is_optional_and_phrase_may_contain_commas() {
	match &parse_rule_line("semantic(ship to prod, 0.42)")[0] {
		ActivateCheck::Semantic { phrase, threshold } => {
			assert_eq!(phrase, "ship to prod");
			assert!((threshold - 0.42).abs() < 1e-6);
		}
		other => panic!("expected Semantic, got {other}"),
	}
	// No trailing number → the whole argument is the phrase.
	match &parse_rule_line("semantic(deploy, release, ship)")[0] {
		ActivateCheck::Semantic { phrase, .. } => {
			assert_eq!(phrase, "deploy, release, ship")
		}
		other => panic!("expected Semantic, got {other}"),
	}
}

#[test]
fn word_pattern_respects_word_boundaries() {
	assert!(match_word_pattern("rust", "I love Rust code"));
	assert!(match_word_pattern("rust", "RUST"));
	// Substring inside a longer word must not match.
	assert!(!match_word_pattern("rust", "trustworthy"));
	// Regex metacharacters in the pattern are literal, not syntax: `.`
	// must match a real dot, not any character.
	assert!(match_word_pattern("a.c", "see a.c here"));
	assert!(!match_word_pattern("a.c", "see abc here"));
}

#[test]
fn space_or_array_values_parse_both_syntaxes() {
	assert_eq!(parse_space_or_array("git memory"), ["git", "memory"]);
	assert_eq!(
		parse_space_or_array(r#"["git", "memory"]"#),
		["git", "memory"]
	);
	assert_eq!(parse_space_or_array("[git, memory]"), ["git", "memory"]);
	assert_eq!(parse_space_or_array("['git']"), ["git"]);
	assert!(parse_space_or_array("").is_empty());
	assert!(parse_space_or_array("[]").is_empty());
	assert!(parse_space_or_array("[ , ]").is_empty());
}

#[test]
fn skill_messages_are_detected_and_named() {
	let msg = "<skill name=\"deploy\">\nbody\n</skill>";
	assert!(is_skill_message(msg));
	assert_eq!(extract_skill_name(msg), Some("deploy"));
	// Leading whitespace is tolerated.
	assert!(is_skill_message("\n  <skill name=\"x\">"));

	assert!(!is_skill_message("just a message"));
	assert_eq!(extract_skill_name("just a message"), None);
	// Opening tag without a closing quote yields no name.
	assert_eq!(extract_skill_name("<skill name=\"unterminated"), None);
}

#[test]
fn frontmatter_is_stripped_only_when_present_and_closed() {
	let doc = "---\nname: x\n---\n\nbody text\n";
	assert_eq!(strip_frontmatter(doc), "body text\n");

	// No frontmatter — content is returned untouched.
	let plain = "just a body\n";
	assert_eq!(strip_frontmatter(plain), plain);

	// Unterminated frontmatter is left alone rather than swallowing the file.
	let unterminated = "---\nname: x\nbody\n";
	assert_eq!(strip_frontmatter(unterminated), unterminated);
}
