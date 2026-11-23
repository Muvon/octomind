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
//! `[mcp] code_mode` switches it on for every session: each external tool is
//! then reached through scripts only, and builtin tools stay tool definitions.
//! Off, every tool is a tool definition and there is no `code` tool.

pub mod catalog;
pub mod check;
pub mod engine;
pub mod schema;

#[cfg(test)]
#[path = "fixtures_tests.rs"]
mod fixtures;

use crate::config::Config;
use crate::mcp::{McpFunction, McpToolCall, McpToolResult, ScriptCall};
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

/// The tool that finds script tools by what they do, sent with `code` when
/// the list of tools is too long to send whole.
pub const SEARCH_TOOL_NAME: &str = "search_tools";

/// What a script that printed and returned nothing sends back, so the model
/// can tell "ran, no output" from a lost result.
const NO_OUTPUT: &str = "(the script finished without printing or returning anything)";

/// How the TypeScript compiler words a name that is not declared (TS2304,
/// and TS2552 when it has a suggestion), and a value it could not type
/// (TS7006 and its relatives).
const UNKNOWN_NAME: &str = "Cannot find name '";
const UNTYPED_VALUE: &str = "implicitly has an 'any' type";

// The first paragraph says why one script should hold many calls. Without the
// reason models wrote one call per script: 19 of 23 scripts in the Sonnet file
// tasks of 2026-11-18, none with `Promise.all`.
const DESCRIPTION: &str = "Run a script that calls the tools listed under \"Tools\" below. Every call of this tool is a round trip that sends the whole conversation again, so one script does everything you can already write: calls that do not depend on each other go together into one `await Promise.all([...])`, calls that do go one after another, and a change goes with the command that checks it. Write a second script only where you have to read a result before you can write the next call. Only what the script prints or returns comes back to you, so filter and summarize large results inside the script instead of returning them whole.\n\
\n\
- Language: TypeScript (plain JavaScript is valid TypeScript). Top-level `await` and `return` work.\n\
- A tool is an async function that takes one object of arguments. Call it by the name written in the list and `await` it. It resolves to the tool's text output (use `JSON.parse` when that text is JSON), or to a value of the type the list shows after `->`. A tool that fails throws an `Error` carrying the tool's message.\n\
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

/// The `search_tools` tool as the core server declares it.
pub fn get_search_function() -> McpFunction {
	McpFunction {
		name: SEARCH_TOOL_NAME.to_string(),
		description: "Find the tools a `code` script can call when the list is too long to send: describe what you want done, in your own words or by name. Prints the five tools that match best, each as the list shows it, ready to call in the next script.".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"query": {
					"type": "string",
					"description": "What the tool should do, such as \"delete a file\" or \"list open issues\"."
				}
			},
			"required": ["query"],
			"additionalProperties": false
		}),
		annotations: Some(rmcp::model::ToolAnnotations::new().read_only(true).open_world(false)),
	}
}

/// Start embedding the catalog's tools for `search_tools`, once per distinct
/// list: the list is rebuilt for every request.
pub fn prewarm_search(catalog: &Catalog) {
	static PREWARMED: std::sync::OnceLock<Mutex<std::collections::HashSet<u64>>> = std::sync::OnceLock::new();
	let key = {
		use std::hash::{Hash, Hasher};
		let mut hasher = std::collections::hash_map::DefaultHasher::new();
		catalog.index().hash(&mut hasher);
		hasher.finish()
	};
	if PREWARMED.get_or_init(Default::default).lock().insert(key) {
		crate::embeddings::prewarm(catalog.search_texts());
	}
}

/// Run one `search_tools` call over the session's script tools.
pub async fn execute_search(call: &McpToolCall, config: &Config) -> McpToolResult {
	let query = match call.parameters.get("query") {
		Some(Value::String(query)) if !query.trim().is_empty() => query,
		_ => {
			return McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"query: non-empty string required".to_string(),
			)
		}
	};
	let (_, catalog) = split(config, crate::mcp::sourced_functions(config).await);
	match catalog.search(query).await {
		Ok(found) => McpToolResult::success(call.tool_name.clone(), call.tool_id.clone(), found),
		Err(error) => McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("search_tools failed: {error:#}"),
		),
	}
}

/// Split the session's tools into the definitions sent to the model and the
/// catalog of tools reached through scripts.
///
/// `functions` pairs each tool with the external server it comes from (`None`
/// for builtin, agent and project-local tools, which always stay definitions).
/// In code mode every external tool goes to the catalog, and the `code` tool
/// (present whenever there is one, see `sourced_functions`) lists them.
pub fn split(config: &Config, functions: Vec<(Option<String>, McpFunction)>) -> (Vec<McpFunction>, Catalog) {
	if !config.mcp.code_mode {
		let definitions = functions.into_iter().map(|(_, function)| function).collect();
		return (definitions, Catalog::default());
	}
	let mut definitions = Vec::with_capacity(functions.len());
	let mut catalog = Catalog::default();
	for (server, function) in functions {
		match server {
			Some(server) => catalog.add(
				&server,
				&function.name,
				&function.description,
				function.parameters.clone(),
			),
			None => definitions.push(function),
		}
	}
	if let Some(code) = definitions.iter_mut().find(|function| function.name == TOOL_NAME) {
		// The example names a tool of this session: a made-up name in the
		// text gets copied into scripts.
		let server = &catalog.servers[0];
		let index = catalog.index();
		// The list goes out with every request. One longer than a tool result
		// may be costs more each turn than the scripts that look tools up.
		let too_long = config.mcp_response_tokens_threshold > 0
			&& crate::session::estimate_tokens(&index) > config.mcp_response_tokens_threshold;
		let collapsed = too_long.then(|| {
			format!(
				"{}\n\nServers, their tools called like `await {}.{}({{...}})`. The full list is too long to send: find tools with the `{}` tool or with `help(\"server\")` in a script, then call them.\n{}",
				code.description,
				server.ident,
				server.tools[0].ident,
				SEARCH_TOOL_NAME,
				catalog.server_index()
			)
		});
		code.description = collapsed.clone().unwrap_or_else(|| {
			format!(
				"{}\n\nTools, called like `await {}.{}({{...}})`:\n{}",
				code.description, server.ident, server.tools[0].ident, index
			)
		});
		if collapsed.is_some() {
			definitions.push(get_search_function());
		}
	}
	(definitions, catalog)
}

/// Turn the calls that name a script tool as if it were a tool of its own
/// into the `code` calls they mean, so every call of an external tool runs
/// through a script: type-checked, marked as a script call, and shown and
/// recorded as the script it is.
///
/// The model may call a tool by the name in the list (`octofs.view`), by the
/// bare tool name, or a server by its name (`octofs`). It was sent no schema
/// for such a call, so text that spells a number, a boolean or a list is read
/// the way the tool's schema types it before it goes into the script; the type
/// check answers what still does not fit. A server's name becomes
/// `help("server")`, which lists its tools.
///
/// The first time a tool is called this way in a context epoch, the model is
/// shown what `help` prints for it: the one line the tool has in the list is
/// not enough to use it step after step.
pub async fn script_direct_calls(config: &Config, calls: Vec<McpToolCall>) -> Vec<McpToolCall> {
	// Only a tool of an external server, or a name no tool has (`octofs.view`,
	// or a server's own name), can concern the script tools. Most batches hold
	// neither, and then the list of script tools is not built.
	let may_be_scripted = |call: &McpToolCall| {
		crate::mcp::tool_map::get_server_for_tool(&call.tool_name)
			.is_none_or(|server| server.connection_type() != crate::config::McpConnectionType::Builtin)
	};
	if !calls.iter().any(may_be_scripted) {
		return calls;
	}
	let (_, catalog) = split(config, crate::mcp::sourced_functions(config).await);
	calls
		.into_iter()
		.map(|mut call| {
			let script = if let Some((server, tool)) = catalog.find(&call.tool_name) {
				if !call.parameters.is_object() {
					call.parameters = json!({});
				}
				schema::coerce(&tool.schema, &mut call.parameters);
				let script_name = format!("{}.{}", server.ident, tool.ident);
				if crate::session::context::note_tool_help_shown(&script_name) {
					crate::mcp::hint_accumulator::push_hint(&format!(
						"`{}` ran as a `code` script. Call tools inside `code`, several in one script. Its description:\n{}",
						call.tool_name,
						catalog.help(&script_name)
					));
				}
				format!("return await {script_name}({});", call.parameters)
			} else if let Some(server) = catalog.server(&call.tool_name) {
				format!("help({:?});", server.ident)
			} else {
				return call;
			};
			call.tool_name = TOOL_NAME.to_string();
			call.parameters = json!({ "script": script });
			call
		})
		.collect()
}

/// What the session keeps of the calls a script made, next to the text the
/// script returned: every call, without the output of a read the script did
/// not print. The model never saw that output, and only the round that ran
/// the script needs it. Edits keep theirs: it says what changed where.
pub fn kept_calls(calls: &[ScriptCall], printed: &str) -> Vec<ScriptCall> {
	calls
		.iter()
		.map(|made| {
			let output = made.output.trim();
			let shown = !output.is_empty() && printed.contains(output);
			let edit = crate::mcp::tool_shape::is_mutation_call(&made.tool, &made.arguments);
			if shown || edit || made.error {
				made.clone()
			} else {
				ScriptCall {
					output: String::new(),
					..made.clone()
				}
			}
		})
		.collect()
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
///
/// Not an `async fn`: its state would hold `call` twice, doubling the size of what it wraps.
pub(crate) fn as_script_call<F: std::future::Future>(call: F) -> impl std::future::Future<Output = F::Output> {
	SCRIPT_CALL.scope((), call)
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
	/// Every call the script made, with its output, as each one finished.
	made: Mutex<Vec<(usize, ScriptCall)>>,
	/// The session as the round found it, for the plugins that judge each call.
	round_snapshot: Option<Arc<crate::plugins::payload::RoundSnapshot>>,
	/// Images returned by the tools, passed on with the script's result.
	images: Mutex<Vec<ContentBlock>>,
}

impl Session {
	async fn dispatch(self: Arc<Self>, server: String, tool: String, arguments: Value) -> Result<Value, String> {
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
		let record = |output: &str, error: bool, denied: bool| {
			let made = ScriptCall {
				id: call.tool_id.clone(),
				server: server.clone(),
				tool: call.tool_name.clone(),
				arguments: call.parameters.clone(),
				output: output.to_string(),
				error,
				denied,
			};
			self.made.lock().push((number, made));
		};
		let verdicts = crate::session::chat::response::tool_execution::admit_tool_batch(
			&self.session_id,
			&self.config,
			std::slice::from_ref(&call),
			crate::session::chat::response::tool_execution::CallOrigin::Script {
				code_call_id: &self.call_id,
				snapshot: self.round_snapshot.clone(),
			},
		)
		.await;
		if let Some(Some(denial)) = verdicts.into_iter().next() {
			record(&denial, true, true);
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
				record(&text, result.is_error(), false);
				if result.is_error() {
					// The script stops here unless it catches this, so the
					// message must say which of its calls failed.
					return Err(format!("{label} failed: {text}"));
				}
				// The script was told this tool resolves to its output schema's
				// shape, which only its structured content has.
				if crate::mcp::tool_shape::tool_output_schema(&call.tool_name).is_none() {
					return Ok(Value::String(text));
				}
				result.result.structured_content.clone().ok_or_else(|| {
					format!("{label} failed: it declares an output schema but returned no structured content")
				})
			}
			Err(error) => {
				let text = error.to_string();
				record(&text, true, false);
				Err(format!("{label} failed: {text}"))
			}
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
		made: Mutex::new(Vec::new()),
		round_snapshot: crate::plugins::payload::round_snapshot(),
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
	let mut made = std::mem::take(&mut *session.made.lock());
	made.sort_by_key(|(number, _)| *number);
	McpToolResult {
		tool_name: call.tool_name.clone(),
		tool_id: call.tool_id.clone(),
		result: if failed {
			CallToolResult::error(content)
		} else {
			CallToolResult::success(content)
		},
		script_calls: made.into_iter().map(|(_, made)| made).collect(),
	}
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "e2e_tests.rs"]
mod e2e_tests;
