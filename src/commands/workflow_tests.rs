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

//! `octomind workflow` command coverage: pure helpers (path detection,
//! truncation), plan rendering for every step kind, local-file dry runs
//! through `execute`, validation error paths, and tap-workflow listing under
//! a sandboxed `OCTOMIND_DATA_DIR`.

use super::*;
use octomind::workflow::schema::{ConditionalStep, LoopStep, ParallelStep};
use serial_test::serial;
use tempfile::tempdir;

use octomind::config::Config;

const DATA_DIR_KEY: &str = "OCTOMIND_DATA_DIR";

/// Snapshot env vars and restore them on drop — a failed assert must not leak
/// a sandboxed data dir into the next test.
struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
	fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}

impl Drop for EnvGuard {
	fn drop(&mut self) {
		for (key, saved) in &self.0 {
			match saved {
				Some(v) => std::env::set_var(key, v),
				None => std::env::remove_var(key),
			}
		}
	}
}

/// A fresh per-test data dir under the system temp dir.
fn sandbox(tag: &str) -> PathBuf {
	let dir = std::env::temp_dir().join(format!("octomind-wf-{tag}-{}", std::process::id()));
	std::fs::create_dir_all(&dir).expect("create sandbox data dir");
	dir
}

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

fn wf_args(name: Option<&str>, dry_run: bool) -> WorkflowArgs {
	WorkflowArgs {
		name: name.map(str::to_string),
		dry_run,
		format: None,
	}
}

fn parse_wf(raw: &str) -> WorkflowDef {
	toml::from_str(raw).expect("workflow parses")
}

/// A minimal valid sequential workflow.
const SEQUENTIAL_WF: &str = r#"
name = "seq"
description = "one step"

[[steps]]
name = "first"
role = "assistant"
prompt = "do {{input}}"
"#;

/// A minimal valid graph workflow with one conditional and one default route.
const GRAPH_WF: &str = r#"
name = "g"
entry = "start"
max_transitions = 3

[[steps]]
name = "start"
role = "assistant"
prompt = "begin {{input}}"

[[steps]]
name = "fin"
role = "assistant"
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
"#;

// ── pure helpers ────────────────────────────────────────────────────────────

#[test]
fn looks_like_path_detects_paths_and_names() {
	assert!(looks_like_path("/abs/workflow.toml"));
	assert!(looks_like_path("rel/workflow.toml"));
	assert!(looks_like_path("rel\\workflow.toml"));
	assert!(looks_like_path("bare.toml"));
	assert!(!looks_like_path("my-workflow"));
	assert!(!looks_like_path(""));
}

#[test]
fn truncate_flattens_and_elides_on_char_boundaries() {
	assert_eq!(truncate("short", 10), "short");
	assert_eq!(truncate("two\nlines", 10), "two lines");
	let long = "x".repeat(130);
	let cut = truncate(&long, 120);
	assert_eq!(cut.chars().count(), 121);
	assert!(cut.ends_with('…'));
	// Multibyte characters must not be split mid-char.
	let unicode = "é".repeat(130);
	let cut = truncate(&unicode, 120);
	assert_eq!(cut.chars().count(), 121);
	assert!(cut.ends_with('…'));
	assert!(cut.chars().take(120).all(|ch| ch == 'é'));
}

// ── plan rendering ──────────────────────────────────────────────────────────

#[test]
fn print_plan_renders_sequential_workflow() {
	print_plan(&parse_wf(SEQUENTIAL_WF));
}

#[test]
fn print_plan_renders_graph_workflow_with_routes() {
	print_plan(&parse_wf(GRAPH_WF));
}

#[test]
fn print_step_renders_every_step_kind() {
	// Sequential with optional model/workdir and a long prompt.
	let seq = parse_wf(
		r#"
name = "kinds"
[[steps]]
name = "leaf"
role = "assistant"
prompt = "p"
model = "openrouter:m"
workdir = "/tmp"
"#,
	);
	print_step(1, &seq.steps[0], 0);

	// Static parallel with count fan-out and success bounds.
	let par = parse_wf(
		r#"
name = "kinds"
[[steps]]
parallel = true
name = "fan"
min_success = 2
max_parallel = 2
[[steps.run]]
name = "a"
role = "assistant"
prompt = "pa"
count = 3
[[steps.run]]
name = "b"
role = "assistant"
prompt = "pb"
"#,
	);
	print_step(2, &par.steps[0], 0);

	// Dynamic parallel (match + source).
	let dynamic = parse_wf(
		r#"
name = "kinds"
[[steps]]
name = "gen"
role = "assistant"
prompt = "list {{input}}"
[[steps]]
parallel = true
name = "fanout"
source = "gen"
match = '(\w+)'
[[steps.run]]
name = "item"
role = "assistant"
prompt = "work on {{fanout}}"
"#,
	);
	print_step(3, &dynamic.steps[1], 0);

	// Loop with and without exit_when.
	let with_exit = parse_wf(
		r#"
name = "kinds"
[[steps]]
loop = true
name = "spin"
max_iterations = 4
[steps.exit_when]
contains = "done"
[[steps.run]]
name = "iter"
role = "assistant"
prompt = "p"
"#,
	);
	print_step(4, &with_exit.steps[0], 0);
	let no_exit = parse_wf(
		r#"
name = "kinds"
[[steps]]
loop = true
name = "spin2"
[[steps.run]]
name = "iter2"
role = "assistant"
prompt = "p"
"#,
	);
	print_step(5, &no_exit.steps[0], 0);

	// Conditional with both branches.
	let cond = parse_wf(
		r#"
name = "kinds"
[[steps]]
conditional = true
name = "gate"
on_match = ["yes"]
on_no_match = ["nope"]
[steps.condition]
contains = "go"
[[steps.run]]
name = "yes"
role = "assistant"
prompt = "p"
[[steps.run]]
name = "nope"
role = "assistant"
prompt = "q"
"#,
	);
	print_step(6, &cond.steps[0], 0);
}

#[test]
fn print_sub_renders_optional_fields() {
	let wf = parse_wf(
		r#"
name = "kinds"
[[steps]]
parallel = true
name = "fan"
[[steps.run]]
name = "a"
role = "assistant"
prompt = "pa"
model = "openrouter:m"
workdir = "/tmp"
count = 3
[[steps.run]]
name = "b"
role = "assistant"
prompt = "pb"
"#,
	);
	let Step::Parallel(p) = &wf.steps[0] else {
		panic!("expected parallel step");
	};
	print_sub(1, &p.run[0], 1);
	print_sub(2, &p.run[1], 1);
}

#[test]
fn step_kinds_parse_from_their_toml_flags() {
	let wf = parse_wf(
		r#"
name = "kinds"
[[steps]]
name = "leaf"
role = "assistant"
prompt = "p"
[[steps]]
parallel = true
name = "fan"
[[steps.run]]
name = "a"
role = "assistant"
prompt = "pa"
[[steps.run]]
name = "b"
role = "assistant"
prompt = "pb"
[[steps]]
loop = true
name = "spin"
[[steps.run]]
name = "iter"
role = "assistant"
prompt = "p"
[[steps]]
conditional = true
name = "gate"
[steps.condition]
contains = "go"
[[steps.run]]
name = "yes"
role = "assistant"
prompt = "p"
"#,
	);
	assert!(matches!(&wf.steps[0], Step::Sequential(_)));
	assert!(matches!(&wf.steps[1], Step::Parallel(ParallelStep { .. })));
	assert!(matches!(&wf.steps[2], Step::Loop(LoopStep { .. })));
	assert!(matches!(
		&wf.steps[3],
		Step::Conditional(ConditionalStep { .. })
	));
}

// ── execute: local files (no env, no network) ───────────────────────────────

#[tokio::test]
async fn dry_run_local_sequential_file_prints_plan() {
	let dir = tempdir().expect("temp dir");
	let file = dir.path().join("seq.toml");
	std::fs::write(&file, SEQUENTIAL_WF.trim()).expect("write workflow");
	execute(
		&wf_args(Some(file.to_str().expect("utf8 path")), true),
		&template_config(),
	)
	.await
	.expect("dry run validates and prints the plan");
}

#[tokio::test]
async fn dry_run_local_graph_file_prints_plan() {
	let dir = tempdir().expect("temp dir");
	let file = dir.path().join("graph.toml");
	std::fs::write(&file, GRAPH_WF.trim()).expect("write workflow");
	execute(
		&wf_args(Some(file.to_str().expect("utf8 path")), true),
		&template_config(),
	)
	.await
	.expect("graph dry run validates and prints the plan");
}

#[tokio::test]
async fn missing_toml_path_errors() {
	let err = execute(
		&wf_args(Some("definitely-missing-12345.toml"), true),
		&template_config(),
	)
	.await
	.expect_err("missing *.toml path must error");
	assert!(
		err.to_string().contains("workflow file not found"),
		"got: {err}"
	);
}

#[tokio::test]
async fn missing_nested_path_errors() {
	let err = execute(
		&wf_args(Some("no/such/dir/x.toml"), true),
		&template_config(),
	)
	.await
	.expect_err("missing nested path must error");
	assert!(
		err.to_string().contains("workflow file not found"),
		"got: {err}"
	);
}

#[tokio::test]
async fn invalid_toml_errors() {
	let dir = tempdir().expect("temp dir");
	let file = dir.path().join("bad.toml");
	std::fs::write(&file, "name = ").expect("write broken workflow");
	let err = execute(
		&wf_args(Some(file.to_str().expect("utf8 path")), true),
		&template_config(),
	)
	.await
	.expect_err("broken TOML must error");
	assert!(err.to_string().contains("failed to parse"), "got: {err}");
}

#[tokio::test]
async fn workflow_without_steps_errors() {
	let dir = tempdir().expect("temp dir");
	let file = dir.path().join("empty.toml");
	std::fs::write(&file, "name = \"empty\"\n").expect("write stepless workflow");
	let err = execute(
		&wf_args(Some(file.to_str().expect("utf8 path")), true),
		&template_config(),
	)
	.await
	.expect_err("stepless workflow must error");
	assert!(err.to_string().contains("no steps"), "got: {err}");
}

#[tokio::test]
async fn duplicate_step_names_error() {
	let dir = tempdir().expect("temp dir");
	let file = dir.path().join("dup.toml");
	std::fs::write(
		&file,
		r#"
name = "dup"
[[steps]]
name = "same"
role = "assistant"
prompt = "p"
[[steps]]
name = "same"
role = "assistant"
prompt = "q"
"#,
	)
	.expect("write workflow");
	let err = execute(
		&wf_args(Some(file.to_str().expect("utf8 path")), true),
		&template_config(),
	)
	.await
	.expect_err("duplicate names must error");
	assert!(
		err.to_string().contains("duplicate step name"),
		"got: {err}"
	);
}

#[tokio::test]
async fn unknown_reference_errors() {
	let dir = tempdir().expect("temp dir");
	let file = dir.path().join("ref.toml");
	std::fs::write(
		&file,
		r#"
name = "ref"
[[steps]]
name = "first"
role = "assistant"
prompt = "use {{nope}}"
"#,
	)
	.expect("write workflow");
	assert!(
		execute(
			&wf_args(Some(file.to_str().expect("utf8 path")), true),
			&template_config()
		)
		.await
		.is_err(),
		"unknown {{ref}} must fail validation"
	);
}

// ── execute: tap-workflow listing (sandboxed data dir) ──────────────────────

#[tokio::test]
#[serial]
async fn listing_without_taps_prints_empty_hint() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("list-empty");
	std::env::set_var(DATA_DIR_KEY, &dir);
	execute(&wf_args(None, false), &template_config())
		.await
		.expect("empty listing is not an error");
}

#[tokio::test]
#[serial]
async fn listing_enumerates_tap_workflows_first_tap_wins() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("list-full");
	std::env::set_var(DATA_DIR_KEY, &dir);

	// A user tap plus the built-in default tap, both with workflows.
	std::fs::write(dir.join("taps.toml"), "[[taps]]\nname = \"acme/lib\"\n")
		.expect("write taps.toml");
	let user_wf = dir
		.join("taps")
		.join("acme")
		.join("octomind-lib")
		.join("workflows");
	std::fs::create_dir_all(&user_wf).expect("user workflows dir");
	std::fs::write(
		user_wf.join("alpha.toml"),
		"description = \"user tap wins\"\n",
	)
	.expect("user workflow");
	let default_wf = dir
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("workflows");
	std::fs::create_dir_all(&default_wf).expect("default workflows dir");
	std::fs::write(
		default_wf.join("alpha.toml"),
		"description = \"shadowed\"\n",
	)
	.expect("shadowed workflow");
	std::fs::write(default_wf.join("beta.toml"), "name = \"beta\"\n").expect("no-description");
	std::fs::write(default_wf.join("notes.txt"), "not a workflow\n").expect("non-toml");
	std::fs::write(default_wf.join("broken.toml"), "description = \n").expect("broken toml");

	let workflows = registry::list_all_tap_workflows().expect("enumerate");
	let names: Vec<&str> = workflows.iter().map(|w| w.name.as_str()).collect();
	assert_eq!(
		names,
		vec!["alpha", "beta"],
		"sorted, non-toml and broken files skipped"
	);
	let alpha = &workflows[0];
	assert_eq!(alpha.source_tap, "acme/lib", "user taps shadow the default");
	assert_eq!(alpha.description, "user tap wins");
	assert_eq!(workflows[1].source_tap, "muvon/tap");
	assert_eq!(
		workflows[1].description, "",
		"missing description renders empty"
	);

	execute(&wf_args(None, false), &template_config())
		.await
		.expect("non-empty listing prints and returns Ok");
}

// ── execute: tap workflows (sandboxed data dir) ──────────────────────────

/// A minimal valid workflow as a tap would ship it.
const TAP_WF: &str = r#"
name = "alpha"
description = "from a tap"
max_cost = 0.5

[[steps]]
name = "only"
role = "developer:general"
prompt = "do {{input}}"
"#;

/// Install a user tap (`acme/lib`) shipping one workflow, plus the built-in
/// default tap directory so tap loading never reaches for git.
fn install_tap_workflow(dir: &std::path::Path, body: &str) {
	std::fs::write(dir.join("taps.toml"), "[[taps]]\nname = \"acme/lib\"\n")
		.expect("write taps.toml");
	let workflows = dir
		.join("taps")
		.join("acme")
		.join("octomind-lib")
		.join("workflows");
	std::fs::create_dir_all(&workflows).expect("tap workflows dir");
	std::fs::write(workflows.join("alpha.toml"), body).expect("tap workflow");
	let default_tap = dir.join("taps").join("muvon").join("octomind-tap");
	std::fs::create_dir_all(default_tap.join("workflows")).expect("default tap dir");
}

#[tokio::test]
#[serial]
async fn tap_workflow_dry_run_checks_public_roles_and_prints_plan() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("tap-dry");
	std::env::set_var(DATA_DIR_KEY, &dir);
	install_tap_workflow(&dir, TAP_WF.trim());

	// The workflow's role is public: the tap ships a matching agent file.
	let agents = dir
		.join("taps")
		.join("acme")
		.join("octomind-lib")
		.join("agents")
		.join("developer");
	std::fs::create_dir_all(&agents).expect("tap agents dir");
	std::fs::write(agents.join("general.toml"), "# agent\n").expect("tap agent");

	execute(&wf_args(Some("alpha"), true), &template_config())
		.await
		.expect("tap dry run validates public roles and prints the plan");
}

#[tokio::test]
#[serial]
async fn tap_workflow_with_non_public_role_errors() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("tap-role");
	std::env::set_var(DATA_DIR_KEY, &dir);
	install_tap_workflow(&dir, TAP_WF.trim());

	// No agents shipped → the workflow's role is not a public tap role.
	let err = execute(&wf_args(Some("alpha"), true), &template_config())
		.await
		.expect_err("a non-public role must fail");
	assert!(
		err.to_string().contains("is not a public tap role"),
		"got: {err}"
	);
}

#[tokio::test]
#[serial]
async fn local_workflow_requires_input_via_stdin() {
	use std::os::fd::AsRawFd;

	let dir = tempdir().expect("temp dir");
	let file = dir.path().join("seq.toml");
	std::fs::write(&file, SEQUENTIAL_WF.trim()).expect("write workflow");

	// Point stdin at /dev/null so the read is deterministically empty even
	// under the test harness's own stdin. fd 0 is process-global → serial.
	let saved = unsafe { libc::dup(libc::STDIN_FILENO) };
	assert!(saved >= 0, "dup stdin for restore");
	let null = std::fs::File::open("/dev/null").expect("/dev/null");
	unsafe {
		libc::dup2(null.as_raw_fd(), libc::STDIN_FILENO);
	}
	let result = execute(
		&wf_args(Some(file.to_str().expect("utf8 path")), false),
		&template_config(),
	)
	.await;
	unsafe {
		libc::dup2(saved, libc::STDIN_FILENO);
		libc::close(saved);
	}
	let err = result.expect_err("empty stdin must be rejected");
	assert!(
		err.to_string().contains("requires input via stdin"),
		"got: {err}"
	);
}
