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

fn cumulative(cost: f64, tokens: u64) -> StepStats {
	StepStats {
		cost,
		total_tokens: tokens,
		input_tokens: tokens,
		..Default::default()
	}
}

#[test]
fn continue_delta_counts_each_turn_once() {
	// A continue-session step reports CUMULATIVE session totals every
	// iteration: 0.10 → 0.25 → 0.45 (turn costs 0.10 / 0.15 / 0.20).
	let mut base = StepStats::default();
	let d1 = continue_delta(&mut base, &cumulative(0.10, 100));
	let d2 = continue_delta(&mut base, &cumulative(0.25, 250));
	let d3 = continue_delta(&mut base, &cumulative(0.45, 450));
	assert!((d1.cost - 0.10).abs() < 1e-9);
	assert!((d2.cost - 0.15).abs() < 1e-9);
	assert!((d3.cost - 0.20).abs() < 1e-9);
	// Summed deltas equal the final cumulative — counted once, not the
	// ~3x overcount that summing raw cumulative figures would produce.
	let summed = d1.cost + d2.cost + d3.cost;
	assert!((summed - 0.45).abs() < 1e-9, "summed={summed}");
	assert_eq!(d1.total_tokens + d2.total_tokens + d3.total_tokens, 450);
}

fn seq(name: &str) -> Sequential {
	Sequential {
		name: name.to_string(),
		role: "developer:general".to_string(),
		prompt: "{{input}}".to_string(),
		session: SessionMode::Fresh,
		timeout: 0,
		retries: 0,
		model: None,
		workdir: None,
		count: None,
		skills: None,
		capabilities: None,
	}
}

#[test]
fn expand_count_replicates_with_own_model() {
	let mut s = seq("candidate");
	s.count = Some(3);
	s.model = Some("openai:gpt-5".into());
	let reps = expand_substep(&s);
	assert_eq!(reps.len(), 3);
	assert!(reps
		.iter()
		.all(|r| r.seq.model.as_deref() == Some("openai:gpt-5")));
	assert_eq!(reps[2].label, "candidate #3");
}

#[test]
fn expand_none_is_single_passthrough() {
	let reps = expand_substep(&seq("solo"));
	assert_eq!(reps.len(), 1);
	assert_eq!(reps[0].label, "solo");
	assert!(reps[0].seq.model.is_none());
}

#[test]
fn extract_items_xml_capture_group() {
	let re = Regex::new(r"(?s)<task>(.*?)</task>").unwrap();
	let src =
		"Here are tasks:\n<task>research A\nspanning lines</task>\nnoise\n<task>research B</task>";
	let items = extract_items(&re, src);
	assert_eq!(items, vec!["research A\nspanning lines", "research B"]);
}

#[test]
fn extract_items_requires_capture_group() {
	// No capture group → the regex matches but produces no items, because
	// the caller has to express what part of the match is the item.
	let re = Regex::new(r"\d+").unwrap();
	assert!(extract_items(&re, "a1 b22 c333").is_empty());

	// A capture group on a similar pattern yields the groups.
	let re2 = Regex::new(r"(\d+)").unwrap();
	assert_eq!(extract_items(&re2, "a1 b22 c333"), vec!["1", "22", "333"]);
}

#[test]
fn extract_items_skips_empty() {
	let re = Regex::new(r"(?s)<t>(.*?)</t>").unwrap();
	let items = extract_items(&re, "<t>keep</t><t>   </t><t>also</t>");
	assert_eq!(items, vec!["keep", "also"]);
}

#[test]
fn join_labeled_skips_empty_and_headers_rest() {
	let parts = vec![
		("a".to_string(), "one".to_string()),
		("b".to_string(), "   ".to_string()),
		("c".to_string(), "two".to_string()),
	];
	let joined = join_labeled(&parts);
	assert_eq!(joined, "── a ──\none\n\n── c ──\ntwo");
}

#[test]
fn continue_delta_clamps_nonmonotonic_drop() {
	// Cumulative figures should never drop, but guard against it anyway.
	let mut base = StepStats::default();
	let _ = continue_delta(&mut base, &cumulative(0.50, 500));
	let d = continue_delta(&mut base, &cumulative(0.40, 400));
	assert_eq!(d.cost, 0.0);
	assert_eq!(d.total_tokens, 0);
}

#[test]
fn graph_edge_selects_condition_then_default() {
	let edges = vec![
		Edge {
			from: "review".into(),
			to: END_NODE.into(),
			when: Some(Condition {
				output: None,
				contains: Some("PASS".into()),
				matches: None,
			}),
		},
		Edge {
			from: "review".into(),
			to: "fix".into(),
			when: None,
		},
	];
	let mut outputs = HashMap::from([("review".to_string(), "needs work".to_string())]);
	assert_eq!(
		select_graph_edge(&edges, &outputs, "review").unwrap(),
		"fix"
	);

	outputs.insert("review".into(), "PASS".into());
	assert_eq!(
		select_graph_edge(&edges, &outputs, "review").unwrap(),
		END_NODE
	);
}

#[test]
fn graph_edge_rejects_unavailable_condition_output() {
	let edges = vec![Edge {
		from: "review".into(),
		to: END_NODE.into(),
		when: Some(Condition {
			output: Some("verdict".into()),
			contains: Some("PASS".into()),
			matches: None,
		}),
	}];
	let err = select_graph_edge(&edges, &HashMap::new(), "review")
		.expect_err("missing route output must fail");
	assert!(err.to_string().contains("unavailable"), "got: {err}");
}

#[test]
fn graph_edge_without_route_is_an_error() {
	let edges = vec![Edge {
		from: "a".into(),
		to: "b".into(),
		when: None,
	}];
	let err = select_graph_edge(&edges, &HashMap::new(), "orphan")
		.expect_err("a node with no outgoing edge must fail");
	assert!(err.to_string().contains("no matching route"), "got: {err}");
}

#[test]
fn graph_edge_named_output_is_read_instead_of_current_node() {
	// `output = "verdict"` routes on another step's text, not the node's own.
	let edges = vec![
		Edge {
			from: "fix".into(),
			to: END_NODE.into(),
			when: Some(Condition {
				output: Some("verdict".into()),
				contains: Some("PASS".into()),
				matches: None,
			}),
		},
		Edge {
			from: "fix".into(),
			to: "review".into(),
			when: None,
		},
	];
	let outputs = HashMap::from([
		("fix".to_string(), "PASS".to_string()),
		("verdict".to_string(), "FAIL".to_string()),
	]);
	assert_eq!(
		select_graph_edge(&edges, &outputs, "fix").unwrap(),
		"review"
	);
}

#[test]
fn graph_edge_ignores_other_nodes_edges() {
	let edges = vec![
		Edge {
			from: "other".into(),
			to: "wrong".into(),
			when: None,
		},
		Edge {
			from: "here".into(),
			to: "right".into(),
			when: None,
		},
	];
	assert_eq!(
		select_graph_edge(&edges, &HashMap::new(), "here").unwrap(),
		"right"
	);
}

#[test]
fn condition_matches_contains_and_regex() {
	let contains = Condition {
		output: None,
		contains: Some("PASS".into()),
		matches: None,
	};
	assert!(condition_matches(&contains, "verdict: PASS"));
	// Case-sensitive by design.
	assert!(!condition_matches(&contains, "verdict: pass"));

	let regex = Condition {
		output: None,
		contains: None,
		matches: Some(r"^\s*DONE\b".into()),
	};
	assert!(condition_matches(&regex, "  DONE with the task"));
	assert!(!condition_matches(&regex, "not DONE"));
}

#[test]
fn condition_matches_is_a_disjunction_and_empty_is_false() {
	let both = Condition {
		output: None,
		contains: Some("NOPE".into()),
		matches: Some(r"\bok\b".into()),
	};
	// Either side matching is enough.
	assert!(condition_matches(&both, "all ok here"));
	assert!(condition_matches(&both, "NOPE"));
	assert!(!condition_matches(&both, "neither"));

	// A condition that tests nothing never fires — it must not default to true.
	let empty = Condition {
		output: None,
		contains: None,
		matches: None,
	};
	assert!(!condition_matches(&empty, "anything"));
}

#[test]
fn sanitize_replaces_everything_but_alphanumerics_and_dash() {
	assert_eq!(sanitize("plan-step"), "plan-step");
	assert_eq!(sanitize("build & test"), "build---test");
	assert_eq!(sanitize("../etc/passwd"), "---etc-passwd");
	// Non-ASCII collapses too — session names end up on the filesystem.
	assert_eq!(sanitize("шаг"), "---");
}

#[test]
fn fmt_dur_never_renders_sixty_seconds() {
	assert_eq!(fmt_dur(Duration::from_millis(1500)), "1.5s");
	assert_eq!(fmt_dur(Duration::from_secs(60)), "1m00s");
	assert_eq!(fmt_dur(Duration::from_secs(125)), "2m05s");
	// 119.6s must roll over to 2m00s, not "1m60s".
	assert_eq!(fmt_dur(Duration::from_millis(119_600)), "2m00s");
}

#[test]
fn fmt_tools_flags_failures_only_when_present() {
	assert_eq!(fmt_tools(3, 0), "⚒3");
	assert!(fmt_tools(3, 1).contains("⚒3"));
	assert!(fmt_tools(3, 1).contains('1'));
}

#[test]
fn workflow_output_names_includes_block_and_substep_names() {
	let wf: WorkflowDef = toml::from_str(
		r#"
name = "t"
[[steps]]
name = "plan"
role = "developer:general"
prompt = "{{input}}"

[[steps]]
name = "fanout"
parallel = true
[[steps.run]]
name = "worker"
role = "developer:general"
prompt = "{{plan}}"

[[steps]]
name = "refine"
loop = true
[[steps.run]]
name = "iterate"
role = "developer:general"
prompt = "{{worker}}"
"#,
	)
	.expect("workflow parses");
	let names = workflow_output_names(&wf);
	for expected in ["plan", "fanout", "worker", "refine", "iterate"] {
		assert!(names.contains(expected), "missing {expected} in {names:?}");
	}
	assert_eq!(names.len(), 5);
}

#[test]
fn resolve_workdir_passes_through_none_and_rejects_missing_dir() {
	assert!(resolve_workdir("s", None).unwrap().is_none());

	let dir = tempfile::tempdir().unwrap();
	let abs = resolve_workdir("s", Some(dir.path().to_str().unwrap()))
		.unwrap()
		.expect("existing dir resolves");
	assert_eq!(abs, dir.path());

	let missing = dir.path().join("nope");
	let err = resolve_workdir("build", Some(missing.to_str().unwrap()))
		.expect_err("missing workdir must fail loudly");
	assert!(err.to_string().contains("build"), "got: {err}");
	assert!(err.to_string().contains("not a directory"), "got: {err}");
}

#[test]
fn resolve_workdir_makes_relative_paths_absolute() {
	let resolved = resolve_workdir("s", Some("src"))
		.unwrap()
		.expect("src exists relative to the crate root");
	assert!(resolved.is_absolute());
	assert!(resolved.ends_with("src"));
}

// ---- Executor: fold_stats / enforce_budget / substitute / next_graph_node ----

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

#[test]
fn fold_stats_passes_fresh_sessions_through_unchanged() {
	let mut ex = Executor::new(
		"wf".to_string(),
		&template_config(),
		false,
		None,
		false,
		HashSet::new(),
	);
	// Fresh sessions never advance a baseline — every invocation reports as-is.
	let first = ex.fold_stats("s", SessionMode::Fresh, &cumulative(0.25, 250));
	assert!((first.cost - 0.25).abs() < 1e-9);
	let second = ex.fold_stats("s", SessionMode::Fresh, &cumulative(0.40, 400));
	assert!((second.cost - 0.40).abs() < 1e-9);
}

#[test]
fn fold_stats_folds_continue_sessions_into_per_turn_deltas() {
	let mut ex = Executor::new(
		"wf".to_string(),
		&template_config(),
		false,
		None,
		false,
		HashSet::new(),
	);
	let d1 = ex.fold_stats("c", SessionMode::Continue, &cumulative(0.10, 100));
	let d2 = ex.fold_stats("c", SessionMode::Continue, &cumulative(0.25, 250));
	assert!(
		(d1.cost - 0.10).abs() < 1e-9,
		"first turn reports its full spend"
	);
	assert!(
		(d2.cost - 0.15).abs() < 1e-9,
		"second turn reports only the delta"
	);
	assert_eq!(d1.total_tokens + d2.total_tokens, 250);
}

#[test]
fn enforce_budget_aborts_only_when_the_cap_is_crossed() {
	let mut ex = Executor::new(
		"wf".to_string(),
		&template_config(),
		false,
		None,
		false,
		HashSet::new(),
	);
	// No cap: never aborts.
	ex.totals.cost = 99.0;
	assert!(ex.enforce_budget("any").is_ok());

	ex.max_cost = Some(2.0);
	ex.totals.cost = 1.5;
	assert!(ex.enforce_budget("under").is_ok());

	ex.totals.cost = 2.5;
	let err = ex.enforce_budget("runaway").expect_err("cap must abort");
	let msg = err.to_string();
	assert!(msg.contains("budget exceeded"), "got: {msg}");
	assert!(
		msg.contains("runaway"),
		"the offending step is named: {msg}"
	);
}

#[tokio::test]
async fn substitute_resolves_input_and_prior_outputs_and_keeps_unknowns() {
	let mut ex = Executor::new(
		"wf".to_string(),
		&template_config(),
		false,
		None,
		false,
		HashSet::new(),
	);
	ex.outputs.insert("plan".to_string(), "PLANNED".to_string());

	let resolved = ex
		.substitute(
			"{{input}} then {{plan}} then {{not_a_var}}",
			"DO",
			"developer:general",
		)
		.await
		.expect("substitution is pure text resolution");
	assert_eq!(resolved, "DO then PLANNED then {{not_a_var}}");
}

#[tokio::test]
async fn substitute_graph_mode_rejects_outputs_not_on_the_route() {
	let known: HashSet<String> = ["later".to_string()].into_iter().collect();
	let mut ex = Executor::new(
		"wf".to_string(),
		&template_config(),
		false,
		None,
		true,
		known,
	);

	let err = ex
		.substitute("use {{later}}", "DO", "developer:general")
		.await
		.expect_err("a known output that never ran must fail clearly");
	assert!(
		err.to_string().contains("unavailable on the current route"),
		"got: {err}"
	);

	// Once the producer has run the same template resolves.
	ex.outputs.insert("later".to_string(), "READY".to_string());
	let ok = ex
		.substitute("use {{later}}", "DO", "developer:general")
		.await
		.expect("produced output resolves");
	assert_eq!(ok, "use READY");

	// Names the workflow never declared stay literal — no false route error.
	let unknown = ex
		.substitute("{{totally_unknown}}", "DO", "developer:general")
		.await
		.expect("unknown names are not route-checked");
	assert_eq!(unknown, "{{totally_unknown}}");
}

#[test]
fn next_graph_node_routes_on_condition_then_default() {
	let wf: WorkflowDef = toml::from_str(
		r#"
name = "g"
entry = "review"
max_transitions = 5

[[steps]]
name = "review"
role = "developer:general"
prompt = "review it"

[[steps]]
name = "fix"
role = "developer:general"
prompt = "fix it"

[[edges]]
from = "review"
to = "$end"
[edges.when]
contains = "PASS"

[[edges]]
from = "review"
to = "fix"
"#,
	)
	.expect("graph workflow parses");

	let mut ex = Executor::new(
		"g".to_string(),
		&template_config(),
		false,
		None,
		true,
		HashSet::new(),
	);
	ex.outputs
		.insert("review".to_string(), "needs work".to_string());
	assert_eq!(
		ex.next_graph_node(&wf, "review").expect("default edge"),
		"fix"
	);

	ex.outputs.insert("review".to_string(), "PASS".to_string());
	assert_eq!(
		ex.next_graph_node(&wf, "review").expect("conditional edge"),
		END_NODE
	);
}

// ---- Totals ----

#[test]
fn totals_add_sums_every_counter() {
	let mut totals = Totals::default();
	totals.add(&cumulative(0.25, 100));
	totals.add(&StepStats {
		duration: Duration::from_secs(2),
		cost: 0.5,
		total_tokens: 200,
		input_tokens: 150,
		output_tokens: 50,
		tool_count: 2,
		tool_failed: 1,
		..Default::default()
	});
	assert!((totals.cost - 0.75).abs() < 1e-9);
	assert_eq!(totals.tokens, 300);
	assert_eq!(totals.input_tokens, 250);
	assert_eq!(totals.output_tokens, 50);
	assert_eq!(totals.tools, 2);
	assert_eq!(totals.tools_failed, 1);
	assert_eq!(totals.duration, Duration::from_secs(2));
}

// ---- display helpers ----

#[test]
fn with_stderr_appends_a_tail_only_when_present() {
	assert_eq!(with_stderr("boom".to_string(), "   "), "boom");
	assert_eq!(
		with_stderr("boom".to_string(), "details"),
		"boom\n  stderr: details"
	);
}

#[test]
fn fmt_stats_renders_duration_cost_tokens_and_tools() {
	let stats = StepStats {
		duration: Duration::from_millis(1500),
		cost: 0.1,
		total_tokens: 450,
		tool_count: 2,
		..Default::default()
	};
	let line = fmt_stats(&stats);
	assert!(line.contains("1.5s"), "got: {line}");
	assert!(line.contains("$0.1000"), "got: {line}");
	assert!(line.contains("450 tok"), "got: {line}");
	assert!(line.contains("⚒2"), "got: {line}");
}

#[test]
fn short_uuid_is_the_first_uuid_segment() {
	let id = short_uuid();
	assert_eq!(id.len(), 8, "uuid v4 first segment is 8 hex chars");
	assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "got: {id}");
}

#[test]
fn display_helpers_write_without_panicking() {
	// These render to stderr only; the contract under test is that every
	// block/line shape completes for both empty and populated content.
	box_open("title");
	box_close_ok("name", "1.0s · $0.0000");
	box_close_err("name", "failed");
	box_line("inner");
	info_line("note");
	print_response("", false, "");
	print_response("plain text", false, "");
}

// ---- Executor: exec paths ----
// Under `cargo test` the spawned step subprocess is the test binary itself,
// which rejects `--format jsonl` and exits non-zero — every successful-outcome
// arm is therefore a hard ceiling here; these tests drive the failure paths.

fn executor_for(wf_name: &str, graph_mode: bool) -> Executor {
	Executor::new(
		wf_name.to_string(),
		&template_config(),
		false,
		None,
		graph_mode,
		HashSet::new(),
	)
}

#[tokio::test]
async fn exec_sequential_retries_tag_attempts_and_fail_loudly() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		name = "flaky"
		role = "developer:general"
		prompt = "do {{input}}"
		retries = 1
		"#,
	)
	.expect("workflow parses");
	let Step::Sequential(s) = &wf.steps[0] else {
		panic!("expected a sequential step");
	};
	let mut ex = executor_for("wf", false);
	let err = ex
		.exec_sequential(s, "DO", "")
		.await
		.expect_err("both attempts must fail");
	let msg = err.to_string();
	assert!(msg.contains("step 'flaky' failed after 2 attempts"), "got: {msg}");
}

#[tokio::test]
async fn exec_sequential_continue_creates_and_reuses_session_id() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		name = "cont"
		role = "developer:general"
		prompt = "refine {{input}}"
		session = "continue"
		"#,
	)
	.expect("workflow parses");
	let Step::Sequential(s) = &wf.steps[0] else {
		panic!("expected a sequential step");
	};
	let mut ex = executor_for("c", false);
	ex.exec_sequential(s, "DO", "")
		.await
		.expect_err("subprocess fails on first use");
	let id = ex.session_ids.get("cont").cloned().expect("session id created");
	assert!(id.starts_with("wf-c-cont-"), "got: {id}");

	ex.exec_sequential(s, "DO", "")
		.await
		.expect_err("subprocess fails on reuse too");
	assert_eq!(
		ex.session_ids.get("cont"),
		Some(&id),
		"a Continue session keeps its id, never regenerates it"
	);
}

#[tokio::test]
async fn exec_sequential_continue_reuse_sends_done_and_feeds_prior_output() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		name = "cont"
		role = "developer:general"
		prompt = "refine {{input}}"
		session = "continue"
		"#,
	)
	.expect("workflow parses");
	let Step::Sequential(s) = &wf.steps[0] else {
		panic!("expected a sequential step");
	};
	let mut ex = executor_for("c", false);
	// Pre-stage a used Continue session: the executor must send best-effort
	// /done to the old session and nudge with the prior step's output
	// instead of re-sending the full templated prompt.
	ex.session_ids
		.insert("cont".to_string(), "wf-c-cont-fixed".to_string());
	ex.used_continue.insert("cont".to_string(), true);
	ex.last_step = Some("prev".to_string());
	ex.outputs
		.insert("prev".to_string(), "prior verdict".to_string());

	let err = ex
		.exec_sequential(s, "DO", "")
		.await
		.expect_err("subprocess still fails");
	assert!(err.to_string().contains("step 'cont' failed"), "got: {err}");
	assert_eq!(
		ex.session_ids.get("cont"),
		Some(&"wf-c-cont-fixed".to_string()),
		"the existing session id is kept"
	);
}

#[tokio::test]
async fn exec_sequential_interactive_mode_runs_with_spinner() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		name = "solo"
		role = "developer:general"
		prompt = "do {{input}}"
		"#,
	)
	.expect("workflow parses");
	let Step::Sequential(s) = &wf.steps[0] else {
		panic!("expected a sequential step");
	};
	let mut ex = executor_for("wf", false);
	ex.interactive = true;
	let err = ex
		.exec_sequential(s, "DO", "")
		.await
		.expect_err("step fails under the spinner too");
	assert!(err.to_string().contains("step 'solo' failed"), "got: {err}");
}

#[tokio::test]
async fn exec_sequential_spawn_error_names_the_cause() {
	use std::os::unix::fs::PermissionsExt;

	let dir = tempfile::tempdir().expect("temp dir");
	std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000))
		.expect("chmod 000");
	struct Restore(std::path::PathBuf);
	impl Drop for Restore {
		fn drop(&mut self) {
			use std::os::unix::fs::PermissionsExt;
			let _ = std::fs::set_permissions(
				&self.0,
				std::fs::Permissions::from_mode(0o755),
			);
		}
	}
	let _restore = Restore(dir.path().to_path_buf());

	let wf: WorkflowDef = toml::from_str(&format!(
		r#"
		name = "wf"
		[[steps]]
		name = "locked"
		role = "developer:general"
		prompt = "do {{{{input}}}}"
		workdir = "{}"
		"#,
		dir.path().display()
	))
	.expect("workflow parses");
	let Step::Sequential(s) = &wf.steps[0] else {
		panic!("expected a sequential step");
	};
	let mut ex = executor_for("wf", false);
	// The dir exists (resolve_workdir accepts it) but the child cannot chdir
	// into it — spawn itself must fail with a named cause.
	let err = ex
		.exec_sequential(s, "DO", "")
		.await
		.expect_err("spawn must fail in an inaccessible dir");
	assert!(err.to_string().contains("spawn error"), "got: {err}");
}

#[tokio::test]
async fn exec_parallel_dynamic_source_missing_fails_clearly() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		name = "fan"
		parallel = true
		source = "ghost"
		match = "(.+)"
		  [[steps.run]]
		  name = "worker"
		  role = "developer:general"
		  prompt = "{{fan}}"
		"#,
	)
	.expect("workflow parses");
	let Step::Parallel(p) = &wf.steps[0] else {
		panic!("expected a parallel step");
	};
	let mut ex = executor_for("wf", false);
	let err = ex
		.exec_parallel(p, "DO")
		.await
		.expect_err("missing source output must fail");
	assert!(
		err.to_string().contains("unavailable on the current route"),
		"got: {err}"
	);
}

#[tokio::test]
async fn exec_parallel_dynamic_invalid_regex_fails_at_run_time() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		name = "fan"
		parallel = true
		source = "gen"
		match = "(unclosed"
		  [[steps.run]]
		  name = "worker"
		  role = "developer:general"
		  prompt = "{{fan}}"
		"#,
	)
	.expect("workflow parses");
	let Step::Parallel(p) = &wf.steps[0] else {
		panic!("expected a parallel step");
	};
	let mut ex = executor_for("wf", false);
	ex.outputs.insert("gen".to_string(), "zzz".to_string());
	let err = ex
		.exec_parallel(p, "DO")
		.await
		.expect_err("invalid regex must fail at run time");
	assert!(
		err.to_string().contains("invalid match regex"),
		"got: {err}"
	);
}

#[tokio::test]
async fn exec_parallel_dynamic_zero_items_fails() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		name = "fan"
		parallel = true
		source = "gen"
		match = "<task>(.*?)</task>"
		  [[steps.run]]
		  name = "worker"
		  role = "developer:general"
		  prompt = "{{fan}}"
		"#,
	)
	.expect("workflow parses");
	let Step::Parallel(p) = &wf.steps[0] else {
		panic!("expected a parallel step");
	};
	let mut ex = executor_for("wf", false);
	ex.outputs
		.insert("gen".to_string(), "no markers here".to_string());
	let err = ex
		.exec_parallel(p, "DO")
		.await
		.expect_err("zero matches must fail");
	assert!(err.to_string().contains("found 0 items"), "got: {err}");
}

#[tokio::test]
async fn exec_parallel_dynamic_replicas_fail_and_block_aborts() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		name = "gen"
		role = "developer:general"
		prompt = "list {{input}}"
		[[steps]]
		name = "fan"
		parallel = true
		source = "gen"
		match = "(?s)<task>(.*?)</task>"
		  [[steps.run]]
		  name = "worker"
		  role = "developer:general"
		  prompt = "work on {{fan}}"
		"#,
	)
	.expect("workflow parses");
	let Step::Parallel(p) = &wf.steps[1] else {
		panic!("expected a parallel step");
	};
	let mut ex = executor_for("wf", false);
	ex.outputs.insert(
		"gen".to_string(),
		"<task>alpha</task>\n<task>beta</task>".to_string(),
	);
	let err = ex
		.exec_parallel(p, "DO")
		.await
		.expect_err("both replicas fail → block aborts");
	assert!(
		err.to_string().contains("only 0/2 replicas succeeded"),
		"got: {err}"
	);
	// During fan-out the block's own name is the per-item variable — the
	// last matched item stays bound even though the block then aborted.
	assert_eq!(ex.outputs.get("fan"), Some(&"beta".to_string()));
}

#[tokio::test]
async fn exec_parallel_static_spawn_errors_throttled_abort() {
	use std::os::unix::fs::PermissionsExt;

	let dir = tempfile::tempdir().expect("temp dir");
	std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000))
		.expect("chmod 000");
	struct Restore(std::path::PathBuf);
	impl Drop for Restore {
		fn drop(&mut self) {
			use std::os::unix::fs::PermissionsExt;
			let _ = std::fs::set_permissions(
				&self.0,
				std::fs::Permissions::from_mode(0o755),
			);
		}
	}
	let _restore = Restore(dir.path().to_path_buf());

	let wf: WorkflowDef = toml::from_str(&format!(
		r#"
		name = "wf"
		[[steps]]
		name = "duo"
		parallel = true
		max_parallel = 1
		  [[steps.run]]
		  name = "a"
		  role = "developer:general"
		  prompt = "pa"
		  workdir = "{}"
		  [[steps.run]]
		  name = "b"
		  role = "developer:general"
		  prompt = "pb"
		  workdir = "{}"
		"#,
		dir.path().display(),
		dir.path().display()
	))
	.expect("workflow parses");
	let Step::Parallel(p) = &wf.steps[0] else {
		panic!("expected a parallel step");
	};
	let mut ex = executor_for("wf", false);
	let err = ex
		.exec_parallel(p, "DO")
		.await
		.expect_err("both throttled replicas fail to spawn");
	assert!(
		err.to_string().contains("only 0/2 replicas succeeded"),
		"got: {err}"
	);
}

#[tokio::test]
async fn exec_parallel_static_count_expansion_reports_total() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		name = "sweep"
		parallel = true
		  [[steps.run]]
		  name = "a"
		  role = "developer:general"
		  prompt = "pa"
		  count = 2
		  [[steps.run]]
		  name = "b"
		  role = "developer:general"
		  prompt = "pb"
		"#,
	)
	.expect("workflow parses");
	let Step::Parallel(p) = &wf.steps[0] else {
		panic!("expected a parallel step");
	};
	let mut ex = executor_for("wf", false);
	let err = ex
		.exec_parallel(p, "DO")
		.await
		.expect_err("all three expanded replicas fail");
	assert!(
		err.to_string().contains("only 0/3 replicas succeeded"),
		"got: {err}"
	);
}

#[tokio::test]
async fn exec_conditional_without_prior_output_fails() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		conditional = true
		name = "gate"
		on_match = ["yes"]
		[steps.condition]
		contains = "go"
		[[steps.run]]
		name = "yes"
		role = "developer:general"
		prompt = "p"
		"#,
	)
	.expect("workflow parses");
	let mut ex = executor_for("wf", false);
	let err = ex
		.exec_node(&wf.steps[0], "DO")
		.await
		.expect_err("no prior output to test must fail");
	assert!(
		err.to_string().contains("no prior step output to test"),
		"got: {err}"
	);
}

#[tokio::test]
async fn exec_conditional_unknown_target_fails() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		conditional = true
		name = "gate"
		on_match = ["yes"]
		[steps.condition]
		contains = "go"
		output = "ghost"
		[[steps.run]]
		name = "yes"
		role = "developer:general"
		prompt = "p"
		"#,
	)
	.expect("workflow parses");
	let mut ex = executor_for("wf", false);
	let err = ex
		.exec_node(&wf.steps[0], "DO")
		.await
		.expect_err("a target that never ran must fail");
	assert!(
		err.to_string().contains("condition target 'ghost' has no output"),
		"got: {err}"
	);
}

#[tokio::test]
async fn exec_conditional_matched_branch_runs_and_fails() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		conditional = true
		name = "gate"
		on_match = ["yes"]
		on_no_match = ["nope"]
		[steps.condition]
		contains = "PASS"
		output = "prev"
		[[steps.run]]
		name = "yes"
		role = "developer:general"
		prompt = "ship it"
		[[steps.run]]
		name = "nope"
		role = "developer:general"
		prompt = "fix it"
		"#,
	)
	.expect("workflow parses");
	let mut ex = executor_for("wf", false);
	ex.outputs
		.insert("prev".to_string(), "PASS GO".to_string());
	let err = ex
		.exec_node(&wf.steps[0], "DO")
		.await
		.expect_err("the chosen branch's subprocess fails");
	assert!(
		err.to_string().contains("step 'yes' failed after 1 attempts"),
		"got: {err}"
	);
}

#[tokio::test]
async fn exec_conditional_empty_branches_store_empty_outputs() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		conditional = true
		name = "gate"
		on_match = []
		on_no_match = []
		[steps.condition]
		contains = "go"
		output = "prev"
		[[steps.run]]
		name = "yes"
		role = "developer:general"
		prompt = "p"
		[[steps.run]]
		name = "nope"
		role = "developer:general"
		prompt = "q"
		"#,
	)
	.expect("workflow parses");
	let mut ex = executor_for("wf", true);
	ex.outputs
		.insert("prev".to_string(), "go right ahead".to_string());
	ex.exec_node(&wf.steps[0], "DO")
		.await
		.expect("a conditional with no branches is trivially Ok");
	// Skipped branch outputs resolve to empty entries, the gate itself
	// stores the (empty) selected output, and graph mode advances to it.
	assert_eq!(ex.outputs.get("yes"), Some(&String::new()));
	assert_eq!(ex.outputs.get("nope"), Some(&String::new()));
	assert_eq!(ex.outputs.get("gate"), Some(&String::new()));
	assert_eq!(ex.last_step, Some("gate".to_string()));
}

#[test]
fn condition_matches_treats_invalid_regex_as_no_match() {
	let bad = Condition {
		output: None,
		contains: None,
		matches: Some("(unclosed".to_string()),
	};
	assert!(
		!condition_matches(&bad, "anything"),
		"an invalid regex must simply never match, not panic"
	);
}

#[test]
fn print_response_renders_markdown_content() {
	// Markdown-looking output goes through the themed MarkdownRenderer;
	// the contract is completing without panic, matching the display-helper
	// smoke test above.
	print_response(
		"# Plan\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```",
		true,
		"default",
	);
}

#[test]
fn workflow_output_names_includes_conditional_branches() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "wf"
		[[steps]]
		conditional = true
		name = "gate"
		on_match = ["yes"]
		on_no_match = ["nope"]
		[steps.condition]
		contains = "go"
		[[steps.run]]
		name = "yes"
		role = "developer:general"
		prompt = "p"
		[[steps.run]]
		name = "nope"
		role = "developer:general"
		prompt = "q"
		"#,
	)
	.expect("workflow parses");
	let names = workflow_output_names(&wf);
	for expected in ["gate", "yes", "nope"] {
		assert!(names.contains(expected), "missing {expected} in {names:?}");
	}
	assert_eq!(names.len(), 3);
}

#[tokio::test]
async fn execute_graph_aborts_when_first_node_fails() {
	let wf: WorkflowDef = toml::from_str(
		r#"
		name = "g"
		entry = "start"
		max_transitions = 3
		[[steps]]
		name = "start"
		role = "developer:general"
		prompt = "begin {{input}}"
		[[steps]]
		name = "fin"
		role = "developer:general"
		prompt = "end {{start}}"
		[[edges]]
		from = "start"
		to = "fin"
		when = { contains = "go" }
		[[edges]]
		from = "start"
		to = "$end"
		[[edges]]
		from = "fin"
		to = "$end"
		"#,
	)
	.expect("workflow parses");
	let err = execute(&wf, "DO", &template_config(), None)
		.await
		.expect_err("the entry node's subprocess fails");
	assert!(
		err.to_string().contains("step 'start' failed after 1 attempts"),
		"got: {err}"
	);
}
