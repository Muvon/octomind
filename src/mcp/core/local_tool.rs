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

//! Project-local MCP tools — shebang scripts at `<workdir>/.agents/tools/<name>`.
//!
//! Auto-discovered every turn from disk; no config, no env var. Mirrors the
//! always-on injection shape of `OCTOMIND_SKILLS` but driven purely by file
//! presence, so any project can drop in tools without editing the role config.
//!
//! ## File contract
//!
//! - Path: `<workdir>/.agents/tools/<tool-name>`
//! - Filename = tool name (must match `[A-Za-z0-9_-]+`).
//! - File must be executable (`chmod +x`). Non-executable files are skipped.
//! - First line may be a shebang (`#!...`); skipped during header parsing.
//! - The leading comment block (lines starting with `#`, `//`, or `--`) defines
//!   the tool schema. Parsing stops at the first non-comment, non-blank line, or
//!   after `HEADER_MAX_LINES`.
//!
//! ## Header schema
//!
//! ```text
//! #!/usr/bin/env bash
//! # @description Short summary of what the tool does.
//! # Continuation lines (no @ prefix) append to the previous tag.
//! # @param *target string Path to operate on    (required: leading *)
//! # @param force boolean Overwrite existing      (optional: no *)
//! # @param *count integer Iterations
//! ```
//!
//! - `@description` is required (or `@desc`).
//! - `@param NAME TYPE DESCRIPTION` — repeatable. TYPE ∈
//!   `{string, number, integer, boolean, array, object}` (default `string`).
//!   Prefix the name with `*` to mark the param as required:
//!   `@param *target string Path to operate on`. Unmarked = optional. This
//!   mirrors how octomind renders the schema in `/tools` output.
//! - Unknown tags are ignored with a debug log.
//!
//! ## Calling convention (script side)
//!
//! - stdin: JSON object of all params (`{"target":"x.txt","force":true}`).
//! - env: `OCTOMIND_PARAM_<UPPERCASE_NAME>` for each param (string form;
//!   complex values JSON-stringified). Plus `OCTOMIND_TOOL_NAME`,
//!   `OCTOMIND_WORKDIR`.
//! - cwd: the session's working directory.
//! - stdout = success content shown to the model.
//! - stderr appended to the result with an `[stderr]` marker.
//! - non-zero exit = error result.

use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Project-relative tools directory.
pub const TOOLS_DIR: &str = ".agents/tools";

/// Synthetic builtin server name reserved for project-local tools.
pub const SERVER_NAME: &str = "local";

/// Maximum leading lines scanned for the header. Prevents reading entire files
/// when the contract is "header is at the top".
const HEADER_MAX_LINES: usize = 80;

/// Default tool execution timeout if not overridden.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub struct LocalToolMeta {
	pub name: String,
	pub description: String,
	pub params: Vec<ParamDef>,
	pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ParamDef {
	pub name: String,
	pub ty: String,
	pub description: String,
	pub required: bool,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Scan `<workdir>/.agents/tools/` and return one entry per valid tool file.
pub fn discover(workdir: &Path) -> Vec<LocalToolMeta> {
	let dir = workdir.join(TOOLS_DIR);
	let entries = match std::fs::read_dir(&dir) {
		Ok(e) => e,
		Err(_) => return Vec::new(),
	};

	let mut out = Vec::new();
	for ent in entries.flatten() {
		let path = ent.path();
		if !path.is_file() {
			continue;
		}

		let fname = match path.file_name().and_then(|n| n.to_str()) {
			Some(n) => n,
			None => continue,
		};
		if !is_valid_tool_name(fname) {
			continue;
		}
		if !is_executable(&path) {
			crate::log_debug!(
				"local_tool: '{}' not executable (chmod +x to enable)",
				path.display()
			);
			continue;
		}

		match parse_file(&path, fname) {
			Ok(meta) => out.push(meta),
			Err(e) => crate::log_debug!("local_tool: parse {} failed: {}", path.display(), e),
		}
	}
	out
}

/// MCP function definitions for all local tools in the current session's workdir.
pub fn get_all_functions() -> Vec<McpFunction> {
	let workdir = crate::mcp::workdir::get_thread_working_directory();
	discover(&workdir)
		.into_iter()
		.map(|t| t.to_function())
		.collect()
}

/// True if a tool with `name` exists in the current session's workdir.
pub fn is_local_tool(name: &str) -> bool {
	let workdir = crate::mcp::workdir::get_thread_working_directory();
	discover(&workdir).iter().any(|t| t.name == name)
}

/// Execute a local-tool call. Spawns the script, pipes JSON params on stdin,
/// captures stdout/stderr, returns the result.
pub async fn execute(call: &McpToolCall) -> Result<McpToolResult> {
	let workdir = crate::mcp::workdir::get_thread_working_directory();
	let tools = discover(&workdir);
	let tool = tools
		.iter()
		.find(|t| t.name == call.tool_name)
		.ok_or_else(|| {
			anyhow::anyhow!(
				"local tool '{}' not found in {}/{}",
				call.tool_name,
				workdir.display(),
				TOOLS_DIR
			)
		})?;

	let params_json = serde_json::to_string(&call.parameters).unwrap_or_else(|_| "{}".to_string());

	let mut cmd = tokio::process::Command::new(&tool.path);
	cmd.current_dir(&workdir);
	cmd.env("OCTOMIND_TOOL_NAME", &tool.name);
	cmd.env("OCTOMIND_WORKDIR", workdir.display().to_string());

	if let Some(obj) = call.parameters.as_object() {
		for (k, v) in obj {
			let val = match v {
				Value::String(s) => s.clone(),
				Value::Null => String::new(),
				Value::Bool(b) => b.to_string(),
				Value::Number(n) => n.to_string(),
				_ => serde_json::to_string(v).unwrap_or_default(),
			};
			cmd.env(format!("OCTOMIND_PARAM_{}", k.to_uppercase()), val);
		}
	}

	cmd.stdin(std::process::Stdio::piped());
	cmd.stdout(std::process::Stdio::piped());
	cmd.stderr(std::process::Stdio::piped());

	let mut child = cmd
		.spawn()
		.map_err(|e| anyhow::anyhow!("spawn '{}' failed: {}", tool.path.display(), e))?;

	if let Some(mut stdin) = child.stdin.take() {
		use tokio::io::AsyncWriteExt;
		let _ = stdin.write_all(params_json.as_bytes()).await;
		let _ = stdin.shutdown().await;
	}

	let output = tokio::time::timeout(
		std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS),
		child.wait_with_output(),
	)
	.await
	.map_err(|_| {
		anyhow::anyhow!(
			"local tool '{}' timed out after {}s",
			tool.name,
			DEFAULT_TIMEOUT_SECS
		)
	})?
	.map_err(|e| anyhow::anyhow!("wait for '{}' failed: {}", tool.name, e))?;

	let stdout = String::from_utf8_lossy(&output.stdout).to_string();
	let stderr = String::from_utf8_lossy(&output.stderr).to_string();

	if !output.status.success() {
		let mut msg = format!(
			"local tool '{}' exited with status {}",
			tool.name, output.status
		);
		if !stderr.is_empty() {
			msg.push_str("\n[stderr]\n");
			msg.push_str(stderr.trim_end());
		}
		if !stdout.is_empty() {
			msg.push_str("\n[stdout]\n");
			msg.push_str(stdout.trim_end());
		}
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			msg,
		));
	}

	let mut content = stdout;
	if !stderr.trim().is_empty() {
		if !content.is_empty() && !content.ends_with('\n') {
			content.push('\n');
		}
		content.push_str("[stderr]\n");
		content.push_str(stderr.trim_end());
	}

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		content,
	))
}

// ---------------------------------------------------------------------------
// Conversion: meta -> McpFunction
// ---------------------------------------------------------------------------

impl LocalToolMeta {
	pub fn to_function(&self) -> McpFunction {
		let mut props = serde_json::Map::new();
		let mut required: Vec<String> = Vec::new();

		for p in &self.params {
			props.insert(
				p.name.clone(),
				json!({
					"type": p.ty,
					"description": p.description,
				}),
			);
			if p.required {
				required.push(p.name.clone());
			}
		}

		McpFunction {
			name: self.name.clone(),
			description: self.description.clone(),
			parameters: json!({
				"type": "object",
				"properties": Value::Object(props),
				"required": required,
			}),
		}
	}
}

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

fn is_valid_tool_name(s: &str) -> bool {
	!s.is_empty()
		&& !s.starts_with('-')
		&& !s.starts_with('.')
		&& s.chars()
			.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn is_executable(p: &Path) -> bool {
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::metadata(p)
			.map(|m| m.permissions().mode() & 0o111 != 0)
			.unwrap_or(false)
	}
	#[cfg(not(unix))]
	{
		// On non-unix, treat any readable file as runnable; the OS will reject if not.
		p.exists()
	}
}

// ---------------------------------------------------------------------------
// Header parsing
// ---------------------------------------------------------------------------

fn parse_file(path: &Path, name: &str) -> Result<LocalToolMeta> {
	let content = std::fs::read_to_string(path)?;
	let header = extract_header(&content);
	parse_header(&header, path, name)
}

/// Extract the leading comment block. Skips a shebang on line 1.
/// Stops at the first non-comment, non-blank line. Capped at HEADER_MAX_LINES.
fn extract_header(content: &str) -> String {
	let mut out = String::new();
	let mut started = false;

	for (i, line) in content.lines().take(HEADER_MAX_LINES).enumerate() {
		if i == 0 && line.starts_with("#!") {
			continue;
		}
		let trimmed = line.trim_start();

		if trimmed.is_empty() {
			if !started {
				continue; // allow blank lines between shebang and header
			}
			break;
		}

		match strip_comment_prefix(trimmed) {
			Some(rest) => {
				started = true;
				out.push_str(rest);
				out.push('\n');
			}
			None => break, // first non-comment line ends the header
		}
	}
	out
}

/// Strip a single comment prefix (`# `, `#`, `// `, `//`, `-- `, `--`).
fn strip_comment_prefix(s: &str) -> Option<&str> {
	if let Some(r) = s.strip_prefix("# ") {
		return Some(r);
	}
	if let Some(r) = s.strip_prefix("#") {
		return Some(r);
	}
	if let Some(r) = s.strip_prefix("// ") {
		return Some(r);
	}
	if let Some(r) = s.strip_prefix("//") {
		return Some(r);
	}
	if let Some(r) = s.strip_prefix("-- ") {
		return Some(r);
	}
	if let Some(r) = s.strip_prefix("--") {
		return Some(r);
	}
	None
}

fn parse_header(header: &str, path: &Path, name: &str) -> Result<LocalToolMeta> {
	let mut description = String::new();
	let mut params: Vec<ParamDef> = Vec::new();
	let mut current_tag: Option<&'static str> = None;

	for raw in header.lines() {
		let line = raw.trim();
		if line.is_empty() {
			continue;
		}

		if let Some(rest) = line.strip_prefix('@') {
			let mut split = rest.splitn(2, char::is_whitespace);
			let tag = split.next().unwrap_or("").trim().to_lowercase();
			let val = split.next().unwrap_or("").trim();

			match tag.as_str() {
				"description" | "desc" => {
					if !description.is_empty() {
						description.push('\n');
					}
					description.push_str(val);
					current_tag = Some("description");
				}
				"param" | "arg" => {
					if let Some(p) = parse_param_line(val) {
						params.push(p);
					} else {
						crate::log_debug!(
							"local_tool: malformed @param in {}: '{}'",
							path.display(),
							val
						);
					}
					current_tag = None;
				}
				other => {
					crate::log_debug!("local_tool: unknown tag @{} in {}", other, path.display());
					current_tag = None;
				}
			}
		} else if current_tag == Some("description") {
			if !description.is_empty() && !description.ends_with('\n') {
				description.push('\n');
			}
			description.push_str(line);
		}
	}

	let description = description.trim().to_string();
	if description.is_empty() {
		return Err(anyhow::anyhow!(
			"missing @description in {}",
			path.display()
		));
	}

	Ok(LocalToolMeta {
		name: name.to_string(),
		description,
		params,
		path: path.to_path_buf(),
	})
}

/// Parse `NAME TYPE DESCRIPTION...` from the value half of `@param`.
///
/// A leading `*` on the name marks the param as required. No `*` = optional.
/// This mirrors how octomind renders required params in `/tools` output, so
/// the file format and the displayed schema use the same visual convention.
fn parse_param_line(s: &str) -> Option<ParamDef> {
	let mut it = s.split_whitespace();
	let raw_name = it.next()?;

	let (required, name) = if let Some(stripped) = raw_name.strip_prefix('*') {
		(true, stripped.to_string())
	} else {
		(false, raw_name.to_string())
	};
	if name.is_empty() {
		return None;
	}

	let ty_raw = it.next().unwrap_or("string").to_string();
	let ty = normalize_type(&ty_raw);
	let description = it.collect::<Vec<&str>>().join(" ");

	Some(ParamDef {
		name,
		ty,
		description,
		required,
	})
}

fn normalize_type(t: &str) -> String {
	match t.to_lowercase().as_str() {
		"str" | "string" => "string",
		"int" | "integer" => "integer",
		"num" | "number" | "float" => "number",
		"bool" | "boolean" => "boolean",
		"array" | "list" => "array",
		"object" | "obj" | "map" => "object",
		_ => "string",
	}
	.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_simple_header() {
		let h = "@description Does the thing.\n@param *target string Path to file\n@param force boolean Overwrite\n";
		let meta = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
		assert_eq!(meta.description, "Does the thing.");
		assert_eq!(meta.params.len(), 2);
		assert_eq!(meta.params[0].name, "target");
		assert_eq!(meta.params[0].ty, "string");
		assert!(meta.params[0].required);
		assert_eq!(meta.params[1].name, "force");
		assert_eq!(meta.params[1].ty, "boolean");
		assert!(!meta.params[1].required);
	}

	#[test]
	fn star_prefix_marks_required() {
		// no star → optional (default)
		let h = "@description x\n@param a string the a\n";
		let m = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
		assert!(!m.params[0].required);

		// star → required, name has no star
		let h = "@description x\n@param *a string the a\n";
		let m = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
		assert!(m.params[0].required);
		assert_eq!(m.params[0].name, "a");
	}

	#[test]
	fn lone_star_param_is_skipped() {
		let h = "@description x\n@param * string oops\n@param *real string ok\n";
		let m = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
		assert_eq!(m.params.len(), 1);
		assert_eq!(m.params[0].name, "real");
	}

	#[test]
	fn multiline_description_continues() {
		let h = "@description Line one.\nLine two.\n@param x string The x\n";
		let meta = parse_header(h, Path::new("/tmp/x"), "x").unwrap();
		assert_eq!(meta.description, "Line one.\nLine two.");
		assert_eq!(meta.params.len(), 1);
	}

	#[test]
	fn header_extraction_skips_shebang_and_stops_at_code() {
		let src = "#!/usr/bin/env bash\n# @description Hi.\n# @param a string the a\necho hello\n# not part of header\n";
		let h = extract_header(src);
		assert!(h.contains("@description Hi."));
		assert!(h.contains("@param a"));
		assert!(!h.contains("not part of header"));
	}

	#[test]
	fn slash_slash_comments_work() {
		let src = "#!/usr/bin/env node\n// @description JS tool\n// @param msg string The message\nconsole.log('hi')\n";
		let h = extract_header(src);
		let meta = parse_header(&h, Path::new("/tmp/x"), "x").unwrap();
		assert_eq!(meta.description, "JS tool");
		assert_eq!(meta.params[0].name, "msg");
	}

	#[test]
	fn missing_description_errors() {
		let h = "@param x string just a thing\n";
		assert!(parse_header(h, Path::new("/tmp/x"), "x").is_err());
	}

	#[test]
	fn invalid_names_rejected() {
		assert!(!is_valid_tool_name(""));
		assert!(!is_valid_tool_name(".hidden"));
		assert!(!is_valid_tool_name("-leading-dash"));
		assert!(!is_valid_tool_name("has space"));
		assert!(!is_valid_tool_name("dot.ext"));
		assert!(is_valid_tool_name("toola"));
		assert!(is_valid_tool_name("tool_b"));
		assert!(is_valid_tool_name("tool-3"));
	}

	#[test]
	fn type_aliases_normalize() {
		assert_eq!(normalize_type("str"), "string");
		assert_eq!(normalize_type("INT"), "integer");
		assert_eq!(normalize_type("bool"), "boolean");
		assert_eq!(normalize_type("unknown"), "string");
	}

	#[test]
	fn to_function_builds_schema() {
		let meta = LocalToolMeta {
			name: "doit".into(),
			description: "Do it.".into(),
			params: vec![
				ParamDef {
					name: "a".into(),
					ty: "string".into(),
					description: "the a".into(),
					required: true,
				},
				ParamDef {
					name: "b".into(),
					ty: "integer".into(),
					description: "the b".into(),
					required: false,
				},
			],
			path: PathBuf::from("/tmp/doit"),
		};
		let f = meta.to_function();
		assert_eq!(f.name, "doit");
		let req = f.parameters["required"].as_array().unwrap();
		assert_eq!(req.len(), 1);
		assert_eq!(req[0], "a");
		assert_eq!(f.parameters["properties"]["a"]["type"], "string");
		assert_eq!(f.parameters["properties"]["b"]["type"], "integer");
	}
}
