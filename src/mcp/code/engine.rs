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

//! Runs one script in an embedded JavaScript engine (QuickJS, compiled into
//! the binary). The engine has no file, network or process access: the only
//! way out is a tool call, which goes through the [`Host`].

use super::catalog::{call_form, Catalog};
use super::check::Checked;
use super::schema;
use parking_lot::Mutex;
use rquickjs::prelude::Async;
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Function};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Memory one script may hold. Tool results are text, so this is far above
/// what a script needs and only stops one that grows without bound.
const MEMORY_LIMIT_BYTES: usize = 256 * 1024 * 1024;

/// How long a script may compute without calling a tool, sleeping or
/// finishing before it is stopped as an endless loop.
const MAX_BUSY: Duration = Duration::from_secs(10);

/// Longest a script may run in total, tool calls and sleeps included. A tool
/// call made directly ends at its server's timeout; a script that loops over
/// calls or sleeps has no such end, and a session with nobody to cancel it
/// would wait on it forever.
const MAX_RUN: Duration = Duration::from_secs(60 * 60);

/// Most text a script may print and return. What comes back goes into the
/// conversation, where far less than this is already too much; the limit
/// only stops a loop that prints without end.
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Name the script is known by in error positions (`script:3:7`).
const SCRIPT_NAME: &str = "script";

/// The functions every script starts with. Tool servers are added after it.
const PRELUDE: &str = r#"
const __show = (value) => {
	if (typeof value === "string") return value;
	if (value === undefined) return "undefined";
	if (value instanceof Error) return __describe(value);
	try {
		const text = JSON.stringify(value);
		return text === undefined ? String(value) : text;
	} catch (_) {
		return String(value);
	}
};
const __describe = (error) => {
	if (!(error instanceof Error)) return "Thrown: " + __show(error);
	// Stack frames name lines of the compiled script; `__lines` maps each
	// back to the line of the script as written.
	const lines = [];
	for (const frame of String(error.stack || "").split("\n")) {
		const found = /\(script:(\d+)/.exec(frame);
		const line = found ? __lines[Number(found[1]) - 1] : 0;
		if (line && !lines.includes(line)) lines.push(line);
	}
	return `${error.name}: ${error.message}` + (lines.length ? `\n  at line ${lines.join(", called from line ")}` : "");
};
globalThis.print = (...values) => __print(values.map(__show).join(" "));
globalThis.console = { log: print, info: print, warn: print, error: print, debug: print };
globalThis.help = (target) =>
	print(typeof target === "string" || target === undefined
		? __help(target === undefined ? "" : target)
		: 'help takes a name: help("server") or help("server.tool")');
globalThis.sleep = (ms) => __sleep(Number(ms) || 0);
globalThis.setTimeout = (callback, ms, ...args) => {
	sleep(ms).then(() => callback(...args));
	return 0;
};
// A last expression that only did things (`await Promise.all(calls.map(print))`)
// leaves undefined, or a list of them: there is no result to show.
const __empty = (value) =>
	value === undefined || value === null || (Array.isArray(value) && value.length > 0 && value.every(__empty));
globalThis.__ok = (value) => __done(false, __empty(value) ? "" : __show(value));
globalThis.__fail = (error) => __done(true, __describe(error));
const __server = (server, tools) =>
	new Proxy({}, {
		get(_, tool) {
			if (typeof tool !== "string" || tool === "then") return undefined;
			if (!tools.includes(tool)) {
				throw new TypeError(`${server}.${tool} is not a tool. Tools of ${server}: ${tools.join(", ")}`);
			}
			return (args) => {
				// Made now, while the calling line of the script is still on the
				// stack: by the time the tool answers, it no longer is.
				const failure = new Error();
				return __call(server, tool, JSON.stringify(args === undefined ? {} : args)).then((text) => {
					const reply = JSON.parse(text);
					if ("err" in reply) {
						failure.message = reply.err;
						throw failure;
					}
					return reply.ok;
				});
			};
		},
	});
"#;

pub type CallFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

/// What a script's tool calls are handed to.
pub trait Host: Send + Sync + 'static {
	/// Run one tool. `Ok` is what the call resolves to (the tool's text, or
	/// its structured result when it declares an output schema), `Err` the
	/// reason it failed; the script sees the second as a thrown error.
	fn call(&self, server: &str, tool: &str, arguments: Value) -> CallFuture;

	/// True once the user cancelled the turn.
	fn cancelled(&self) -> bool;
}

/// What one script produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
	/// Everything the script printed, then its result value.
	pub output: String,
	/// Why the script did not finish, when it did not.
	pub error: Option<String>,
}

#[derive(Default)]
struct State {
	output: Mutex<String>,
	/// Set by the script's own completion: `Ok(result)` or `Err(thrown)`.
	outcome: Mutex<Option<Result<String, String>>>,
	/// When the script last got control back from a tool call or a sleep.
	resumed_at: Mutex<Option<Instant>>,
	stalled: AtomicBool,
	overflowed: AtomicBool,
}

impl State {
	fn resume(&self) {
		*self.resumed_at.lock() = Some(Instant::now());
	}
}

/// The globals of one run: the line map of the compiled script, and one
/// object per server.
fn run_globals(catalog: &Catalog, lines: &[usize]) -> String {
	let mut source = format!("globalThis.__lines = {};\n", json!(lines));
	for server in &catalog.servers {
		source.push_str(&format!(
			"globalThis[{}] = __server({}, {});\n",
			json!(server.ident),
			json!(server.ident),
			json!(server.tool_idents())
		));
	}
	source
}

/// One tool call from the script: check the arguments against the tool's
/// schema, then hand the call to the host.
async fn call_tool(
	catalog: &Catalog,
	host: &dyn Host,
	server: &str,
	tool: &str,
	arguments: &str,
) -> Result<Value, String> {
	let arguments: Value =
		serde_json::from_str(arguments).map_err(|error| format!("arguments are not JSON: {error}"))?;
	let (server, tool) = catalog
		.server(server)
		.and_then(|server| server.tool(tool).map(|tool| (server, tool)))
		.ok_or_else(|| format!("`{server}.{tool}` is not a tool"))?;
	let violations = schema::validate(&tool.schema, &arguments);
	if !violations.is_empty() {
		return Err(format!(
			"{}\n  expected: {}",
			violations.join("; "),
			call_form(server, tool)
		));
	}
	host.call(&server.name, &tool.name, arguments).await
}

fn install(ctx: &Ctx<'_>, state: &Arc<State>, catalog: &Arc<Catalog>, host: &Arc<dyn Host>) -> rquickjs::Result<()> {
	let globals = ctx.globals();

	let print_state = state.clone();
	globals.set(
		"__print",
		Function::new(ctx.clone(), move |text: String| {
			let mut output = print_state.output.lock();
			if output.len() + text.len() > MAX_OUTPUT_BYTES {
				// Dropped, and reported when the run ends: a script that
				// prints in a loop would otherwise grow this without bound.
				print_state.overflowed.store(true, Ordering::Relaxed);
				return;
			}
			output.push_str(&text);
			output.push('\n');
		})?,
	)?;

	let help_catalog = catalog.clone();
	globals.set(
		"__help",
		Function::new(ctx.clone(), move |target: String| help_catalog.help(&target))?,
	)?;

	let done_state = state.clone();
	globals.set(
		"__done",
		Function::new(ctx.clone(), move |failed: bool, text: String| {
			*done_state.outcome.lock() = Some(if failed { Err(text) } else { Ok(text) });
		})?,
	)?;

	let sleep_state = state.clone();
	globals.set(
		"__sleep",
		Function::new(
			ctx.clone(),
			Async(move |milliseconds: f64| {
				let state = sleep_state.clone();
				async move {
					tokio::time::sleep(Duration::from_millis(milliseconds.max(0.0) as u64)).await;
					state.resume();
					Ok::<(), rquickjs::Error>(())
				}
			}),
		)?,
	)?;

	let call_state = state.clone();
	let call_catalog = catalog.clone();
	let call_host = host.clone();
	globals.set(
		"__call",
		Function::new(
			ctx.clone(),
			Async(move |server: String, tool: String, arguments: String| {
				let state = call_state.clone();
				let catalog = call_catalog.clone();
				let host = call_host.clone();
				async move {
					let reply = match call_tool(&catalog, host.as_ref(), &server, &tool, &arguments).await {
						Ok(value) => json!({ "ok": value }),
						Err(text) => json!({ "err": text }),
					};
					state.resume();
					Ok::<String, rquickjs::Error>(reply.to_string())
				}
			}),
		)?,
	)?;
	Ok(())
}

/// Run a script that passed the type check.
pub async fn run(checked: Checked, catalog: Arc<Catalog>, host: Arc<dyn Host>) -> Run {
	let state = Arc::new(State::default());
	let failed = |error: String| Run {
		output: String::new(),
		error: Some(error),
	};

	let runtime = match AsyncRuntime::new() {
		Ok(runtime) => runtime,
		Err(error) => return failed(format!("could not start the script engine: {error}")),
	};
	runtime.set_memory_limit(MEMORY_LIMIT_BYTES).await;
	let watch_state = state.clone();
	let watch_host = host.clone();
	runtime
		.set_interrupt_handler(Some(Box::new(move || {
			if watch_host.cancelled() {
				return true;
			}
			let busy = watch_state
				.resumed_at
				.lock()
				.is_some_and(|resumed_at| resumed_at.elapsed() > MAX_BUSY);
			if busy {
				watch_state.stalled.store(true, Ordering::Relaxed);
			}
			busy
		})))
		.await;
	let context = match AsyncContext::full(&runtime).await {
		Ok(context) => context,
		Err(error) => return failed(format!("could not start the script engine: {error}")),
	};

	let source = checked.javascript;
	let globals = run_globals(&catalog, &checked.lines);
	let start_state = state.clone();
	let started: Result<(), String> = context
		.with(move |ctx| {
			install(&ctx, &start_state, &catalog, &host).map_err(|error| error.to_string())?;
			ctx.eval::<(), _>(PRELUDE)
				.catch(&ctx)
				.map_err(|error| format!("sandbox setup failed: {error}"))?;
			ctx.eval::<(), _>(globals)
				.catch(&ctx)
				.map_err(|error| format!("sandbox setup failed: {error}"))?;
			let mut options = rquickjs::context::EvalOptions::default();
			options.filename = Some(SCRIPT_NAME.to_string());
			start_state.resume();
			ctx.eval_with_options::<(), _>(source, options)
				.catch(&ctx)
				.map_err(|error| error.to_string())
		})
		.await;
	if let Err(error) = started {
		return failed(error);
	}
	// Drives the script's promises and its pending tool calls to the end.
	// Past the limit the engine is dropped, and what is still pending with it.
	let timed_out = tokio::time::timeout(MAX_RUN, runtime.idle()).await.is_err();

	let mut output = std::mem::take(&mut *state.output.lock());
	let outcome = state.outcome.lock().take();
	let error = match outcome {
		_ if timed_out => Some(format!(
			"stopped: the script ran for more than {} minutes",
			MAX_RUN.as_secs() / 60
		)),
		Some(Ok(result)) if output.len() + result.len() > MAX_OUTPUT_BYTES => {
			state.overflowed.store(true, Ordering::Relaxed);
			None
		}
		Some(Ok(result)) => {
			output.push_str(&result);
			None
		}
		Some(Err(thrown)) => Some(thrown),
		None if state.stalled.load(Ordering::Relaxed) => Some(format!(
			"stopped: the script computed for {} seconds without calling a tool or finishing",
			MAX_BUSY.as_secs()
		)),
		None => Some(
			"the script stopped before it finished: it was cancelled, or it awaits a promise that never settles"
				.to_string(),
		),
	};
	let error = match error {
		None if state.overflowed.load(Ordering::Relaxed) => Some(format!(
			"the script printed or returned more than {} KB, and the rest was dropped: filter or summarize inside the script",
			MAX_OUTPUT_BYTES / 1024
		)),
		error => error,
	};
	Run {
		output: output.trim_end().to_string(),
		error,
	}
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
