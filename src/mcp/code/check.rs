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

//! The check a script passes before any of it runs: the TypeScript compiler
//! type-checks it, in strict mode, against declarations of every tool it may
//! call.
//!
//! A script that calls three tools and gets the third one wrong would
//! otherwise run the first two and then fail, leaving the model to work out
//! what already happened. A wrong tool name, a wrong argument, or a result
//! used as something it is not (a text result treated as a list) rejects the
//! whole script, so a wrong script has no side effects.
//!
//! The compiler is TypeScript's own, embedded in the binary and run by the
//! same JavaScript engine that runs the scripts: no Node.js on the machine.
//! It lives on one thread for the life of the process, because loading it and
//! parsing the standard library typings is the expensive part (about a second
//! in total); a check after that takes tens of milliseconds.

use parking_lot::Mutex;
use rquickjs::{CatchResultExt, Context, Function, Runtime};
use serde::Deserialize;
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// The TypeScript compiler, zstd-compressed. See `assets/typescript/vendor.sh`.
const COMPILER: &[u8] = include_bytes!("../../../assets/typescript/typescript.js.zst");

/// TypeScript's standard library typings as JSON (file name -> text), zstd-compressed.
const LIBRARIES: &[u8] = include_bytes!("../../../assets/typescript/lib.json.zst");

const CHECKER: &str = include_str!("checker.js");

/// Stack of the checker thread. The compiler recurses deeply over types; the
/// engine's own limit sits below it so a runaway recursion is a JavaScript
/// error, not a crashed thread.
const THREAD_STACK_BYTES: usize = 64 * 1024 * 1024;
const ENGINE_STACK_BYTES: usize = 48 * 1024 * 1024;

/// Memory the compiler may hold. It needs about 45 MB; the limit only stops
/// a check that grows without bound.
const MEMORY_LIMIT_BYTES: usize = 1024 * 1024 * 1024;

/// Longest one check may take. A check takes tens of milliseconds (about a
/// second for the first); every session waits on the same checker, so one
/// that never ends would stop them all.
const MAX_CHECK: Duration = Duration::from_secs(60);

/// When the check in progress must be over; `None` between checks.
type Deadline = Arc<Mutex<Option<Instant>>>;

/// One reason a script was rejected.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Problem {
	/// 1-based place in the script; 0 when the problem has none.
	#[serde(default)]
	pub line: usize,
	#[serde(default)]
	pub column: usize,
	pub message: String,
}

/// A script that passed the check, ready to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
	/// The script as plain JavaScript, wrapped so its result is reported.
	pub javascript: String,
	/// `lines[n - 1]` is the script line that line `n` of `javascript` came
	/// from, 0 when it came from none.
	pub lines: Vec<usize>,
}

#[derive(Deserialize)]
struct Verdict {
	#[serde(default)]
	problems: Vec<Problem>,
	#[serde(default)]
	javascript: String,
	#[serde(default)]
	lines: Vec<usize>,
}

struct Request {
	declarations: String,
	script: String,
	/// Absent for the warm-up request, whose answer nobody reads.
	reply: Option<oneshot::Sender<Result<String, String>>>,
}

fn checker() -> &'static mpsc::Sender<Request> {
	static CHECKER_THREAD: OnceLock<mpsc::Sender<Request>> = OnceLock::new();
	CHECKER_THREAD.get_or_init(|| {
		let (requests, inbox) = mpsc::channel();
		std::thread::Builder::new()
			.name("code-typecheck".to_string())
			.stack_size(THREAD_STACK_BYTES)
			.spawn(move || serve(inbox))
			.expect("the operating system refused to start the type-check thread");
		requests
	})
}

fn load(deadline: &Deadline) -> Result<(Runtime, Context), String> {
	let compiler = zstd::decode_all(COMPILER).map_err(|error| format!("embedded compiler is unreadable: {error}"))?;
	let libraries =
		zstd::decode_all(LIBRARIES).map_err(|error| format!("embedded library typings are unreadable: {error}"))?;
	let runtime = Runtime::new().map_err(|error| error.to_string())?;
	runtime.set_max_stack_size(ENGINE_STACK_BYTES);
	runtime.set_memory_limit(MEMORY_LIMIT_BYTES);
	let deadline = deadline.clone();
	runtime.set_interrupt_handler(Some(Box::new(move || {
		deadline.lock().is_some_and(|deadline| Instant::now() > deadline)
	})));
	let context = Context::full(&runtime).map_err(|error| error.to_string())?;
	context.with(|ctx| {
		ctx.eval::<(), _>(compiler)
			.catch(&ctx)
			.map_err(|error| error.to_string())?;
		// The typings file is JSON, and JSON is a JavaScript expression.
		let mut libraries_source = b"globalThis.__libs = ".to_vec();
		libraries_source.extend_from_slice(&libraries);
		ctx.eval::<(), _>(libraries_source)
			.catch(&ctx)
			.map_err(|error| error.to_string())?;
		ctx.eval::<(), _>(CHECKER)
			.catch(&ctx)
			.map_err(|error| error.to_string())
	})?;
	Ok((runtime, context))
}

fn serve(inbox: mpsc::Receiver<Request>) {
	let deadline = Deadline::default();
	let mut loaded = load(&deadline);
	for request in inbox {
		let answer = match &loaded {
			Ok((_, context)) => {
				*deadline.lock() = Some(Instant::now() + MAX_CHECK);
				let answer = context.with(|ctx| {
					let check: Function = ctx.globals().get("__check").map_err(|error| error.to_string())?;
					check
						.call::<_, String>((request.declarations, request.script))
						.catch(&ctx)
						.map_err(|error| error.to_string())
				});
				let overran = deadline.lock().take().is_some_and(|deadline| Instant::now() > deadline);
				match answer {
					Err(_) if overran => Err(format!(
						"the type check was stopped after {} seconds",
						MAX_CHECK.as_secs()
					)),
					answer => answer,
				}
			}
			Err(error) => Err(error.clone()),
		};
		// Problems in a script come back as an answer. A check that threw was
		// cut short (time, memory, stack) and may have left the compiler's
		// caches half written, so the next check gets a fresh compiler.
		if answer.is_err() && loaded.is_ok() {
			loaded = load(&deadline);
		}
		if let Some(reply) = request.reply {
			// The asker may have been cancelled; its answer is then unwanted.
			let _ = reply.send(answer);
		}
	}
}

/// Start loading the compiler now, so the first script does not wait for it.
/// Only the first call does anything.
pub fn warm() {
	static WARMED: std::sync::Once = std::sync::Once::new();
	WARMED.call_once(|| {
		// The thread outlives every sender, so a send cannot fail.
		let _ = checker().send(Request {
			declarations: String::new(),
			script: String::new(),
			reply: None,
		});
	});
}

fn failure(message: String) -> Vec<Problem> {
	vec![Problem {
		line: 0,
		column: 0,
		message,
	}]
}

/// Type-check `script` against `declarations` (TypeScript `declare` text for
/// everything a script may use).
pub async fn check(script: &str, declarations: &str) -> Result<Checked, Vec<Problem>> {
	let (reply, answer) = oneshot::channel();
	checker()
		.send(Request {
			declarations: declarations.to_string(),
			script: script.to_string(),
			reply: Some(reply),
		})
		.map_err(|_| failure("the type checker stopped".to_string()))?;
	let answer = answer
		.await
		.map_err(|_| failure("the type checker stopped".to_string()))?
		.map_err(|error| failure(format!("the type checker failed: {error}")))?;
	let verdict: Verdict = serde_json::from_str(&answer)
		.map_err(|error| failure(format!("the type checker answered nonsense: {error}")))?;
	if !verdict.problems.is_empty() {
		return Err(verdict.problems);
	}
	Ok(Checked {
		javascript: verdict.javascript,
		lines: verdict.lines,
	})
}

/// The rejection the model reads: every problem with its place in the script.
pub fn report(problems: &[Problem]) -> String {
	let mut text = String::from("Script rejected by the type check before it ran; no tool was called.");
	for problem in problems {
		text.push('\n');
		if problem.line > 0 {
			text.push_str(&format!("line {}:{} ", problem.line, problem.column));
		}
		text.push_str(&problem.message);
	}
	text
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
