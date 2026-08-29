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

//! Filesystem-backed coverage for project-local tools: real temp directories,
//! real executable bits, real subprocesses — discovery, header parsing, and
//! the execute() calling convention (stdin JSON, OCTOMIND_PARAM_* env).

use super::*;

fn make_executable(path: &Path) {
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
			.expect("chmod +x fixture");
	}
	#[cfg(not(unix))]
	{
		let _ = path;
	}
}

/// Write `<workdir>/.agents/tools/<name>` with the given body and +x.
fn write_tool(workdir: &Path, name: &str, body: &str) -> PathBuf {
	let dir = workdir.join(TOOLS_DIR);
	std::fs::create_dir_all(&dir).expect("create tools dir");
	let path = dir.join(name);
	std::fs::write(&path, body).expect("write tool script");
	make_executable(&path);
	path
}

fn call(tool_name: &str, params: Value) -> McpToolCall {
	McpToolCall {
		tool_name: tool_name.to_string(),
		parameters: params,
		tool_id: "t1".to_string(),
	}
}

// ---------------------------------------------------------------------------
// discover()
// ---------------------------------------------------------------------------

#[test]
fn discover_returns_empty_when_tools_directory_is_missing() {
	let tmp = tempfile::tempdir().expect("tempdir");
	assert!(discover(tmp.path()).is_empty());
}

#[test]
fn discover_finds_executable_tool_with_parsed_header() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = write_tool(
		tmp.path(),
		"greet",
		"#!/bin/sh\n# @description Say hi.\n# @param *name string Who to greet\necho hi\n",
	);

	let tools = discover(tmp.path());
	assert_eq!(tools.len(), 1);
	let tool = &tools[0];
	assert_eq!(tool.name, "greet");
	assert_eq!(tool.description, "Say hi.");
	assert_eq!(tool.path, path);
	assert_eq!(tool.params.len(), 1);
	assert_eq!(tool.params[0].name, "name");
	assert!(tool.params[0].required);
}

#[test]
fn discover_skips_files_without_the_executable_bit() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let dir = tmp.path().join(TOOLS_DIR);
	std::fs::create_dir_all(&dir).expect("create tools dir");
	std::fs::write(dir.join("noexec"), "#!/bin/sh\n# @description x\necho x\n")
		.expect("write fixture");
	assert!(discover(tmp.path()).is_empty());
}

#[test]
fn discover_skips_directories_and_invalid_names() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let dir = tmp.path().join(TOOLS_DIR);
	std::fs::create_dir_all(dir.join("subdir")).expect("create nested dir");
	for name in [".hidden", "-dash", "dot.ext", "sp ace"] {
		let path = dir.join(name);
		std::fs::write(&path, "#!/bin/sh\n# @description x\necho x\n").expect("write fixture");
		make_executable(&path);
	}
	assert!(discover(tmp.path()).is_empty());
}

#[test]
fn discover_skips_scripts_without_a_description() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_tool(
		tmp.path(),
		"nodesc",
		"#!/bin/sh\n# @param a string x\necho x\n",
	);
	assert!(discover(tmp.path()).is_empty());
}

#[test]
#[cfg(unix)]
fn discover_skips_scripts_it_cannot_read() {
	use std::os::unix::fs::PermissionsExt;
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = write_tool(
		tmp.path(),
		"noread",
		"#!/bin/sh\n# @description x\necho x\n",
	);
	std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o111))
		.expect("chmod 111 fixture");
	assert!(discover(tmp.path()).is_empty());
}

#[test]
fn get_all_functions_and_is_local_tool_follow_the_thread_workdir() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_tool(
		tmp.path(),
		"probe",
		"#!/bin/sh\n# @description Probe.\necho ok\n",
	);
	crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

	let functions = get_all_functions();
	assert_eq!(functions.len(), 1);
	assert_eq!(functions[0].name, "probe");
	assert_eq!(functions[0].description, "Probe.");
	assert!(functions[0].parameters["required"]
		.as_array()
		.expect("required is an array")
		.is_empty());
	assert!(is_local_tool("probe"));
	assert!(!is_local_tool("absent"));
}

// ---------------------------------------------------------------------------
// execute()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_returns_stdout_as_success_content() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_tool(
		tmp.path(),
		"hello",
		"#!/bin/sh\n# @description Greets.\necho 'hello from tool'\n",
	);
	crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

	let result = execute(&call("hello", json!({})))
		.await
		.expect("execute succeeds");
	assert!(!result.is_error());
	assert_eq!(result.extract_content().trim(), "hello from tool");
	assert_eq!(result.tool_id, "t1");
}

#[tokio::test]
async fn execute_passes_params_via_env_and_stdin() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_tool(
		tmp.path(),
		"echoenv",
		"#!/bin/sh\n# @description Echoes.\nprintf 'target=%s\\n' \"$OCTOMIND_PARAM_TARGET\"\ncat\n",
	);
	crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

	let result = execute(&call("echoenv", json!({"target": "x.txt", "force": true})))
		.await
		.expect("execute succeeds");
	let content = result.extract_content();
	assert!(
		content.contains("target=x.txt"),
		"env param missing: {content}"
	);
	assert!(
		content.contains("\"force\":true"),
		"stdin JSON params missing: {content}"
	);
}

#[tokio::test]
async fn execute_appends_stderr_with_a_marker_on_success() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_tool(
		tmp.path(),
		"noisy",
		"#!/bin/sh\n# @description Noisy.\necho out\necho warn >&2\n",
	);
	crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

	let result = execute(&call("noisy", json!({})))
		.await
		.expect("execute succeeds");
	assert!(!result.is_error());
	let content = result.extract_content();
	assert!(content.contains("out"));
	assert!(content.contains("[stderr]"), "content: {content}");
	assert!(content.contains("warn"));
}

#[tokio::test]
async fn execute_reports_nonzero_exit_as_an_error_result() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_tool(
		tmp.path(),
		"failer",
		"#!/bin/sh\n# @description Fails.\necho partial\necho boom >&2\nexit 3\n",
	);
	crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

	let result = execute(&call("failer", json!({})))
		.await
		.expect("routing never returns Err for exit codes");
	assert!(result.is_error());
	let content = result.extract_content();
	assert!(content.contains("exited with status"), "content: {content}");
	assert!(content.contains("[stderr]"));
	assert!(content.contains("boom"));
	assert!(content.contains("[stdout]"));
	assert!(content.contains("partial"));
}

#[tokio::test]
async fn execute_errors_for_an_unknown_tool_name() {
	let tmp = tempfile::tempdir().expect("tempdir");
	crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

	let err = execute(&call("ghost", json!({})))
		.await
		.expect_err("tool is absent");
	assert!(err.to_string().contains("not found"), "err: {err}");
}

#[tokio::test]
async fn execute_reports_spawn_failures() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_tool(
		tmp.path(),
		"badinterp",
		"#!/no/such/interpreter\n# @description x\necho x\n",
	);
	crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

	let err = execute(&call("badinterp", json!({})))
		.await
		.expect_err("spawn must fail");
	assert!(err.to_string().contains("spawn"), "err: {err}");
}

// ---------------------------------------------------------------------------
// Header parsing helpers
// ---------------------------------------------------------------------------

#[test]
fn strip_comment_prefix_accepts_sql_and_bare_markers() {
	assert_eq!(strip_comment_prefix("-- drop it"), Some("drop it"));
	assert_eq!(strip_comment_prefix("--drop"), Some("drop"));
	assert_eq!(strip_comment_prefix("#bare"), Some("bare"));
	assert_eq!(strip_comment_prefix("code"), None);
	assert_eq!(strip_comment_prefix(""), None);
}

#[test]
fn extract_header_allows_leading_blanks_and_caps_the_scan() {
	let src = "#!/usr/bin/env bash\n\n\n# @description Hi.\n# @param a string x\necho hi\n";
	let header = extract_header(src);
	assert!(header.contains("@description Hi."));
	assert!(header.contains("@param a"));

	let long: String = (0..100).map(|i| format!("# line {i}\n")).collect();
	assert_eq!(extract_header(&long).lines().count(), HEADER_MAX_LINES);
}

#[test]
fn parse_header_accepts_aliases_and_ignores_unknown_tags() {
	let h = "@desc Alias works.\n@unknown tag value\n@arg *path string Where\n";
	let meta = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
	assert_eq!(meta.description, "Alias works.");
	assert_eq!(meta.params.len(), 1);
	assert_eq!(meta.params[0].name, "path");
	assert!(meta.params[0].required);
}

#[test]
fn parse_param_line_defaults_the_type_and_joins_the_description() {
	let p = parse_param_line("count  string  the  count  to reach").unwrap();
	assert_eq!(p.name, "count");
	assert_eq!(p.ty, "string");
	assert_eq!(p.description, "the count to reach");
	assert!(!p.required);

	// No type token: defaults to string, description stays empty.
	let p = parse_param_line("solo").unwrap();
	assert_eq!(p.ty, "string");
	assert_eq!(p.description, "");
	assert!(parse_param_line("").is_none());
}

#[test]
fn normalize_type_maps_every_alias() {
	for (raw, want) in [
		("str", "string"),
		("STRING", "string"),
		("int", "integer"),
		("integer", "integer"),
		("num", "number"),
		("float", "number"),
		("number", "number"),
		("bool", "boolean"),
		("boolean", "boolean"),
		("list", "array"),
		("array", "array"),
		("obj", "object"),
		("map", "object"),
		("object", "object"),
		("weird", "string"),
	] {
		assert_eq!(normalize_type(raw), want);
	}
}

#[test]
fn parse_file_reads_the_script_from_disk() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = write_tool(
		tmp.path(),
		"diskread",
		"#!/usr/bin/env node\n// @description From disk.\nconsole.log(1)\n",
	);
	let meta = parse_file(&path, "diskread").unwrap();
	assert_eq!(meta.name, "diskread");
	assert_eq!(meta.description, "From disk.");
	assert_eq!(meta.path, path);
}

#[test]
#[cfg(unix)]
fn is_executable_reflects_the_mode_bits() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let p = tmp.path().join("f");
	std::fs::write(&p, "x").expect("write fixture");
	assert!(!is_executable(&p));
	make_executable(&p);
	assert!(is_executable(&p));
	assert!(!is_executable(&tmp.path().join("missing")));
}
