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

// ---------------------------------------------------------------------------
// Raw `has` field: the list form.
// ---------------------------------------------------------------------------

#[test]
fn has_accepts_a_list_of_capabilities() {
	let g = Guardrails::parse(
		r#"
			[[guard]]
			match = "shell"
			has = ["network", "filesystem"]
			message = "needs both"
			"#,
	)
	.unwrap();
	assert_eq!(g.guards.len(), 1);
	assert_eq!(g.guards[0].has, vec!["network", "filesystem"]);
}

// ---------------------------------------------------------------------------
// load_from_workdir: present, absent, and unparseable files.
// ---------------------------------------------------------------------------

#[test]
fn load_from_workdir_reads_a_valid_file() {
	let tmp = tempfile::tempdir().expect("tempdir");
	std::fs::create_dir_all(tmp.path().join(".agents")).expect("agents dir");
	std::fs::write(
		tmp.path().join(".agents").join("guardrails.toml"),
		r#"
			[[guard]]
			match = "shell"
			message = "no shell"
			"#,
	)
	.expect("write guardrails");
	let g = Guardrails::load_from_workdir(tmp.path());
	assert_eq!(g.guards.len(), 1, "the file is parsed and compiled");
}

#[test]
fn load_from_workdir_without_a_file_is_default() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let g = Guardrails::load_from_workdir(tmp.path());
	assert!(g.guards.is_empty());
	assert!(g.pipes.is_empty());
}

#[test]
fn load_from_workdir_degrades_to_default_on_a_bad_file() {
	let tmp = tempfile::tempdir().expect("tempdir");
	std::fs::create_dir_all(tmp.path().join(".agents")).expect("agents dir");
	std::fs::write(
		tmp.path().join(".agents").join("guardrails.toml"),
		"this = [not : valid",
	)
	.expect("write broken guardrails");
	let g = Guardrails::load_from_workdir(tmp.path());
	assert!(g.guards.is_empty(), "a broken file never blocks startup");
}

// ---------------------------------------------------------------------------
// parse(): loud rejection of malformed declarations.
// ---------------------------------------------------------------------------

#[test]
fn a_pipe_without_a_name_is_rejected() {
	let err = Guardrails::parse("[[pipe]]\ncommand = \"a.sh\"\n").unwrap_err();
	assert!(err.to_string().contains("missing field `name`"), "{err}");
}

#[test]
fn a_pipe_without_a_command_is_rejected() {
	let err = Guardrails::parse("[[pipe]]\nname = \"x\"\n").unwrap_err();
	assert!(err.to_string().contains("missing field `command`"), "{err}");
}

#[test]
fn a_pipe_with_an_invalid_match_regex_is_rejected() {
	let err =
		Guardrails::parse("[[pipe]]\nname = \"x\"\ncommand = \"a.sh\"\nmatch = \"([unclosed\"\n")
			.unwrap_err();
	assert!(err.to_string().contains("invalid match regex"), "{err}");
}

#[test]
fn a_guard_when_entry_without_a_sign_is_rejected() {
	let err = Guardrails::parse(
		r#"
			[[guard]]
			match = "shell"
			when = ["shell"]
			message = "no"
			"#,
	)
	.unwrap_err();
	assert!(
		err.to_string().contains("must start with `+` or `-`"),
		"{err}"
	);
}

#[test]
fn a_hook_with_match_result_and_script_compiles() {
	let g = Guardrails::parse(
		r#"
			[[hook]]
			match = "shell"
			result = "error"
			on = "success"
			script = "report.sh"
			"#,
	)
	.unwrap();
	assert_eq!(g.hooks.len(), 1);
	assert!(g.hooks[0].trigger.is_some());
	assert!(g.hooks[0].result_regex.is_some());
}

#[test]
fn a_hook_with_an_invalid_result_regex_is_rejected() {
	let err = Guardrails::parse("[[hook]]\nresult = \"([bad\"\nscript = \"a.sh\"\n").unwrap_err();
	assert!(err.to_string().contains("invalid result regex"), "{err}");
}

#[test]
fn a_hook_without_a_script_is_rejected() {
	let err = Guardrails::parse("[[hook]]\nmatch = \"shell\"\n").unwrap_err();
	assert!(err.to_string().contains("missing field `script`"), "{err}");
}

#[test]
fn a_hook_with_an_invalid_match_target_is_rejected() {
	let err =
		Guardrails::parse("[[hook]]\nmatch = \"shell([bad\"\nscript = \"a.sh\"\n").unwrap_err();
	assert!(err.to_string().contains("invalid match"), "{err}");
}

#[test]
fn a_validator_without_a_name_is_rejected() {
	let err = Guardrails::parse("[[validator]]\nscript = \"a.sh\"\n").unwrap_err();
	assert!(err.to_string().contains("missing field `name`"), "{err}");
}

#[test]
fn a_validator_without_a_script_is_rejected() {
	let err = Guardrails::parse("[[validator]]\nname = \"v\"\n").unwrap_err();
	assert!(err.to_string().contains("missing field `script`"), "{err}");
}

#[test]
fn a_validator_with_an_invalid_match_regex_is_rejected() {
	let err =
		Guardrails::parse("[[validator]]\nname = \"v\"\nscript = \"a.sh\"\nmatch = \"([bad\"\n")
			.unwrap_err();
	assert!(err.to_string().contains("invalid match regex"), "{err}");
}

#[test]
fn validator_when_entries_accept_both_signs_and_reject_none() {
	let g = Guardrails::parse(
		r#"
			[[validator]]
			name = "v"
			script = "a.sh"
			when = ["+shell", "-network"]
			"#,
	)
	.unwrap();
	assert_eq!(g.validators.len(), 1);

	let err = Guardrails::parse(
		r#"
			[[validator]]
			name = "v"
			script = "a.sh"
			when = ["shell"]
			"#,
	)
	.unwrap_err();
	assert!(
		err.to_string().contains("must start with `+` or `-`"),
		"{err}"
	);
}

// ---------------------------------------------------------------------------
// parse_target / split_arg / target_matches edges.
// ---------------------------------------------------------------------------

#[test]
fn parse_target_rejects_empty_and_malformed_forms() {
	assert!(parse_target("").is_err());
	assert!(parse_target("   ").is_err());
	let unclosed = parse_target("shell(rm").unwrap_err();
	assert!(
		unclosed.to_string().contains("missing closing"),
		"{unclosed}"
	);
	let no_capability = parse_target("(rm)").unwrap_err();
	assert!(
		no_capability.to_string().contains("empty capability"),
		"{no_capability}"
	);
	let bad_regex = parse_target("shell([bad)").unwrap_err();
	assert!(
		bad_regex.to_string().contains("invalid regex"),
		"{bad_regex}"
	);
}

#[test]
fn split_arg_treats_a_non_word_head_as_part_of_the_regex() {
	// "a-b=x" is not arg=regex (head has a dash) — the whole inner string
	// stays the regex.
	let t = parse_target("shell(a-b=x)").unwrap();
	assert!(t.arg_name.is_none());
	assert!(t.regex.as_ref().unwrap().as_str().contains("a-b=x"));
}

#[test]
fn target_matches_without_a_capability_never_fires() {
	let t = parse_target("shell").unwrap();
	assert!(!target_matches(&t, None, &json!({})));
}

#[test]
fn an_arg_target_on_a_missing_param_matches_only_an_empty_haystack() {
	let t = parse_target("shell(command=^$)").unwrap();
	let params = json!({ "other": "value" });
	assert!(
		target_matches(&t, Some("shell"), &params),
		"a missing arg is an empty haystack, so ^$ matches"
	);
	let t2 = parse_target("shell(command=never-empty)").unwrap();
	assert!(!target_matches(&t2, Some("shell"), &params));
}

#[test]
fn a_whole_params_target_matches_the_serialized_object() {
	let t = parse_target("shell(secret\\.env)").unwrap();
	let params = json!({ "paths": ["a.rs", "secret.env"] });
	assert!(target_matches(&t, Some("shell"), &params));
	let clean = json!({ "paths": ["a.rs"] });
	assert!(!target_matches(&t, Some("shell"), &clean));
}
