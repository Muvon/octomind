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
