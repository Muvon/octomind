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

//! The `code` tool: the model reaches the tools of external MCP servers by
//! writing a script instead of emitting one JSON tool call per step.
//!
//! What the model gets from it:
//! - The request no longer carries the full schema and description of every
//!   external tool. The `code` tool lists each one as a one-line signature;
//!   `help()` inside a script prints the rest on demand.
//! - One script can make many calls, in sequence or at once, and feed one
//!   result into the next call. Intermediate results stay in the script; only
//!   what it prints or returns enters the conversation.
//! - A wrong script does nothing: it is type-checked against the tool
//!   signatures before any of it runs ([`check`]).
//!
//! What stays as it was: the servers, their transport and config, and what
//! happens to each call. A call made by a script passes the same guardrails
//! and plugin checks as a call made directly, and runs through the same
//! executor.
//!
//! Which tools go through scripts is decided by `[mcp] direct_tools`; builtin
//! tools are always sent to the model as tool definitions.

pub mod catalog;
pub mod check;
pub mod engine;
pub mod schema;

#[cfg(test)]
#[path = "fixtures_tests.rs"]
mod fixtures;

use crate::config::Config;
use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use catalog::Catalog;
use engine::{CallFuture, Host, Run};
use parking_lot::Mutex;
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::watch;

pub const TOOL_NAME: &str = "code";

/// What a script that printed and returned nothing sends back, so the model
/// can tell "ran, no output" from a lost result.
const NO_OUTPUT: &str = "(the script finished without printing or returning anything)";

/// How the TypeScript compiler words a name that is not declared (TS2304,
/// and TS2552 when it has a suggestion), and a value it could not type
/// (TS7006 and its relatives).
const UNKNOWN_NAME: &str = "Cannot find name '";
const UNTYPED_VALUE: &str = "implicitly has an 'any' type";

const DESCRIPTION: &str = "Run a script that calls the tools listed under \"Tools\" below: one call, or many with loops, conditions and `Promise.all`. Only what the script prints or returns comes back to you, so filter and summarize large results inside the script instead of returning them whole.\n\
\n\
- Language: TypeScript (plain JavaScript is valid TypeScript). Top-level `await` and `return` work.\n\
- A tool is an async function that takes one object of arguments. Call it by the name written in the list and `await` it. It resolves to the tool's text output; use `JSON.parse` when that text is JSON. A tool that fails throws an `Error` carrying the tool's message.\n\
- The value of the last expression, or of `return`, is the result. `print(...)` adds lines to it.\n\
- `help(name)` prints a tool's full description and arguments when given a name from the list, and every tool of a server with a summary when given only the part before the dot. Read it before the first use of a tool whose arguments the list does not make obvious.\n\
- `input` is the `input` argument of this call. Put long or multi-line text (file content, patches) there instead of quoting it in the script, and pass it on: `{content: input.content}`.\n\
- Before it runs, the script is type-checked in strict mode against the tool signatures. An unknown tool, a wrong or missing argument, a missing `await`, or a text result used as a list rejects the whole script, and no tool is called. Strict mode also wants a type on every parameter of a function you write: `(path: string) => ...`.\n\
- No imports, files or network: tools are the only way out. `sleep(ms)` waits.";

/// The `code` tool as the core server declares it. The list of tools a
/// script can call is added per request by [`split`].
pub fn get_code_function() -> McpFunction {
	McpFunction {
		name: TOOL_NAME.to_string(),
		description: DESCRIPTION.to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"script": {
					"type": "string",
					"description": "TypeScript to run."
				},
				"input": {
					"type": "object",
					"description": "Data for the script, readable there as `input`."
				}
			},
			"required": ["script"],
			"additionalProperties": false
		}),
		annotations: None,
	}
}

/// Whether `pattern` (from `[mcp] direct_tools`) names this tool:
/// `server:tool`, `server:*`, `tool` or `prefix*`.
fn pattern_matches(pattern: &str, server: &str, tool: &str) -> bool {
	let tool_pattern = match pattern.split_once(':') {
		Some((named, _)) if named != server => return false,
		Some((_, tool_pattern)) => tool_pattern,
		None => pattern,
	};
	crate::mcp::is_tool_allowed_by_patterns(tool, &[tool_pattern.to_string()])
}

/// Split the session's tools into the definitions sent to the model and the
/// catalog of tools reached through scripts.
///
/// `functions` pairs each tool with the external server it comes from (`None`
/// for builtin, agent and project-local tools, which always stay definitions).
/// An external tool goes to the catalog unless `[mcp] direct_tools` names it.
/// Without the `code` tool in the list (the role does not grant it) nothing
/// can run a script, so every tool stays a definition.
pub fn split(config: &Config, functions: Vec<(Option<String>, McpFunction)>) -> (Vec<McpFunction>, Catalog) {
	// The builtin one: an external server may have a tool of the same name.
	let scripts_available = functions
		.iter()
		.any(|(server, function)| server.is_none() && function.name == TOOL_NAME);
	if !scripts_available {
		let definitions = functions.into_iter().map(|(_, function)| function).collect();
		return (definitions, Catalog::default());
	}
	let mut definitions = Vec::with_capacity(functions.len());
	let mut catalog = Catalog::default();
	for (server, function) in functions {
		let scripted = server.as_deref().filter(|server| {
			!config
				.mcp
				.direct_tools
				.iter()
				.any(|pattern| pattern_matches(pattern, server, &function.name))
		});
		match scripted {
			// Two tools of one server can map to the same script name; the
			// second one stays a definition so it remains callable.
			Some(server)
				if catalog.add(
					server,
					&function.name,
					&function.description,
					function.parameters.clone(),
				) => {}
			_ => definitions.push(function),
		}
	}
	if catalog.is_empty() {
		definitions.retain(|function| function.name != TOOL_NAME);
	} else if let Some(code) = definitions.iter_mut().find(|function| function.name == TOOL_NAME) {
		// The example names a tool of this session: a made-up name in the
		// text gets copied into scripts.
		let server = &catalog.servers[0];
		code.description = format!(
			"{}\n\nTools, called like `await {}.{}({{...}})`:\n{}",
			code.description,
			server.ident,
			server.tools[0].ident,
			catalog.index()
		);
	}
	(definitions, catalog)
}

/// Settle the calls that name a script tool as if it were a tool of its own,
/// by the name in the list (`octofs.view`) or by the bare tool name.
///
/// Models do this, small ones on most steps, and refusing costs a round each
/// time. The model was sent no schema for such a call, so its arguments are
/// guesses: numbers and booleans arrive as text, one argument is taken for
/// another. So the arguments are read the way the tool's schema types them
/// and checked against it. A call that fits runs as the tool it means,
/// under the tool's real name, so every later check sees that name. A call
/// that does not fit is not run: its answer (the second value, `None` for
/// every other call) says what is wrong.
///
/// Either way the model is shown what `help` prints for the tool, once for
/// as long as its context keeps it: a schema that accepts a call does not
/// stop a model from using an argument for the wrong purpose, and the one
/// line the tool has in the list is not enough to use it step after step.
/// So a tool's full description enters the context when the model starts
/// to call the tool directly, and not before.
pub async fn settle_direct_calls(config: &Config, calls: Vec<McpToolCall>) -> (Vec<McpToolCall>, Vec<Option<String>>) {
	// Only a tool of an external server, or a name with a server in front of
	// it, can be a script tool. Most batches hold neither, and then the list
	// of script tools is not built.
	let may_be_scripted = |call: &McpToolCall| match crate::mcp::tool_map::get_server_for_tool(&call.tool_name) {
		Some(server) => server.connection_type() != crate::config::McpConnectionType::Builtin,
		None => call.tool_name.contains('.'),
	};
	if !calls.iter().any(may_be_scripted) {
		let answers = vec![None; calls.len()];
		return (calls, answers);
	}
	let (_, catalog) = split(config, crate::mcp::sourced_functions(config).await);
	let mut answers = Vec::with_capacity(calls.len());
	let calls = calls
		.into_iter()
		.map(|mut call| {
			let Some((server, tool)) = catalog.find(&call.tool_name) else {
				answers.push(None);
				return call;
			};
			if call.parameters.is_null() {
				call.parameters = json!({});
			}
			schema::coerce(&tool.schema, &mut call.parameters);
			let violations = schema::validate(&tool.schema, &call.parameters);
			let script_name = format!("{}.{}", server.ident, tool.ident);
			let unseen = crate::session::context::note_tool_help_shown(&script_name);
			if violations.is_empty() {
				if unseen {
					crate::mcp::hint_accumulator::push_hint(&format!(
						"You called `{}` without its description. Read it before the next call:\n{}",
						call.tool_name,
						catalog.help(&script_name)
					));
				}
				call.tool_name = tool.name.clone();
				answers.push(None);
			} else {
				let mut answer = format!("`{}` was not called: {}", call.tool_name, violations.join("; "));
				if unseen {
					answer.push_str("\n\n");
					answer.push_str(&catalog.help(&script_name));
				}
				answers.push(Some(answer));
			}
			call
		})
		.collect();
	(calls, answers)
}

/// Whether a script mentions a tool from `writers` (script names of the
/// tools that may change something), as a word anywhere in its text.
///
/// A tool can be reached in more ways than a call written out in full
/// (`const {write} = files`, `files["write"]`), so any mention counts: a
/// variable that happens to share a tool's name only errs towards "may have".
fn names_any(script: &str, writers: &[String]) -> bool {
	script
		.split(|character: char| !(character.is_alphanumeric() || character == '_' || character == '$'))
		.any(|word| writers.iter().any(|writer| writer == word))
}

/// Whether a `code` call may have changed something: its script mentions at
/// least one external tool that is not declared read-only.
///
/// The rest of the session asks this about every tool call to decide whether
/// the working tree may have moved. For a script the answer comes from the
/// tools it names, not from what it did: it is asked again when a session is
/// resumed, long after the run is gone. A mentioned tool counts even when the
/// script never reached the call, so the answer errs towards "may have".
pub fn can_write(parameters: &Value) -> bool {
	let Some(script) = parameters.get("script").and_then(Value::as_str) else {
		return false;
	};
	let writers: Vec<String> = crate::mcp::tool_map::get_all_tool_names()
		.into_iter()
		.filter(|tool| crate::mcp::tool_shape::tool_read_only_hint(tool) != Some(true))
		.filter(|tool| {
			crate::mcp::tool_map::get_server_for_tool(tool)
				.is_some_and(|server| server.connection_type() != crate::config::McpConnectionType::Builtin)
		})
		.map(|tool| catalog::js_ident(&tool))
		.collect();
	names_any(script, &writers)
}

/// The rejection the model reads when a script fails the type check.
///
/// The compiler's messages are completed with what it cannot know: a tool
/// that does not exist names the server's type but not its tools, and a name
/// that does not exist says nothing about which servers there are. A value
/// read from an unknown name has no type, so every callback that touches it
/// is reported as untyped too; those follow-on problems are left out, since
/// fixing the name fixes them.
fn rejection(catalog: &Catalog, problems: &[check::Problem]) -> String {
	let unknown_name = problems.iter().any(|problem| problem.message.contains(UNKNOWN_NAME));
	let shown: Vec<check::Problem> = problems
		.iter()
		.filter(|problem| !(unknown_name && problem.message.contains(UNTYPED_VALUE)))
		.cloned()
		.collect();
	let mut text = check::report(&shown);
	if unknown_name {
		let servers: Vec<&str> = catalog.servers.iter().map(|server| server.ident.as_str()).collect();
		text.push_str(&format!("\nServers: {}", servers.join(", ")));
	}
	for server in &catalog.servers {
		let unknown_tool = format!("does not exist on type '{}'", server.type_name());
		if problems.iter().any(|problem| problem.message.contains(&unknown_tool)) {
			text.push_str(&format!(
				"\nTools of {}: {}",
				server.ident,
				server.tool_idents().join(", ")
			));
		}
	}
	text
}

tokio::task_local! {
	/// Present while a tool call a script makes is being executed.
	static SCRIPT_CALL: ();
}

/// Run `call`, the execution of a tool call a script makes, marked as such.
pub(crate) async fn as_script_call<F: std::future::Future>(call: F) -> F::Output {
	SCRIPT_CALL.scope((), call).await
}

/// Whether the tool call being executed is made by a script.
///
/// MCP servers are told with each such call (`experimental.session.script`,
/// see `mcp/client.rs`): its result goes to the script, and the model sees
/// only what the script prints. A server that tracks what the model has
/// seen must not count a script's call as the model's.
pub fn is_script_call() -> bool {
	SCRIPT_CALL.try_with(|_| ()).is_ok()
}

/// The session a script runs in: everything one of its tool calls needs to be
/// treated like a call the model made directly.
struct Session {
	config: Config,
	cancellation: Option<watch::Receiver<bool>>,
	session_id: String,
	workdir: Option<PathBuf>,
	/// Id of the `code` call; each call a script makes gets an id under it.
	call_id: String,
	/// Calls the script started. Calls made at once are admitted at once, so
	/// each takes its number before it waits for anything.
	started: AtomicUsize,
	/// Tools that ran, `server.tool` in call order.
	ran: Mutex<Vec<String>>,
	/// Images returned by the tools, passed on with the script's result.
	images: Mutex<Vec<ContentBlock>>,
}

impl Session {
	async fn dispatch(self: Arc<Self>, server: String, tool: String, arguments: Value) -> Result<String, String> {
		// Tool names are unique in a session, but a script names the server too.
		if crate::mcp::tool_map::get_tool_server_name(&tool).as_deref() != Some(server.as_str()) {
			return Err(format!("`{tool}` is not a tool of server `{server}`"));
		}
		let label = format!("{server}.{tool}");
		let number = self.started.fetch_add(1, Ordering::Relaxed) + 1;
		let call = McpToolCall {
			tool_name: tool,
			parameters: arguments,
			tool_id: format!("{}:{}", self.call_id, number),
		};
		let verdicts = crate::session::chat::response::tool_execution::admit_tool_batch(
			&self.session_id,
			&self.config,
			std::slice::from_ref(&call),
			None,
		)
		.await;
		if let Some(Some(denial)) = verdicts.into_iter().next() {
			return Err(denial);
		}
		self.ran.lock().push(label.clone());
		crate::log_debug!("code: script calls {} ({})", call.tool_name, call.tool_id);
		let execution = crate::mcp::execute_tool_call(&call, &self.config, self.cancellation.clone());
		match as_script_call(execution).await {
			Ok((result, _)) => {
				self.images.lock().extend(
					result
						.result
						.content
						.iter()
						.filter(|block| matches!(block, ContentBlock::Image(_)))
						.cloned(),
				);
				let text = result.extract_content();
				if result.is_error() {
					// The script stops here unless it catches this, so the
					// message must say which of its calls failed.
					Err(format!("{label} failed: {text}"))
				} else {
					Ok(text)
				}
			}
			Err(error) => Err(format!("{label} failed: {error}")),
		}
	}

	/// The tools that ran, each with how often: `files.read x3, files.write`.
	fn ran_summary(&self) -> String {
		let mut counts: Vec<(String, usize)> = Vec::new();
		for label in self.ran.lock().iter() {
			match counts.iter_mut().find(|(known, _)| known == label) {
				Some((_, count)) => *count += 1,
				None => counts.push((label.clone(), 1)),
			}
		}
		counts
			.into_iter()
			.map(
				|(label, count)| {
					if count == 1 {
						label
					} else {
						format!("{label} x{count}")
					}
				},
			)
			.collect::<Vec<_>>()
			.join(", ")
	}
}

/// [`Host`] over a shared [`Session`]: a script's calls outlive the borrow of
/// the host that started them.
struct SessionHost(Arc<Session>);

impl Host for SessionHost {
	fn call(&self, server: &str, tool: &str, arguments: Value) -> CallFuture {
		let session = self.0.clone();
		let (server, tool) = (server.to_string(), tool.to_string());
		Box::pin(async move {
			// The engine polls this future, so the task-locals of the task
			// that started the script are restated here rather than assumed.
			let session_id = session.session_id.clone();
			let workdir = session.workdir.clone();
			let work = crate::session::context::with_session_id(session_id, session.dispatch(server, tool, arguments));
			match workdir {
				Some(workdir) => crate::mcp::workdir::with_working_directory(workdir, work).await,
				None => work.await,
			}
		})
	}

	fn cancelled(&self) -> bool {
		self.0.cancellation.as_ref().is_some_and(|token| *token.borrow())
	}
}

/// Run one `code` call: type-check the script, run it, and return what it
/// printed and returned.
pub async fn execute(
	call: &McpToolCall,
	config: &Config,
	cancellation: Option<watch::Receiver<bool>>,
) -> McpToolResult {
	let refuse = |message: String| McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), message);
	let script = match call.parameters.get("script") {
		Some(Value::String(script)) if !script.trim().is_empty() => script,
		_ => return refuse("script: non-empty string required".to_string()),
	};
	let input = call.parameters.get("input").filter(|input| !input.is_null());
	// The guardrails and plugin checks every tool call passes belong to a
	// session; without one a script's calls would run unchecked.
	let Some(session_id) = crate::session::context::current_session_id() else {
		return refuse("scripts run only inside a session".to_string());
	};
	let (_, catalog) = split(config, crate::mcp::sourced_functions(config).await);
	if catalog.is_empty() {
		return refuse("no tool is available to scripts in this session".to_string());
	}
	let checked = match check::check(script, &catalog.declarations(input)).await {
		Ok(checked) => checked,
		Err(problems) => return refuse(rejection(&catalog, &problems)),
	};

	let session = Arc::new(Session {
		config: config.clone(),
		cancellation,
		session_id,
		workdir: crate::mcp::workdir::working_directory_override(),
		call_id: call.tool_id.clone(),
		started: AtomicUsize::new(0),
		ran: Mutex::new(Vec::new()),
		images: Mutex::new(Vec::new()),
	});
	let Run { output, error } = engine::run(
		checked,
		input,
		Arc::new(catalog),
		Arc::new(SessionHost(session.clone())),
	)
	.await;

	let failed = error.is_some();
	let text = match error {
		// Why the script stopped comes first: a long result is cut from its
		// end, and a failed call is shown by its first line. A script that
		// stops half way has done part of its work; the model must know
		// which part before it writes the next one.
		Some(mut text) => {
			let made = session.ran_summary();
			text.push_str(&if made.is_empty() {
				"\nNo tool call was made.".to_string()
			} else {
				format!("\nTool calls made before the script stopped: {made}")
			});
			if !output.is_empty() {
				text.push_str("\n\nPrinted before the script stopped:\n");
				text.push_str(&output);
			}
			text
		}
		None if output.is_empty() => NO_OUTPUT.to_string(),
		None => output,
	};
	let mut content = vec![ContentBlock::text(text)];
	content.append(&mut session.images.lock());
	McpToolResult {
		tool_name: call.tool_name.clone(),
		tool_id: call.tool_id.clone(),
		result: if failed {
			CallToolResult::error(content)
		} else {
			CallToolResult::success(content)
		},
	}
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "e2e_tests.rs"]
mod e2e_tests;
