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

use super::super::check;
use super::super::fixtures::catalog;
use super::*;
use std::sync::atomic::AtomicUsize;

/// A host that records every call and answers from the arguments.
#[derive(Default)]
struct Recorder {
	calls: Mutex<Vec<(String, String, Value)>>,
	/// Calls in flight at once, and the most seen.
	running: AtomicUsize,
	peak: AtomicUsize,
	cancelled: AtomicBool,
}

impl Host for Arc<Recorder> {
	fn call(&self, server: &str, tool: &str, arguments: Value) -> CallFuture {
		let recorder = self.clone();
		let (server, tool) = (server.to_string(), tool.to_string());
		Box::pin(async move {
			let running = recorder.running.fetch_add(1, Ordering::SeqCst) + 1;
			recorder.peak.fetch_max(running, Ordering::SeqCst);
			tokio::time::sleep(Duration::from_millis(20)).await;
			recorder.running.fetch_sub(1, Ordering::SeqCst);
			recorder
				.calls
				.lock()
				.push((server.clone(), tool.clone(), arguments.clone()));
			match arguments.get("path").and_then(Value::as_str) {
				Some("missing.txt") => Err("no such file: missing.txt".to_string()),
				// A tool with an output schema resolves to its structured result.
				Some("stats.json") => Ok(json!({"lines": 3, "names": ["a", "b"]})),
				Some(path) => Ok(Value::String(format!("contents of {path}"))),
				None => Ok(Value::String(format!("{server}.{tool} ran"))),
			}
		})
	}

	fn cancelled(&self) -> bool {
		self.cancelled.load(Ordering::SeqCst)
	}
}

async fn run_script(script: &str, input: Value) -> (Run, Arc<Recorder>) {
	let recorder = Arc::new(Recorder::default());
	let run = run_with(script, input, recorder.clone()).await;
	(run, recorder)
}

async fn run_with(script: &str, input: Value, recorder: Arc<Recorder>) -> Run {
	let catalog = catalog();
	let input = (!input.is_null()).then_some(input);
	let checked = check::check(script, &catalog.declarations(input.as_ref()))
		.await
		.expect("test scripts pass the check");
	run(checked, input.as_ref(), Arc::new(catalog), Arc::new(recorder)).await
}

/// A last expression that only did things leaves nothing to show. A list with
/// something in it, or an empty list, is a result.
#[tokio::test]
async fn a_result_that_holds_nothing_is_not_shown() {
	let script = "await Promise.all(['a', 'b'].map(async (word: string) => print(word)))";
	let (run, _) = run_script(script, Value::Null).await;
	assert_eq!(run.error, None);
	assert_eq!(run.output, "a\nb");

	let (run, _) = run_script("[null, 1]", Value::Null).await;
	assert_eq!(run.output, "[null,1]");
	let (run, _) = run_script("const none: string[] = [];\nnone", Value::Null).await;
	assert_eq!(run.output, "[]");
}

#[tokio::test]
async fn printed_text_and_the_result_come_back() {
	let (run, _) = run_script("print('a', 1, {b: 2});\n'done'", Value::Null).await;
	assert_eq!(run.error, None);
	assert_eq!(run.output, "a 1 {\"b\":2}\ndone");
}

#[tokio::test]
async fn a_single_call_is_its_own_result() {
	let (run, recorder) = run_script("files.read({path: 'a.txt'})", Value::Null).await;
	assert_eq!(run.error, None);
	assert_eq!(run.output, "contents of a.txt");
	assert_eq!(
		*recorder.calls.lock(),
		vec![(
			"files".to_string(),
			"read".to_string(),
			serde_json::json!({"path": "a.txt"})
		)]
	);
}

/// A structured result reaches the script as the value it is, not as text.
#[tokio::test]
async fn a_structured_result_arrives_as_a_value() {
	let script = "const stats: any = await files.read({path: 'stats.json'});\nreturn stats.lines + stats.names.length";
	let (run, _) = run_script(script, Value::Null).await;
	assert_eq!(run.error, None);
	assert_eq!(run.output, "5");
}

#[tokio::test]
async fn a_call_reaches_the_server_under_its_real_name() {
	let (run, recorder) = run_script("return await my_search.find_all({query: 'q'})", Value::Null).await;
	assert_eq!(run.error, None);
	assert_eq!(run.output, "my-search.find-all ran");
	assert_eq!(recorder.calls.lock()[0].0, "my-search");
	assert_eq!(recorder.calls.lock()[0].1, "find-all");
}

#[tokio::test]
async fn results_chain_into_later_calls_without_leaving_the_script() {
	let script = r#"
const first = await files.read({ path: "a.txt" });
const second = await files.read({ path: first.split(" ").pop() ?? "" });
print(second.length);
"#;
	let (run, recorder) = run_script(script, Value::Null).await;
	assert_eq!(run.error, None);
	assert_eq!(run.output, "17");
	assert_eq!(recorder.calls.lock().len(), 2);
}

#[tokio::test]
async fn promise_all_runs_calls_at_the_same_time() {
	let script = r#"
const texts = await Promise.all(["a", "b", "c", "d"].map((path) => files.read({ path })));
print(texts.length);
"#;
	let (run, recorder) = run_script(script, Value::Null).await;
	assert_eq!(run.error, None);
	assert_eq!(run.output, "4");
	assert_eq!(recorder.peak.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn a_failed_tool_throws_and_can_be_caught() {
	let script = r#"
try {
	await files.read({ path: "missing.txt" });
} catch (error) {
	print("caught:", error.message);
}
"#;
	let (run, _) = run_script(script, Value::Null).await;
	assert_eq!(run.error, None);
	assert_eq!(run.output, "caught: no such file: missing.txt");
}

#[tokio::test]
async fn an_uncaught_failure_ends_the_script_and_keeps_what_was_printed() {
	let script = "print('before');\nawait files.read({ path: 'missing.txt' });\nprint('after');";
	let (run, _) = run_script(script, Value::Null).await;
	assert_eq!(run.output, "before");
	let error = run.error.expect("the script failed");
	assert_eq!(error, "Error: no such file: missing.txt\n  at line 2");
}

#[tokio::test]
async fn the_failing_line_is_the_one_in_the_script_as_written() {
	let script = "interface Unused {\n\tn: number;\n}\nprint('before');\nawait files.read({ path: 'missing.txt' });";
	let (run, _) = run_script(script, Value::Null).await;
	assert_eq!(
		run.error.as_deref(),
		Some("Error: no such file: missing.txt\n  at line 5")
	);
}

#[tokio::test]
async fn values_the_types_cannot_see_are_checked_when_the_call_is_made() {
	// `JSON.parse` gives `any`, so the type check lets this through.
	let script = "const args = JSON.parse('{\"file\": \"a\"}');\nawait files.read(args);";
	let (run, recorder) = run_script(script, Value::Null).await;
	let error = run.error.expect("the call is refused");
	assert!(error.contains("unknown argument `file`"), "{error}");
	assert!(
		error.contains("expected: files.read({path: string, limit?: number})"),
		"{error}"
	);
	assert!(
		recorder.calls.lock().is_empty(),
		"a refused call never reaches the server"
	);
}

#[tokio::test]
async fn input_passes_text_without_javascript_quoting() {
	let text = "line1\nline2 `tick` ${x} \\n \"quoted\"";
	let input = serde_json::json!({"path": "a.rs", "content": text});
	let (run, recorder) = run_script("files.write(input)", input.clone()).await;
	assert_eq!(run.error, None);
	assert_eq!(recorder.calls.lock()[0].2, input);
}

#[tokio::test]
async fn omitted_input_is_undefined() {
	let (run, _) = run_script("return input === undefined ? 'undefined' : 'defined'", Value::Null).await;
	assert_eq!(run.error, None);
	assert_eq!(run.output, "undefined");
}

#[tokio::test]
async fn help_prints_signatures() {
	let (run, _) = run_script("help('files')", Value::Null).await;
	assert_eq!(run.error, None);
	assert!(
		run.output.contains("files.read({path: string, limit?: number})"),
		"{}",
		run.output
	);
}

#[tokio::test]
async fn a_tool_name_the_types_cannot_see_is_still_refused_when_unknown() {
	let (run, _) = run_script("const server: any = files;\nawait server.nope({});", Value::Null).await;
	let error = run.error.expect("unknown tool");
	assert!(
		error.contains("files.nope is not a tool. Tools of files: read, write"),
		"{error}"
	);
}

#[tokio::test]
async fn sleep_and_set_timeout_work() {
	let script = "await sleep(5);\nawait new Promise((resolve) => setTimeout(resolve, 5));\n'woke'";
	let (run, _) = run_script(script, Value::Null).await;
	assert_eq!(run.error, None);
	assert_eq!(run.output, "woke");
}

#[tokio::test]
async fn the_sandbox_has_no_way_out_but_tools() {
	let script = "const globals: any = globalThis;\n\
		['require', 'process', 'fetch', 'Deno', 'os', 'std', 'XMLHttpRequest'].map((name) => typeof globals[name]).join(' ')";
	let (run, _) = run_script(script, Value::Null).await;
	assert_eq!(
		run.output,
		"undefined undefined undefined undefined undefined undefined undefined"
	);
}

#[tokio::test]
async fn a_promise_that_never_settles_does_not_hang() {
	let (run, _) = run_script("await new Promise(() => {});", Value::Null).await;
	let error = run.error.expect("the script cannot finish");
	assert!(error.contains("never settles"), "{error}");
}

#[tokio::test]
async fn output_past_the_limit_is_dropped_and_reported() {
	let script = "for (let i = 0; i < 2000; i++) print('x'.repeat(1000));\n'done'";
	let (run, _) = run_script(script, Value::Null).await;
	let error = run.error.expect("more output than a script may produce");
	assert!(error.contains("more than 1024 KB"), "{error}");
	assert!(run.output.len() <= MAX_OUTPUT_BYTES);
}

#[tokio::test]
async fn a_helper_that_fails_names_both_lines() {
	let script = "async function load(path: string) {\n\treturn await files.read({ path });\n}\nprint('x');\nawait load('missing.txt');";
	let (run, _) = run_script(script, Value::Null).await;
	assert_eq!(
		run.error.as_deref(),
		Some("Error: no such file: missing.txt\n  at line 2, called from line 5")
	);
}

/// A tool call is run by a spawned task, so a run must be able to move
/// between threads.
#[test]
fn a_run_can_move_between_threads() {
	fn assert_send<T: Send>(_: T) {}
	let checked = check::Checked {
		javascript: String::new(),
		lines: Vec::new(),
	};
	assert_send(run(
		checked,
		None,
		Arc::new(catalog()),
		Arc::new(Arc::new(Recorder::default())),
	));
}

#[tokio::test]
async fn a_cancelled_turn_stops_a_running_script() {
	let recorder = Arc::new(Recorder::default());
	recorder.cancelled.store(true, Ordering::SeqCst);
	let run = run_with("let n = 0;\nwhile (true) { n += 1; }", Value::Null, recorder).await;
	assert!(run.error.is_some());
}
