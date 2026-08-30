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
