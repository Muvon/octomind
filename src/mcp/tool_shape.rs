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

//! What a tool call is, read from the tool's own schema and annotations: does
//! it write, does it run a free-form command, which of its parameters name
//! paths. Tool inventories register their facts here when they arrive.

use crate::mcp::McpToolResult;

/// The command a call executed, when it executed one. A command execution can
/// provide evidence even when it also mutates state, so its output is kept.
/// A parameter the tool's own schema declares as a command
/// ([`register_tool_command_params`]) counts whatever else the call did — an
/// edit that also ran a check returned that check's output. Otherwise the
/// `command` parameter decides: registered runners are identified by their
/// schema; unregistered tools retain the conservative non-mutation shape
/// fallback.
pub fn executed_command<'a>(tool: &str, parameters: &'a serde_json::Value) -> Option<&'a str> {
	let declared = tool_command_params().read().ok().and_then(|declared| {
		declared
			.get(tool)?
			.iter()
			.find_map(|name| parameters.get(name)?.as_str())
	});
	let cmd = match declared {
		Some(cmd) => cmd,
		None => {
			let cmd = parameters.get("command")?.as_str()?;
			if !executes_free_form_command(tool) && is_mutation_call(tool, parameters) {
				return None;
			}
			cmd
		}
	};
	// Reject empty command strings: they execute nothing and cannot validate
	// completion.
	if cmd.trim().is_empty() {
		return None;
	}
	match crate::mcp::tool_map::get_tool_server_name(tool) {
		Some(server) if matches!(server.as_str(), "core" | "runtime" | "orchestration" | "agent") => None,
		// Unregistered command tool: retain the output as possible evidence.
		_ => Some(cmd),
	}
}

/// Path-like values in a tool call's parameters — the artifact identities a
/// mutation touches and a later read-back can verify. Generic across tools and
/// domains: any non-empty string under a key containing "path" or "file", plus
/// string arrays under such keys, at any depth — a multi-target call nests one
/// path per item (a list of edits, each naming its own file), and a file it
/// skips is invisible to ground truth and read-back alike. No tool names, no
/// extension lists.
pub fn param_paths(parameters: &serde_json::Value) -> Vec<String> {
	let mut out = Vec::new();
	collect_param_paths(parameters, &mut out);
	out
}

fn collect_param_paths(value: &serde_json::Value, out: &mut Vec<String>) {
	match value {
		serde_json::Value::Object(obj) => {
			for (k, v) in obj {
				let kl = k.to_ascii_lowercase();
				if kl.contains("path") || kl.contains("file") {
					match v {
						serde_json::Value::String(s) if !s.trim().is_empty() => out.push(s.clone()),
						serde_json::Value::Array(a) => out.extend(
							a.iter()
								.filter_map(|x| x.as_str())
								.filter(|s| !s.trim().is_empty())
								.map(str::to_string),
						),
						_ => {}
					}
				}
				collect_param_paths(v, out);
			}
		}
		serde_json::Value::Array(items) => items.iter().for_each(|item| collect_param_paths(item, out)),
		_ => {}
	}
}

/// Classify one concrete call. Three signals, strongest first, each answering
/// what it actually knows:
///
/// 1. The tool's own SCHEMA says whether the call selects a fixed operation or
///    executes a free-form command ([`register_tool_command_shape`]). A command
///    runner is write-CAPABLE by construction — `octofs shell` and every other
///    honest runner annotates `readOnlyHint: false` — so its tool-level hint
///    carries no information about the concrete call, and answering from it
///    classified every build, test and validator run as a mutation. What the
///    runner's command actually did to the tree has to be observed, not
///    guessed.
/// 2. Otherwise the MCP `readOnlyHint` annotation, which for a single-purpose
///    tool (an editor, a reader) describes the call as well as the tool.
/// 3. Otherwise normalized intent tokens — the compatibility fallback for tools
///    that ship no annotation, and the only signal available for a runner's
///    concrete command.
pub fn is_mutation_call(tool: &str, parameters: &serde_json::Value) -> bool {
	if executes_free_form_command(tool) {
		// The command only. A runner's NAME describes the tool the same way its
		// `readOnlyHint` does — `deploy_shell` running `ls` changes nothing — so
		// neither can answer for the concrete call.
		return command_intent_is_mutation(parameters);
	}
	if let Some(read_only) = tool_read_only_hint(tool) {
		return !read_only;
	}
	has_explicit_mutation_intent(tool, parameters)
}

/// High-confidence mutation signal from the concrete call itself, ignoring a
/// tool-level `readOnly=false` capability hint: a generic shell/browser/API
/// tool may be capable of writes while the concrete call is only gathering
/// evidence, and classifying that read as a mutation would be a false positive.
fn has_explicit_mutation_intent(tool: &str, parameters: &serde_json::Value) -> bool {
	contains_mutation_intent(tool) || command_intent_is_mutation(parameters)
}

/// Mutation intent carried by the call's own operation parameters, with the
/// tool's identity left out of it.
fn command_intent_is_mutation(parameters: &serde_json::Value) -> bool {
	["command", "action", "operation"]
		.iter()
		.filter_map(|key| parameters.get(key).and_then(|value| value.as_str()))
		.any(contains_mutation_intent)
}

fn contains_mutation_intent(value: &str) -> bool {
	let mut normalized = String::with_capacity(value.len());
	let mut previous_lowercase = false;
	for character in value.chars() {
		if character.is_ascii_uppercase() && previous_lowercase {
			normalized.push(' ');
		}
		if character.is_ascii_alphanumeric() {
			normalized.push(character.to_ascii_lowercase());
			previous_lowercase = character.is_ascii_lowercase();
		} else {
			normalized.push(' ');
			previous_lowercase = false;
		}
	}
	let intents = [
		"write", "edit", "create", "replace", "apply", "insert", "delete", "remove", "patch", "mkdir", "rename",
		"move", "update", "set", "send", "publish", "post", "upload", "schedule", "book", "approve", "reject",
		"cancel", "deploy", "install", "commit", "push", "merge",
	];
	normalized.split_whitespace().any(|token| intents.contains(&token))
}

fn tool_read_only_hints() -> &'static std::sync::RwLock<std::collections::HashMap<String, bool>> {
	static HINTS: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, bool>>> =
		std::sync::OnceLock::new();
	HINTS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Register the standard MCP read-only hint when an external tool inventory is
/// received. Per the MCP specification this is a hint, not an authorization or
/// safety boundary; it drives progress classification and execution ordering.
pub fn register_tool_read_only_hint(tool: &str, read_only: Option<bool>) {
	if let Ok(mut hints) = tool_read_only_hints().write() {
		if let Some(read_only) = read_only {
			hints.insert(tool.to_string(), read_only);
		} else {
			// A refreshed inventory without a hint must not inherit an old read-only promise.
			hints.remove(tool);
		}
	}
}

pub fn tool_read_only_hint(tool: &str) -> Option<bool> {
	tool_read_only_hints().read().ok()?.get(tool).copied()
}

fn tool_command_shapes() -> &'static std::sync::RwLock<std::collections::HashMap<String, bool>> {
	static SHAPES: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, bool>>> =
		std::sync::OnceLock::new();
	SHAPES.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Register what a tool's `command` parameter IS, read off the tool's own JSON
/// schema when its inventory arrives (see [`command_param_is_free_form`]).
/// `true` = it executes a free-form command (a runner: shells, remote
/// executors, domain validators); `false` = it selects one of a fixed set of
/// operations (an editor's `command: "str_replace"`). The distinction is
/// structural — no tool names, no program names, no keyword lists — and it is
/// what lets the runtime tell a build/test run apart from an edit when both
/// arrive as a string under the same parameter name.
pub fn register_tool_command_shape(tool: &str, free_form: bool) {
	if let Ok(mut shapes) = tool_command_shapes().write() {
		shapes.insert(tool.to_string(), free_form);
	}
}

/// Does this tool execute a free-form command? `false` when unregistered: a
/// tool whose schema the runtime never saw keeps the annotation-first
/// classification it had before.
pub fn executes_free_form_command(tool: &str) -> bool {
	tool_command_shapes()
		.read()
		.ok()
		.and_then(|shapes| shapes.get(tool).copied())
		.unwrap_or(false)
}

/// Is the `command` property of this tool's input schema a free-form string —
/// a command to execute — rather than a fixed operation vocabulary? Schema
/// facts only: a plain string type with no `enum`/`const` constraint and no
/// `$ref` to a named variant type. `anyOf`/`oneOf` branches are searched too,
/// so a runner that accepts either a string or an argv array still counts.
/// Absent or unreadable `command` → `false`; the caller then falls back to the
/// tool's annotation.
pub fn command_param_is_free_form(schema: &serde_json::Value) -> bool {
	let Some(param) = schema.get("properties").and_then(|p| p.get("command")) else {
		return false;
	};
	fn unconstrained_string(node: &serde_json::Value) -> bool {
		if node.get("enum").is_some() || node.get("const").is_some() || node.get("$ref").is_some() {
			return false;
		}
		match node.get("type") {
			Some(serde_json::Value::String(t)) => t == "string",
			// Nullable/union declarations render the type as a list.
			Some(serde_json::Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("string")),
			_ => false,
		}
	}
	if unconstrained_string(param) {
		return true;
	}
	["anyOf", "oneOf"]
		.iter()
		.filter_map(|key| param.get(key).and_then(|v| v.as_array()))
		.flatten()
		.any(unconstrained_string)
}

/// JSON Schema `contentMediaType` a tool puts on a parameter it executes as a
/// shell command.
const COMMAND_MEDIA_TYPE: &str = "application/x-sh";

fn tool_command_params() -> &'static std::sync::RwLock<std::collections::HashMap<String, Vec<String>>> {
	static PARAMS: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, Vec<String>>>> =
		std::sync::OnceLock::new();
	PARAMS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Register the parameters a tool's own schema declares it executes as a
/// command ([`declared_command_params`]), read when its inventory arrives. An
/// editor that runs a check after writing carries that command beside its
/// operation; the schema is the only place the runtime can learn that such a
/// call executed something, without knowing the tool or the parameter by name.
pub fn register_tool_command_params(tool: &str, params: Vec<String>) {
	if let Ok(mut declared) = tool_command_params().write() {
		// A refreshed inventory replaces the old declaration, an empty one included.
		declared.insert(tool.to_string(), params);
	}
}

fn tool_output_schemas() -> &'static std::sync::RwLock<std::collections::HashMap<String, serde_json::Value>> {
	static SCHEMAS: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, serde_json::Value>>> =
		std::sync::OnceLock::new();
	SCHEMAS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Register the MCP `outputSchema` a tool declares, read when its inventory
/// arrives. A tool with one returns structured content of that shape, which
/// a script then gets as a typed value instead of text.
pub fn register_tool_output_schema(tool: &str, schema: Option<serde_json::Value>) {
	if let Ok(mut schemas) = tool_output_schemas().write() {
		match schema {
			Some(schema) => {
				schemas.insert(tool.to_string(), schema);
			}
			// A refreshed inventory without one must not keep an old promise.
			None => {
				schemas.remove(tool);
			}
		}
	}
}

pub fn tool_output_schema(tool: &str) -> Option<serde_json::Value> {
	tool_output_schemas().read().ok()?.get(tool).cloned()
}

/// Top-level parameters this input schema marks as an executed command.
/// `contentMediaType` is the standard JSON Schema annotation for what a string
/// holds, so the mark is a schema fact like the `command` shape above: no tool
/// names, no parameter names, no description wording.
pub fn declared_command_params(schema: &serde_json::Value) -> Vec<String> {
	let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
		return Vec::new();
	};
	properties
		.iter()
		.filter(|(_, param)| param.get("contentMediaType").and_then(|m| m.as_str()) == Some(COMMAND_MEDIA_TYPE))
		.map(|(name, _)| name.clone())
		.collect()
}

/// Rewriting a result's text payload (dedup, the hard size cap, a plugin's
/// replacement) must not touch rich MCP results: flattening images/resources
/// or structured content and then reconstructing only text would silently
/// change the tool's protocol value.
pub(crate) fn is_plain_text_result(result: &McpToolResult) -> bool {
	result
		.result
		.structured_content
		.as_ref()
		.is_none_or(serde_json::Value::is_null)
		&& result
			.result
			.content
			.iter()
			.all(|block| matches!(block, rmcp::model::ContentBlock::Text(_)))
}

#[cfg(test)]
#[path = "tool_shape_tests.rs"]
mod tests;
