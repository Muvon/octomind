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
	assert_eq!(check(&g, Some("shell"), &p, &[], &loaded(&[])).as_deref(), Some("no"),);
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
	let log: Vec<CallRecord> = vec![(Some("filesystem".to_string()), json!({ "path": "src/main.rs" }))];
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
	let log: Vec<CallRecord> = vec![(Some("shell".to_string()), json!({ "command": "git status" }))];
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
	assert!(role_matches(&[], "developer"));
}

#[test]
fn a_filter_naming_a_namespace_matches_its_base_agent_and_all_its_variants() {
	let filter = vec!["developer".to_string()];
	assert!(role_matches(&filter, "developer"));
	assert!(role_matches(&filter, "developer:rust"));
	// A name that merely starts with the filter is a different namespace.
	assert!(!role_matches(&filter, "developer-lite"));
	assert!(!role_matches(&filter, "developer-lite:rust"));
	assert!(!role_matches(&filter, "developerx"));
	assert!(!role_matches(&filter, "assistant"));
	assert!(!role_matches(&["developer-lite".to_string()], "developer"));
}

#[test]
fn a_filter_with_a_variant_matches_that_one_agent() {
	let filter = vec!["developer:rust".to_string()];
	assert!(role_matches(&filter, "developer:rust"));
	assert!(!role_matches(&filter, "developer"));
	assert!(!role_matches(&filter, "developer:go"));
	assert!(!role_matches(&filter, "developer:rust-x"));
}

#[test]
fn the_default_variant_is_the_base_agent_in_a_filter_and_as_the_current_role() {
	assert!(role_matches(&["developer:default".to_string()], "developer"));
	assert!(role_matches(&["developer".to_string()], "developer:default"));
	// `developer:default` is the agent `developer`, so as a filter it names the namespace.
	assert!(role_matches(&["developer:default".to_string()], "developer:rust"));
}

#[test]
fn role_filter_matches_any_listed_entry() {
	let filter = vec!["assistant".to_string(), "doctor".to_string()];
	assert!(role_matches(&filter, "doctor"));
	assert!(role_matches(&filter, "assistant"));
	assert!(!role_matches(&filter, "developer"));
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
// snapshot_scripts: rules run what was on disk when they were installed.
// ---------------------------------------------------------------------------

fn validator_rules(script: &str) -> Guardrails {
	Guardrails::parse(&format!("[[validator]]\nname = \"v\"\nscript = \"{script}\"\n")).expect("parse")
}

fn snapshot_content(snapshot: &ScriptSnapshot) -> &[u8] {
	match snapshot {
		ScriptSnapshot::Taken { content, .. } => content,
		other => panic!("expected a taken snapshot, got {other:?}"),
	}
}

#[test]
fn snapshot_captures_content_and_ignores_later_edits() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let script = tmp.path().join("check.sh");
	std::fs::write(&script, "original").expect("write script");
	let mut g = validator_rules("check.sh");
	g.snapshot_scripts(tmp.path());

	std::fs::write(&script, "edited").expect("edit script");
	assert_eq!(snapshot_content(&g.validators[0].snapshot), b"original");
	match &g.validators[0].snapshot {
		ScriptSnapshot::Taken { source, .. } => assert_eq!(source, &script),
		other => panic!("expected a taken snapshot, got {other:?}"),
	}
}

#[test]
fn snapshot_scripts_keeps_an_existing_snapshot() {
	// Rules copied into a delegated agent's scope are re-snapshotted on merge;
	// that must not pick up an edit made after session start.
	let tmp = tempfile::tempdir().expect("tempdir");
	let script = tmp.path().join("check.sh");
	std::fs::write(&script, "original").expect("write script");
	let mut g = validator_rules("check.sh");
	g.snapshot_scripts(tmp.path());

	std::fs::write(&script, "edited").expect("edit script");
	g.snapshot_scripts(tmp.path());
	assert_eq!(snapshot_content(&g.validators[0].snapshot), b"original");
}

#[test]
fn missing_and_untaken_scripts_never_fall_back_to_the_live_path() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let mut g = validator_rules("late.sh");
	g.snapshot_scripts(tmp.path());

	// Created after session start: still not runnable this session.
	std::fs::write(tmp.path().join("late.sh"), "created later").expect("write script");
	assert!(g.validators[0].snapshot.command(tmp.path()).is_err());
	assert!(ScriptSnapshot::default().command(tmp.path()).is_err());
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
	let err = Guardrails::parse("[[pipe]]\nname = \"x\"\ncommand = \"a.sh\"\nmatch = \"([unclosed\"\n").unwrap_err();
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
	assert!(err.to_string().contains("must start with `+` or `-`"), "{err}");
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
	let err = Guardrails::parse("[[hook]]\nmatch = \"shell([bad\"\nscript = \"a.sh\"\n").unwrap_err();
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
	let err = Guardrails::parse("[[validator]]\nname = \"v\"\nscript = \"a.sh\"\nmatch = \"([bad\"\n").unwrap_err();
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
	assert!(err.to_string().contains("must start with `+` or `-`"), "{err}");
}

// ---------------------------------------------------------------------------
// parse_target / split_arg / target_matches edges.
// ---------------------------------------------------------------------------

#[test]
fn parse_target_rejects_empty_and_malformed_forms() {
	assert!(parse_target("").is_err());
	assert!(parse_target("   ").is_err());
	let unclosed = parse_target("shell(rm").unwrap_err();
	assert!(unclosed.to_string().contains("missing closing"), "{unclosed}");
	let no_capability = parse_target("(rm)").unwrap_err();
	assert!(
		no_capability.to_string().contains("empty capability"),
		"{no_capability}"
	);
	let bad_regex = parse_target("shell([bad)").unwrap_err();
	assert!(bad_regex.to_string().contains("invalid regex"), "{bad_regex}");
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

#[test]
fn plugin_rules_carry_their_id_and_mode() {
	let rules = Guardrails::parse_plugin_rules(
		r#"
			[[guard]]
			id = "g-apply"
			mode = "apply"
			match = "shell"
			message = "no"

			[[guard]]
			id = "g-observe"
			mode = "observe"
			match = "shell"
			message = "no"

			[[hook]]
			id = "h-observe"
			mode = "observe"
			script = "h.sh"

			[[guard]]
			mode = "apply"
			match = "shell"
			message = "no id"

			[[guard]]
			id = "g-odd"
			mode = "sometimes"
			match = "shell"
			message = "unknown mode"
			"#,
	)
	.unwrap();
	let bindings: Vec<_> = rules
		.bindings()
		.map(|binding| (binding.id.as_str(), binding.shadow, binding.plugin))
		.collect();
	assert_eq!(
		bindings,
		vec![
			("g-apply", false, true),
			("g-observe", true, true),
			("h-observe", true, true)
		]
	);
}

#[test]
fn plugin_rules_that_are_not_toml_are_an_error() {
	assert!(Guardrails::parse_plugin_rules("[[guard").is_err());
}

#[test]
fn dropping_plugin_rules_keeps_the_user_and_evolution_rules() {
	let mut rules = Guardrails::parse("[[pipe]]\nname = \"mine\"\ncommand = \"mine.sh\"\n").unwrap();
	let generated = Guardrails::parse("[[guard]]\nmatch = \"shell\"\nmessage = \"no\"\n").unwrap();
	rules.append_generated(generated, "evolved", false);
	rules.append_compiled(
		Guardrails::parse_plugin_rules(
			"[[guard]]\nid = \"p\"\nmode = \"apply\"\nmatch = \"shell\"\nmessage = \"no\"\n",
		)
		.unwrap(),
	);
	assert!(rules.has_user_pipe());

	rules.drop_plugin_rules();

	let ids: Vec<_> = rules.bindings().map(|binding| binding.id.as_str()).collect();
	assert_eq!(ids, vec!["evolved"]);
	assert_eq!(rules.pipes.len(), 1);
}

#[test]
fn a_plugin_rule_is_shadow_by_its_mode_alone() {
	let rules = Guardrails::parse_plugin_rules(
		"[[guard]]\nid = \"seen\"\nmode = \"observe\"\nmatch = \"shell\"\nmessage = \"no\"\n\n\
		 [[guard]]\nid = \"live\"\nmode = \"apply\"\nmatch = \"shell\"\nmessage = \"stop\"\n",
	)
	.unwrap();
	let params = json!({});

	// Neither id is in the evolution registry, which would make both shadow.
	let only_observed = Guardrails {
		guards: vec![rules.guards[0].clone()],
		..Default::default()
	};
	let evaluation = evaluate_guards(&only_observed, Some("shell"), &params, &[], &loaded(&[]));
	assert!(evaluation.blocked.is_none());
	assert_eq!(evaluation.shadow_ids, vec!["seen".to_string()]);

	let evaluation = evaluate_guards(&rules, Some("shell"), &params, &[], &loaded(&[]));
	let (message, binding) = evaluation.blocked.expect("the apply guard denies");
	assert_eq!(message, "stop");
	assert_eq!(binding.unwrap().id, "live");
	assert_eq!(evaluation.shadow_ids, vec!["seen".to_string()]);
}

#[test]
fn a_role_filter_with_a_colon_is_accepted_and_kept() {
	let guardrails = Guardrails::parse(
		"[[pipe]]\nname = \"p\"\ncommand = \"c.sh\"\nroles = [\"developer:general\"]\n\n[[validator]]\nname = \"v\"\nscript = \"v.sh\"\nroles = [\"a\", \"doctor:blood\"]\n",
	);
	assert!(guardrails.is_ok(), "{:?}", guardrails.err());
}

#[test]
fn loading_a_project_file_with_a_variant_role_filter_keeps_the_rule() {
	let dir = tempfile::tempdir().unwrap();
	std::fs::create_dir_all(dir.path().join(".agents")).unwrap();
	std::fs::write(
		dir.path().join(FILE_PATH),
		"[[pipe]]\nname = \"p\"\ncommand = \"c.sh\"\nroles = [\"developer:rust\"]\n",
	)
	.unwrap();
	let loaded = Guardrails::load_from_workdir(dir.path());
	assert_eq!(loaded.pipes.len(), 1);
	assert_eq!(loaded.pipes[0].roles, ["developer:rust"]);
}
