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
			[[guard]]
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
fn generated_shadow_guard_observes_without_blocking() {
	let mut rules = Guardrails::default();
	let generated = Guardrails::parse(
		r#"
			[[guard]]
			match = "shell"
			message = "generated block"
			"#,
	)
	.unwrap();
	rules.append_generated(generated, "evo-shadow", true);
	let evaluation = evaluate_guards(&rules, Some("shell"), &json!({}), &[], &loaded(&[]));
	assert!(evaluation.blocked.is_none());
	assert_eq!(evaluation.shadow_ids, vec!["evo-shadow"]);
}

#[test]
fn generated_binding_without_registry_fails_closed() {
	let mut rules = Guardrails::default();
	let generated = Guardrails::parse(
		r#"
			[[guard]]
			match = "shell"
			message = "generated block"
			"#,
	)
	.unwrap();
	rules.append_generated(generated, "evo-trial", false);
	let evaluation = evaluate_guards(&rules, Some("shell"), &json!({}), &[], &loaded(&[]));
	assert!(evaluation.blocked.is_none());
	assert_eq!(evaluation.shadow_ids, vec!["evo-trial"]);
}

#[test]
fn has_capability_required() {
	let g = Guardrails::parse(
		r#"
			[[guard]]
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
fn duplicate_validator_name_rejected() {
	// Two validators with the same name share one cursor → one silently
	// never fires. Must fail loudly at load.
	let err = Guardrails::parse(
		r#"
			[[validator]]
			name = "tests"
			script = "a.sh"
			[[validator]]
			name = "tests"
			script = "b.sh"
			"#,
	)
	.unwrap_err();
	assert!(err.to_string().contains("duplicate validator"), "{err}");
}

#[test]
fn duplicate_pipe_name_rejected() {
	let err = Guardrails::parse(
		r#"
			[[pipe]]
			name = "x"
			command = "a.sh"
			[[pipe]]
			name = "x"
			command = "b.sh"
			"#,
	)
	.unwrap_err();
	assert!(err.to_string().contains("duplicate pipe"), "{err}");
}

#[test]
fn when_unused_lifts_after_use() {
	// `-filesystem` = "no filesystem call in history yet" — fires (blocks)
	// only while the user has not exercised the filesystem capability.
	let g = Guardrails::parse(
		r#"
			[[guard]]
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
			[[guard]]
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
			[[guard]]
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
			[[guard]]
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
			[[guard]]
			match = "shell(command=git)"
			message = "first"
			[[guard]]
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

#[test]
fn role_filter_is_empty_means_all_roles() {
	assert!(role_matches(&[], "developer:general"));
}

#[test]
fn role_filter_matches_exact_and_domain_prefix() {
	let filter = vec!["developer".to_string()];
	assert!(role_matches(&filter, "developer"));
	assert!(role_matches(&filter, "developer:general"));
	// A `:` separator is required — a longer name that merely starts with
	// the filter is a different role.
	assert!(!role_matches(&filter, "developer-lite"));
	assert!(!role_matches(&filter, "developerx"));
	assert!(!role_matches(&filter, "assistant"));
	// Prefix direction matters: the filter must not match a shorter role.
	assert!(!role_matches(
		&["developer:general".to_string()],
		"developer"
	));
}

#[test]
fn role_filter_matches_any_listed_entry() {
	let filter = vec!["assistant".to_string(), "doctor".to_string()];
	assert!(role_matches(&filter, "doctor:blood"));
	assert!(role_matches(&filter, "assistant"));
	assert!(!role_matches(&filter, "developer:general"));
}
