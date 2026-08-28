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

#[test]
fn end_node_constant() {
	assert_eq!(END_NODE, "$end");
}

#[test]
fn session_mode_default_is_fresh() {
	assert_eq!(SessionMode::default(), SessionMode::Fresh);
}

#[test]
fn workflow_def_is_graph_false_without_entry_or_edges() {
	let def: WorkflowDef = toml::from_str(
		r#"
		name = "release"

		[[steps]]
		name = "build"
		role = "developer"
		prompt = "Build the project"
		"#,
	)
	.expect("valid workflow");

	assert!(!def.is_graph());
}

#[test]
fn workflow_def_is_graph_true_with_entry() {
	let def: WorkflowDef = toml::from_str(
		r#"
		name = "release"
		entry = "build"
		max_transitions = 5

		[[steps]]
		name = "build"
		role = "developer"
		prompt = "Build the project"
		"#,
	)
	.expect("valid workflow");

	assert!(def.is_graph());
}

#[test]
fn workflow_def_is_graph_true_with_edges_only() {
	let def: WorkflowDef = toml::from_str(
		r#"
		name = "release"

		[[edges]]
		from = "build"
		to = "$end"
		"#,
	)
	.expect("valid workflow");

	assert!(def.is_graph());
}

#[test]
fn sequential_replica_count_defaults_to_one() {
	let step: Sequential = toml::from_str(
		r#"
		name = "build"
		role = "developer"
		prompt = "Build it"
		"#,
	)
	.expect("valid sequential step");

	assert_eq!(step.replica_count(), 1);
}

#[test]
fn sequential_replica_count_returns_count() {
	let step: Sequential = toml::from_str(
		r#"
		name = "sample"
		role = "developer"
		prompt = "Try it"
		count = 4
		"#,
	)
	.expect("valid sequential step");

	assert_eq!(step.replica_count(), 4);
}

#[test]
fn sequential_applies_serde_defaults() {
	let step: Sequential = toml::from_str(
		r#"
		name = "build"
		role = "developer"
		prompt = "Build it"
		"#,
	)
	.expect("valid sequential step");

	assert_eq!(step.session, SessionMode::Fresh);
	assert_eq!(step.timeout, 0);
	assert_eq!(step.retries, 0);
	assert_eq!(step.count, None);
	assert_eq!(step.model, None);
	assert_eq!(step.workdir, None);
}

#[test]
fn step_name_returns_name_for_each_variant() {
	let sequential: Step = toml::from_str(
		r#"
		name = "seq"
		role = "developer"
		prompt = "p"
		"#,
	)
	.expect("valid step");
	let parallel: Step = toml::from_str(
		r#"
		name = "par"
		parallel = true

		[[run]]
		name = "branch"
		role = "developer"
		prompt = "p"
		"#,
	)
	.expect("valid step");
	let looped: Step = toml::from_str(
		r#"
		name = "loo"
		loop = true

		[[run]]
		name = "attempt"
		role = "developer"
		prompt = "p"
		"#,
	)
	.expect("valid step");
	let conditional: Step = toml::from_str(
		r#"
		name = "con"
		conditional = true

		[condition]
		contains = "ok"

		[[run]]
		name = "branch"
		role = "developer"
		prompt = "p"
		"#,
	)
	.expect("valid step");

	assert_eq!(sequential.name(), "seq");
	assert_eq!(parallel.name(), "par");
	assert_eq!(looped.name(), "loo");
	assert_eq!(conditional.name(), "con");
}

#[test]
fn step_deserializes_sequential_without_flags() {
	let step: Step = toml::from_str(
		r#"
		name = "build"
		role = "developer"
		prompt = "Build it"
		"#,
	)
	.expect("valid step");

	match step {
		Step::Sequential(s) => {
			assert_eq!(s.name, "build");
			assert_eq!(s.role, "developer");
			assert_eq!(s.prompt, "Build it");
		}
		other => panic!("expected Step::Sequential, got {other:?}"),
	}
}

#[test]
fn step_deserializes_parallel_flag() {
	let step: Step = toml::from_str(
		r#"
		name = "fanout"
		parallel = true

		[[run]]
		name = "unit"
		role = "developer"
		prompt = "Run unit tests"

		[[run]]
		name = "lint"
		role = "reviewer"
		prompt = "Run linter"
		"#,
	)
	.expect("valid step");

	match step {
		Step::Parallel(p) => {
			assert_eq!(p.name, "fanout");
			assert_eq!(p.run.len(), 2);
			assert_eq!(p.run[0].name, "unit");
			assert_eq!(p.run[1].role, "reviewer");
		}
		other => panic!("expected Step::Parallel, got {other:?}"),
	}
}

#[test]
fn step_deserializes_loop_flag() {
	let step: Step = toml::from_str(
		r#"
		name = "retry"
		loop = true

		[[run]]
		name = "attempt"
		role = "developer"
		prompt = "Try the task"
		"#,
	)
	.expect("valid step");

	match step {
		Step::Loop(l) => {
			assert_eq!(l.name, "retry");
			assert_eq!(l.max_iterations, 10); // default_max_iterations
			assert_eq!(l.run.len(), 1);
			assert!(l.exit_when.is_none());
		}
		other => panic!("expected Step::Loop, got {other:?}"),
	}
}

#[test]
fn step_deserializes_conditional_flag() {
	let step: Step = toml::from_str(
		r#"
		name = "gate"
		conditional = true
		on_match = ["proceed"]

		[condition]
		contains = "ready"

		[[run]]
		name = "proceed"
		role = "developer"
		prompt = "Continue"
		"#,
	)
	.expect("valid step");

	match step {
		Step::Conditional(c) => {
			assert_eq!(c.name, "gate");
			assert_eq!(c.condition.contains.as_deref(), Some("ready"));
			assert_eq!(c.on_match, vec!["proceed".to_string()]);
			assert!(c.on_no_match.is_empty());
			assert_eq!(c.run.len(), 1);
		}
		other => panic!("expected Step::Conditional, got {other:?}"),
	}
}

#[test]
fn step_rejects_multiple_flags() {
	let err = toml::from_str::<Step>(
		r#"
		name = "bad"
		parallel = true
		loop = true

		[[run]]
		name = "branch"
		role = "developer"
		prompt = "p"
		"#,
	)
	.expect_err("multiple flags must be rejected");

	assert!(
		err.to_string().contains("at most one of"),
		"unexpected error: {err}"
	);
}

#[test]
fn edge_deserializes_with_and_without_condition() {
	let def: WorkflowDef = toml::from_str(
		r#"
		name = "graph"

		[[edges]]
		from = "build"
		to = "test"

		[[edges]]
		from = "test"
		to = "$end"

		[edges.when]
		output = "test"
		matches = "ok|passed"
		"#,
	)
	.expect("valid workflow");

	assert_eq!(def.edges.len(), 2);
	assert_eq!(def.edges[0].from, "build");
	assert_eq!(def.edges[0].to, "test");
	assert!(def.edges[0].when.is_none());

	let when = def.edges[1].when.as_ref().expect("condition present");
	assert_eq!(when.output.as_deref(), Some("test"));
	assert_eq!(when.matches.as_deref(), Some("ok|passed"));
	assert_eq!(when.contains, None);
}
