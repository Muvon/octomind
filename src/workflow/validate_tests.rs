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

fn parse(toml_src: &str) -> WorkflowDef {
	toml::from_str(toml_src).expect("valid TOML")
}

#[test]
fn builtin_placeholders_pass_validation() {
	// Built-ins are expanded at run time, not step outputs — they must not
	// be rejected as unknown variables.
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "s1"
			role = "developer:general"
			prompt = "Today is {{DATE}} in {{CWD}}. Context:\n{{CONTEXT}}\n\nRequest: {{input}}"
			"#,
	);
	validate(&wf).expect("built-in placeholders should validate");
}

#[test]
fn genuinely_unknown_variable_still_fails() {
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "s1"
			role = "developer:general"
			prompt = "Hello {{nope}}"
			"#,
	);
	let err = validate(&wf).expect_err("unknown variable must fail");
	assert!(err.to_string().contains("nope"), "got: {err}");
}

#[test]
fn max_cost_must_be_positive() {
	let wf = parse(
		r#"
			name = "wf"
			max_cost = 0.0
			[[steps]]
			name = "s1"
			role = "developer:general"
			prompt = "{{input}}"
			"#,
	);
	assert!(validate(&wf).is_err(), "zero max_cost must fail");

	let ok = parse(
		r#"
			name = "wf"
			max_cost = 1.5
			[[steps]]
			name = "s1"
			role = "developer:general"
			prompt = "{{input}}"
			"#,
	);
	validate(&ok).expect("positive max_cost should pass");
}

#[test]
fn count_sweep_in_parallel_validates() {
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "candidates"
			parallel = true
			min_success = 2
			  [[steps.run]]
			  name = "candidate"
			  role = "developer:general"
			  prompt = "{{input}}"
			  count = 3
			  [[steps.run]]
			  name = "other"
			  role = "developer:general"
			  prompt = "{{input}}"
			"#,
	);
	validate(&wf).expect("count sweep + min_success in range should pass");
}

#[test]
fn expansion_fields_rejected_outside_parallel() {
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "s1"
			role = "developer:general"
			prompt = "{{input}}"
			count = 3
			"#,
	);
	let err = validate(&wf).expect_err("count on a sequential step must fail");
	assert!(
		err.to_string().contains("only valid on parallel"),
		"got: {err}"
	);
}

#[test]
fn count_below_two_fails() {
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "p"
			parallel = true
			  [[steps.run]]
			  name = "a"
			  role = "developer:general"
			  prompt = "{{input}}"
			  count = 1
			  [[steps.run]]
			  name = "b"
			  role = "developer:general"
			  prompt = "{{input}}"
			"#,
	);
	assert!(validate(&wf).is_err(), "count = 1 must fail");
}

#[test]
fn min_success_out_of_range_fails() {
	// One sub-step with count = 2 + one plain sub-step = 3 total replicas.
	// min_success = 4 exceeds that.
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "p"
			parallel = true
			min_success = 4
			  [[steps.run]]
			  name = "a"
			  role = "developer:general"
			  prompt = "{{input}}"
			  count = 2
			  [[steps.run]]
			  name = "b"
			  role = "developer:general"
			  prompt = "{{input}}"
			"#,
	);
	let err = validate(&wf).expect_err("min_success > total replicas must fail");
	assert!(err.to_string().contains("min_success"), "got: {err}");
}

#[test]
fn dynamic_parallel_validates() {
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "plan"
			role = "researcher:general"
			prompt = "List tasks in <task>..</task>:\n{{input}}"
			[[steps]]
			name = "research"
			parallel = true
			source = "plan"
			match = "(?s)<task>(.*?)</task>"
			max_parallel = 4
			min_success = 1
			  [[steps.run]]
			  name = "researcher"
			  role = "researcher:general"
			  prompt = "Research:\n{{research}}"
			[[steps]]
			name = "summary"
			role = "developer:general"
			prompt = "Summarize:\n{{researcher}}"
			"#,
	);
	validate(&wf).expect("dynamic parallel referencing its own name in the template should pass");
}

#[test]
fn dynamic_parallel_requires_single_template() {
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "plan"
			role = "researcher:general"
			prompt = "{{input}}"
			[[steps]]
			name = "research"
			parallel = true
			source = "plan"
			match = "(.+)"
			  [[steps.run]]
			  name = "a"
			  role = "researcher:general"
			  prompt = "{{research}}"
			  [[steps.run]]
			  name = "b"
			  role = "researcher:general"
			  prompt = "{{research}}"
			"#,
	);
	let err = validate(&wf).expect_err("dynamic parallel with 2 sub-steps must fail");
	assert!(err.to_string().contains("exactly 1 sub-step"), "got: {err}");
}

#[test]
fn dynamic_parallel_requires_source() {
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "research"
			parallel = true
			match = "(.+)"
			  [[steps.run]]
			  name = "researcher"
			  role = "researcher:general"
			  prompt = "Research the item"
			"#,
	);
	let err = validate(&wf).expect_err("dynamic parallel without source must fail");
	assert!(err.to_string().contains("requires source"), "got: {err}");
}

#[test]
fn dynamic_parallel_rejects_its_own_output_as_source() {
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "research"
			parallel = true
			source = "research"
			match = "(.+)"
			  [[steps.run]]
			  name = "researcher"
			  role = "researcher:general"
			  prompt = "{{research}}"
			"#,
	);
	let err = validate(&wf).expect_err("dynamic source must come from another node");
	assert!(err.to_string().contains("outside the block"), "got: {err}");
}

#[test]
fn dynamic_parallel_invalid_regex_fails() {
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "plan"
			role = "researcher:general"
			prompt = "{{input}}"
			[[steps]]
			name = "research"
			parallel = true
			source = "plan"
			match = "(unclosed"
			  [[steps.run]]
			  name = "researcher"
			  role = "researcher:general"
			  prompt = "Research:\n{{research}}"
			"#,
	);
	let err = validate(&wf).expect_err("invalid match regex must fail");
	assert!(
		err.to_string().contains("invalid match regex"),
		"got: {err}"
	);
}

#[test]
fn parallel_block_name_reference_resolves() {
	// The parallel block's own name is referenceable downstream (it now
	// aggregates every sub-step's output at runtime).
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "candidates"
			parallel = true
			  [[steps.run]]
			  name = "a"
			  role = "developer:general"
			  prompt = "{{input}}"
			  [[steps.run]]
			  name = "b"
			  role = "developer:general"
			  prompt = "{{input}}"
			[[steps]]
			name = "judge"
			role = "developer:general"
			prompt = "Pick best:\n{{candidates}}"
			"#,
	);
	validate(&wf).expect("reference to parallel block name should validate");
}

#[test]
fn step_output_reference_resolves() {
	let wf = parse(
		r#"
			name = "wf"
			[[steps]]
			name = "spec"
			role = "developer:general"
			prompt = "{{input}}"
			[[steps]]
			name = "build"
			role = "developer:general"
			prompt = "Build {{spec}} on {{DATE}}"
			"#,
	);
	validate(&wf).expect("forward-valid step reference + built-in should pass");
}

#[test]
fn bounded_graph_with_cycle_validates() {
	let wf = parse(
		r#"
			name = "graph"
			entry = "plan"
			max_transitions = 12

			[[steps]]
			name = "plan"
			role = "developer:general"
			prompt = "{{input}}"

			[[steps]]
			name = "review"
			role = "developer:general"
			prompt = "Review {{plan}}"

			[[steps]]
			name = "fix"
			role = "developer:general"
			prompt = "Fix {{review}}"

			[[edges]]
			from = "plan"
			to = "review"

			[[edges]]
			from = "review"
			to = "$end"
			when = { contains = "PASS" }

			[[edges]]
			from = "review"
			to = "fix"

			[[edges]]
			from = "fix"
			to = "review"
			"#,
	);
	validate(&wf).expect("explicit bounded graph should validate");
}

#[test]
fn graph_requires_explicit_transition_bound() {
	let wf = parse(
		r#"
			name = "graph"
			entry = "only"
			[[steps]]
			name = "only"
			role = "developer:general"
			prompt = "{{input}}"
			[[edges]]
			from = "only"
			to = "$end"
			"#,
	);
	let err = validate(&wf).expect_err("graph must declare max_transitions");
	assert!(err.to_string().contains("max_transitions"), "got: {err}");
}

#[test]
fn graph_requires_last_unconditional_edge() {
	let wf = parse(
		r#"
			name = "graph"
			entry = "only"
			max_transitions = 2
			[[steps]]
			name = "only"
			role = "developer:general"
			prompt = "{{input}}"
			[[edges]]
			from = "only"
			to = "$end"
			[[edges]]
			from = "only"
			to = "$end"
			when = { contains = "PASS" }
			"#,
	);
	let err = validate(&wf).expect_err("default route must be last");
	assert!(err.to_string().contains("declared last"), "got: {err}");
}

#[test]
fn graph_dynamic_parallel_requires_named_source() {
	let wf = parse(
		r#"
			name = "graph"
			entry = "fanout"
			max_transitions = 2
			[[steps]]
			name = "fanout"
			parallel = true
			match = "<task>(.*?)</task>"
			  [[steps.run]]
			  name = "worker"
			  role = "developer:general"
			  prompt = "{{fanout}}"
			[[edges]]
			from = "fanout"
			to = "$end"
			"#,
	);
	let err = validate(&wf).expect_err("graph fan-out source must be explicit");
	assert!(err.to_string().contains("requires source"), "got: {err}");
}

#[test]
fn graph_dynamic_parallel_uses_named_source_independent_of_declaration_order() {
	let wf = parse(
		r#"
			name = "graph"
			entry = "plan"
			max_transitions = 3
			[[steps]]
			name = "fanout"
			parallel = true
			source = "plan"
			match = "<task>(.*?)</task>"
			  [[steps.run]]
			  name = "worker"
			  role = "developer:general"
			  prompt = "{{fanout}}"
			[[steps]]
			name = "plan"
			role = "developer:general"
			prompt = "{{input}}"
			[[edges]]
			from = "plan"
			to = "fanout"
			[[edges]]
			from = "fanout"
			to = "$end"
			"#,
	);
	validate(&wf).expect("named source should make declaration order irrelevant");
}

#[test]
fn graph_template_validates() {
	let wf = parse(include_str!("../../config-templates/workflow-graph.toml"));
	validate(&wf).expect("shipped graph template should validate");
}

#[test]
fn basic_template_validates() {
	let wf = parse(include_str!("../../config-templates/workflow.toml"));
	validate(&wf).expect("shipped basic template should validate");
}

#[test]
fn research_template_validates() {
	let wf = parse(include_str!(
		"../../config-templates/workflow-research.toml"
	));
	validate(&wf).expect("shipped research template should validate");
}

#[test]
fn fanout_template_validates() {
	let wf = parse(include_str!("../../config-templates/workflow-fanout.toml"));
	validate(&wf).expect("shipped fan-out template should validate");
}

// ── top-level guards ────────────────────────────────────────────────────────

#[test]
fn max_transitions_zero_is_rejected() {
	let wf = parse(
		r#"
		name = "wf"
		max_transitions = 0
		[[steps]]
		name = "s1"
		role = "developer:general"
		prompt = "{{input}}"
		"#,
	);
	let err = validate(&wf).expect_err("max_transitions = 0 must fail");
	assert!(
		err.to_string().contains("max_transitions must be >= 1"),
		"got: {err}"
	);
}

#[test]
fn max_transitions_requires_graph_mode() {
	let wf = parse(
		r#"
		name = "wf"
		max_transitions = 5
		[[steps]]
		name = "s1"
		role = "developer:general"
		prompt = "{{input}}"
		"#,
	);
	let err = validate(&wf).expect_err("max_transitions outside graph mode must fail");
	assert!(err.to_string().contains("requires graph mode"), "got: {err}");
}

// ── graph validation ────────────────────────────────────────────────────────

#[test]
fn graph_without_edges_is_rejected() {
	let wf = parse(
		r#"
		name = "g"
		entry = "only"
		max_transitions = 2
		[[steps]]
		name = "only"
		role = "developer:general"
		prompt = "{{input}}"
		"#,
	);
	let err = validate(&wf).expect_err("graph without edges must fail");
	assert!(
		err.to_string().contains("at least one [[edges]]"),
		"got: {err}"
	);
}

#[test]
fn graph_entry_must_reference_a_top_level_step() {
	let wf = parse(
		r#"
		name = "g"
		entry = "ghost"
		max_transitions = 2
		[[steps]]
		name = "only"
		role = "developer:general"
		prompt = "{{input}}"
		[[edges]]
		from = "only"
		to = "$end"
		"#,
	);
	let err = validate(&wf).expect_err("unknown entry must fail");
	assert!(
		err.to_string().contains("entry references unknown"),
		"got: {err}"
	);
}

#[test]
fn graph_dynamic_parallel_source_must_be_a_known_output() {
	let wf = parse(
		r#"
		name = "g"
		entry = "fan"
		max_transitions = 2
		[[steps]]
		name = "fan"
		parallel = true
		source = "ghost"
		match = "(.+)"
		  [[steps.run]]
		  name = "worker"
		  role = "developer:general"
		  prompt = "{{fan}}"
		[[edges]]
		from = "fan"
		to = "$end"
		"#,
	);
	let err = validate(&wf).expect_err("unknown dynamic source must fail");
	assert!(
		err.to_string().contains("source references unknown output"),
		"got: {err}"
	);
}

#[test]
fn graph_edge_endpoints_must_reference_known_steps() {
	let bad_from = parse(
		r#"
		name = "g"
		entry = "a"
		max_transitions = 2
		[[steps]]
		name = "a"
		role = "developer:general"
		prompt = "{{input}}"
		[[edges]]
		from = "ghost"
		to = "$end"
		[[edges]]
		from = "a"
		to = "$end"
		"#,
	);
	let err = validate(&bad_from).expect_err("unknown edge.from must fail");
	assert!(
		err.to_string().contains("edge.from references unknown"),
		"got: {err}"
	);

	let bad_to = parse(
		r#"
		name = "g"
		entry = "a"
		max_transitions = 2
		[[steps]]
		name = "a"
		role = "developer:general"
		prompt = "{{input}}"
		[[edges]]
		from = "a"
		to = "ghost"
		[[edges]]
		from = "a"
		to = "$end"
		"#,
	);
	let err = validate(&bad_to).expect_err("unknown edge.to must fail");
	assert!(
		err.to_string().contains("edge.to references unknown"),
		"got: {err}"
	);
}

#[test]
fn graph_edge_condition_output_must_be_a_known_step() {
	let bad = parse(
		r#"
		name = "g"
		entry = "a"
		max_transitions = 3
		[[steps]]
		name = "a"
		role = "developer:general"
		prompt = "{{input}}"
		[[steps]]
		name = "b"
		role = "developer:general"
		prompt = "p"
		[[edges]]
		from = "a"
		to = "b"
		when = { contains = "go", output = "ghost" }
		[[edges]]
		from = "a"
		to = "$end"
		[[edges]]
		from = "b"
		to = "$end"
		"#,
	);
	let err = validate(&bad).expect_err("unknown condition output must fail");
	assert!(
		err.to_string().contains("condition references unknown output"),
		"got: {err}"
	);

	let good = parse(
		r#"
		name = "g"
		entry = "a"
		max_transitions = 3
		[[steps]]
		name = "a"
		role = "developer:general"
		prompt = "{{input}}"
		[[steps]]
		name = "b"
		role = "developer:general"
		prompt = "p"
		[[edges]]
		from = "a"
		to = "b"
		when = { contains = "go", output = "a" }
		[[edges]]
		from = "a"
		to = "$end"
		[[edges]]
		from = "b"
		to = "$end"
		"#,
	);
	validate(&good).expect("condition output naming a real step validates");
}

#[test]
fn graph_node_without_outgoing_route_is_rejected() {
	let wf = parse(
		r#"
		name = "g"
		entry = "a"
		max_transitions = 3
		[[steps]]
		name = "a"
		role = "developer:general"
		prompt = "{{input}}"
		[[steps]]
		name = "b"
		role = "developer:general"
		prompt = "p"
		[[edges]]
		from = "a"
		to = "$end"
		"#,
	);
	let err = validate(&wf).expect_err("orphan node must fail");
	assert!(err.to_string().contains("has no outgoing route"), "got: {err}");
}

#[test]
fn graph_node_requires_exactly_one_default_edge() {
	let wf = parse(
		r#"
		name = "g"
		entry = "a"
		max_transitions = 2
		[[steps]]
		name = "a"
		role = "developer:general"
		prompt = "{{input}}"
		[[edges]]
		from = "a"
		to = "$end"
		when = { contains = "go" }
		"#,
	);
	let err = validate(&wf).expect_err("conditional-only routes must fail");
	assert!(
		err.to_string()
			.contains("exactly one unconditional default edge (found 0)"),
		"got: {err}"
	);
}

#[test]
fn graph_unreachable_steps_are_rejected() {
	let wf = parse(
		r#"
		name = "g"
		entry = "a"
		max_transitions = 3
		[[steps]]
		name = "a"
		role = "developer:general"
		prompt = "{{input}}"
		[[steps]]
		name = "b"
		role = "developer:general"
		prompt = "p"
		[[edges]]
		from = "a"
		to = "$end"
		[[edges]]
		from = "b"
		to = "$end"
		"#,
	);
	let err = validate(&wf).expect_err("unreachable node must fail");
	assert!(
		err.to_string().contains("unreachable top-level steps: b"),
		"got: {err}"
	);
}

#[test]
fn graph_without_end_route_is_rejected() {
	let wf = parse(
		r#"
		name = "g"
		entry = "a"
		max_transitions = 5
		[[steps]]
		name = "a"
		role = "developer:general"
		prompt = "{{input}}"
		[[steps]]
		name = "b"
		role = "developer:general"
		prompt = "p"
		[[edges]]
		from = "a"
		to = "b"
		[[edges]]
		from = "b"
		to = "a"
		"#,
	);
	let err = validate(&wf).expect_err("cycle without $end must fail");
	assert!(
		err.to_string().contains("no reachable route to $end"),
		"got: {err}"
	);
}

// ── conditions ──────────────────────────────────────────────────────────────

#[test]
fn condition_requires_contains_or_matches() {
	let err = validate_condition(
		"edge 'a -> b'",
		&Condition {
			output: None,
			contains: None,
			matches: None,
		},
	)
	.expect_err("empty condition must fail");
	assert!(
		err.to_string().contains("must set 'contains' or 'matches'"),
		"got: {err}"
	);
}

// ── reserved and empty step names ───────────────────────────────────────────

#[test]
fn reserved_and_empty_step_names_are_rejected() {
	for (name, needle) in [("input", "reserved"), ("$end", "reserved"), ("", "non-empty")] {
		let wf = parse(&format!(
			r#"
		name = "wf"
		[[steps]]
		name = "{name}"
		role = "developer:general"
		prompt = "{{{{input}}}}"
		"#
		));
		let err = validate(&wf).expect_err("reserved/empty names must fail");
		assert!(err.to_string().contains(needle), "name {name:?}: got {err}");
	}
}

// ── structural checks ───────────────────────────────────────────────────────

#[test]
fn dynamic_parallel_min_success_zero_is_rejected() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "plan"
		role = "developer:general"
		prompt = "{{input}}"
		[[steps]]
		name = "fan"
		parallel = true
		source = "plan"
		match = "(.+)"
		min_success = 0
		  [[steps.run]]
		  name = "worker"
		  role = "developer:general"
		  prompt = "{{fan}}"
		"#,
	);
	let err = validate(&wf).expect_err("min_success = 0 must fail");
	assert!(
		err.to_string().contains("min_success must be >= 1"),
		"got: {err}"
	);
}

#[test]
fn static_parallel_source_requires_match() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "fan"
		parallel = true
		source = "plan"
		  [[steps.run]]
		  name = "a"
		  role = "developer:general"
		  prompt = "pa"
		  [[steps.run]]
		  name = "b"
		  role = "developer:general"
		  prompt = "pb"
		"#,
	);
	let err = validate(&wf).expect_err("source without match must fail");
	assert!(err.to_string().contains("source requires match"), "got: {err}");
}

#[test]
fn static_parallel_requires_two_sub_steps() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "fan"
		parallel = true
		  [[steps.run]]
		  name = "a"
		  role = "developer:general"
		  prompt = "pa"
		"#,
	);
	let err = validate(&wf).expect_err("single sub-step must fail");
	assert!(err.to_string().contains("at least 2 sub-steps"), "got: {err}");
}

#[test]
fn parallel_max_parallel_zero_is_rejected() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "fan"
		parallel = true
		max_parallel = 0
		  [[steps.run]]
		  name = "a"
		  role = "developer:general"
		  prompt = "pa"
		  [[steps.run]]
		  name = "b"
		  role = "developer:general"
		  prompt = "pb"
		"#,
	);
	let err = validate(&wf).expect_err("max_parallel = 0 must fail");
	assert!(
		err.to_string().contains("max_parallel must be >= 1"),
		"got: {err}"
	);
}

#[test]
fn loop_without_sub_steps_is_rejected() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		loop = true
		name = "spin"
		run = []
		[steps.exit_when]
		contains = "done"
		"#,
	);
	let err = validate(&wf).expect_err("empty loop body must fail");
	assert!(err.to_string().contains("at least 1 sub-step"), "got: {err}");
}

#[test]
fn loop_requires_exit_when() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		loop = true
		name = "spin"
		[[steps.run]]
		name = "iter"
		role = "developer:general"
		prompt = "p"
		"#,
	);
	let err = validate(&wf).expect_err("loop without exit_when must fail");
	assert!(err.to_string().contains("requires exit_when"), "got: {err}");
}

#[test]
fn empty_model_and_workdir_are_rejected() {
	let bad_model = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "s1"
		role = "developer:general"
		prompt = "{{input}}"
		model = ""
		"#,
	);
	let err = validate(&bad_model).expect_err("empty model must fail");
	assert!(
		err.to_string().contains("model must not be empty"),
		"got: {err}"
	);

	let bad_workdir = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "s1"
		role = "developer:general"
		prompt = "{{input}}"
		workdir = ""
		"#,
	);
	let err = validate(&bad_workdir).expect_err("empty workdir must fail");
	assert!(
		err.to_string().contains("workdir must not be empty"),
		"got: {err}"
	);
}

#[test]
fn step_with_explicit_workdir_validates() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "s1"
		role = "developer:general"
		prompt = "{{input}}"
		workdir = "/tmp"
		"#,
	);
	validate(&wf).expect("non-empty workdir validates");
}

// ── conditional steps ───────────────────────────────────────────────────────

#[test]
fn conditional_workflow_validates() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "prev"
		role = "developer:general"
		prompt = "{{input}}"
		[[steps]]
		conditional = true
		name = "gate"
		on_match = ["yes"]
		on_no_match = ["nope"]
		[steps.condition]
		contains = "go"
		output = "prev"
		[[steps.run]]
		name = "yes"
		role = "developer:general"
		prompt = "ship {{prev}}"
		[[steps.run]]
		name = "nope"
		role = "developer:general"
		prompt = "fix {{prev}}"
		"#,
	);
	validate(&wf).expect("conditional with named condition output validates");
}

#[test]
fn conditional_without_any_branch_is_rejected() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "prev"
		role = "developer:general"
		prompt = "{{input}}"
		[[steps]]
		conditional = true
		name = "gate"
		on_match = []
		on_no_match = []
		[steps.condition]
		contains = "go"
		[[steps.run]]
		name = "yes"
		role = "developer:general"
		prompt = "p"
		"#,
	);
	let err = validate(&wf).expect_err("branchless conditional must fail");
	assert!(
		err.to_string().contains("requires on_match and/or on_no_match"),
		"got: {err}"
	);
}

#[test]
fn conditional_branch_must_reference_a_sub_step() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		conditional = true
		name = "gate"
		on_match = ["ghost"]
		[steps.condition]
		contains = "go"
		[[steps.run]]
		name = "yes"
		role = "developer:general"
		prompt = "p"
		"#,
	);
	let err = validate(&wf).expect_err("unknown branch name must fail");
	assert!(
		err.to_string().contains("branch references unknown sub-step"),
		"got: {err}"
	);
}

// ── reference resolution ────────────────────────────────────────────────────

#[test]
fn dynamic_parallel_source_must_already_be_available() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "fan"
		parallel = true
		source = "later"
		match = "(.+)"
		  [[steps.run]]
		  name = "worker"
		  role = "developer:general"
		  prompt = "{{fan}}"
		[[steps]]
		name = "later"
		role = "developer:general"
		prompt = "{{input}}"
		"#,
	);
	let err = validate(&wf).expect_err("forward source must fail in ordered mode");
	assert!(
		err.to_string().contains("source references unavailable output"),
		"got: {err}"
	);
}

#[test]
fn loop_exit_when_output_must_be_a_known_step() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		loop = true
		name = "spin"
		[steps.exit_when]
		contains = "done"
		output = "ghost"
		[[steps.run]]
		name = "iter"
		role = "developer:general"
		prompt = "p"
		"#,
	);
	let err = validate(&wf).expect_err("unknown exit_when output must fail");
	assert!(
		err.to_string().contains("exit_when.output references unknown step"),
		"got: {err}"
	);
}

#[test]
fn loop_with_named_exit_output_validates() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		loop = true
		name = "spin"
		[steps.exit_when]
		contains = "done"
		output = "iter"
		[[steps.run]]
		name = "iter"
		role = "developer:general"
		prompt = "p"
		"#,
	);
	validate(&wf).expect("exit_when naming a loop sub-step validates");
}

#[test]
fn conditional_condition_output_must_be_available() {
	let wf = parse(
		r#"
		name = "wf"
		[[steps]]
		name = "prev"
		role = "developer:general"
		prompt = "{{input}}"
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
	);
	let err = validate(&wf).expect_err("unknown condition output must fail");
	assert!(
		err.to_string()
			.contains("condition.output references unknown step"),
		"got: {err}"
	);
}

// ── graph nodes of every block kind ─────────────────────────────────────────

#[test]
fn graph_loop_node_validates() {
	let wf = parse(
		r#"
		name = "g"
		entry = "spin"
		max_transitions = 3
		[[steps]]
		loop = true
		name = "spin"
		[steps.exit_when]
		contains = "done"
		[[steps.run]]
		name = "iter"
		role = "developer:general"
		prompt = "p"
		[[edges]]
		from = "spin"
		to = "$end"
		"#,
	);
	validate(&wf).expect("loop node in a graph validates");
}

#[test]
fn graph_conditional_node_validates() {
	let wf = parse(
		r#"
		name = "g"
		entry = "gate"
		max_transitions = 3
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
		[[edges]]
		from = "gate"
		to = "$end"
		"#,
	);
	validate(&wf).expect("conditional node in a graph validates");
}

// ── public tap roles ────────────────────────────────────────────────────────

#[test]
fn public_role_validation_covers_every_step_kind() {
	let wf = parse(
		r#"
		name = "kinds"
		[[steps]]
		name = "leaf"
		role = "developer:general"
		prompt = "p"
		[[steps]]
		parallel = true
		name = "fan"
		  [[steps.run]]
		  name = "a"
		  role = "developer:general"
		  prompt = "pa"
		  [[steps.run]]
		  name = "b"
		  role = "developer:general"
		  prompt = "pb"
		[[steps]]
		loop = true
		name = "spin"
		[steps.exit_when]
		contains = "done"
		[[steps.run]]
		name = "iter"
		role = "developer:general"
		prompt = "p"
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
	);
	let public: HashSet<String> = ["developer:general".to_string()]
		.into_iter()
		.collect();
	validate_public_roles(&wf, &public).expect("all-public workflow passes");

	let none: HashSet<String> = HashSet::new();
	let err = validate_public_roles(&wf, &none).expect_err("unknown role must fail");
	assert!(
		err.to_string().contains("is not a public tap role"),
		"got: {err}"
	);
}
