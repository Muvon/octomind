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

use super::super::fixtures::catalog;
use super::*;

async fn problems(script: &str) -> Vec<Problem> {
	match check(script, &catalog().declarations()).await {
		Ok(_) => Vec::new(),
		Err(problems) => problems,
	}
}

/// The one problem a script has, as `line:column message`.
async fn problem(script: &str) -> String {
	let found = problems(script).await;
	assert_eq!(found.len(), 1, "expected one problem, got {found:?}");
	format!("{}:{} {}", found[0].line, found[0].column, found[0].message)
}

#[tokio::test]
async fn a_correct_script_passes() {
	let script = r#"
const text = await files.read({ path: "a.txt", limit: 10 });
const hits = await my_search.find_all({ query: text, mode: "code", page: { size: 5 } });
print(hits.length);
"#;
	assert_eq!(problems(script).await, Vec::new());
}

#[tokio::test]
async fn typescript_syntax_is_accepted_and_compiled_away() {
	let script = "interface Row { n: number }\nconst rows: Row[] = JSON.parse(await files.read({ path: 'a' }));\nrows.filter((row) => row.n > 1).length";
	let checked = check(script, &catalog().declarations()).await.unwrap();
	assert!(!checked.javascript.contains("interface"), "{}", checked.javascript);
	assert!(!checked.javascript.contains(": Row"), "{}", checked.javascript);
	assert!(
		checked.javascript.contains("return rows.filter"),
		"{}",
		checked.javascript
	);
}

#[tokio::test]
async fn top_level_return_and_await_are_allowed() {
	assert_eq!(
		problems("if (1 > 2) return 'none';\nreturn await files.read({ path: 'a' });").await,
		Vec::new()
	);
}

#[tokio::test]
async fn a_syntax_error_is_placed() {
	let found = problems("const a = 1;\nconst b = ;").await;
	assert_eq!(found[0].line, 2, "{found:?}");
	assert!(found[0].message.starts_with("TS1109"), "{found:?}");
}

#[tokio::test]
async fn an_unknown_tool_is_caught() {
	let text = problem("await files.raed({ path: 'a' })").await;
	assert!(text.starts_with("1:13 TS"), "{text}");
	assert!(
		text.contains("Property 'raed' does not exist on type 'filesTools'"),
		"{text}"
	);
}

#[tokio::test]
async fn a_wrong_argument_name_is_caught_and_the_first_call_never_runs() {
	let text = problem("await files.write({ content: 'x', path: 'a' });\nawait files.read({ file: 'a' });").await;
	assert!(text.starts_with("2:20 TS2353"), "{text}");
	assert!(text.contains("'file' does not exist in type"), "{text}");
}

#[tokio::test]
async fn a_wrong_argument_type_is_caught() {
	let text = problem("await files.read({ path: 'a', limit: 'ten' })").await;
	assert!(
		text.contains("TS2322: Type 'string' is not assignable to type 'number'"),
		"{text}"
	);
	let text = problem("await my_search.find_all({ query: 'q', mode: 'web' })").await;
	assert!(text.contains("TS2322: Type '\"web\"' is not assignable"), "{text}");
}

#[tokio::test]
async fn a_missing_required_argument_is_caught() {
	let text = problem("await files.read({})").await;
	assert!(text.contains("TS2345"), "{text}");
	assert!(text.contains("Property 'path' is missing"), "{text}");
	let text = problem("await files.read()").await;
	assert!(text.contains("TS2554: Expected 1 arguments, but got 0"), "{text}");
}

#[tokio::test]
async fn a_text_result_used_as_a_list_is_caught() {
	let text = problem("const hits = await my_search.find_all({ query: 'q' });\nprint(hits.map(String));").await;
	assert!(
		text.starts_with("2:12 TS2339: Property 'map' does not exist on type 'string'"),
		"{text}"
	);
}

#[tokio::test]
async fn a_forgotten_await_is_caught() {
	let text = problem("const text = files.read({ path: 'a' });\nprint(text.length);").await;
	assert!(
		text.contains("TS2339: Property 'length' does not exist on type 'Promise<string>'"),
		"{text}"
	);
}

#[tokio::test]
async fn values_that_flow_through_variables_are_checked() {
	let text = problem("const args = { path: 1 };\nawait files.read(args);").await;
	assert!(text.starts_with("2:18 TS2345"), "{text}");
}

#[tokio::test]
async fn something_the_sandbox_does_not_have_is_caught() {
	let text = problem("const page = await fetch('https://example.com');").await;
	assert!(text.contains("TS2304: Cannot find name 'fetch'"), "{text}");
	assert_eq!(
		problem("import fs from 'fs';").await,
		"1:1 import and export are not available: a script can only call the listed tools"
	);
}

/// `input` was a global of the sandbox while the tool had a second
/// parameter; a script that still reads it names what does not exist.
#[tokio::test]
async fn a_script_that_reads_input_is_rejected() {
	let found = problems("await files.write(input)").await;
	assert_eq!(found.len(), 1, "{found:?}");
	assert!(found[0].message.contains("Cannot find name 'input'"), "{found:?}");
}

#[tokio::test]
async fn reading_a_caught_error_is_allowed() {
	assert_eq!(
		problems("try { await files.read({ path: 'a' }); } catch (error) { print(error.message); }").await,
		Vec::new()
	);
}

#[tokio::test]
async fn a_trailing_expression_becomes_the_result() {
	let checked = check("const a = 1;\na + 1", &catalog().declarations()).await.unwrap();
	assert!(checked.javascript.contains("return a + 1"), "{}", checked.javascript);
	let checked = check("const a = 1;", &catalog().declarations()).await.unwrap();
	assert!(!checked.javascript.contains("return"), "{}", checked.javascript);
}

#[tokio::test]
async fn compiled_lines_map_back_to_the_script() {
	let script = "interface Row {\n\tn: number;\n}\nconst rows: Row[] = [];\nprint(rows.length);";
	let checked = check(script, &catalog().declarations()).await.unwrap();
	let compiled: Vec<&str> = checked.javascript.lines().collect();
	let print_at = compiled
		.iter()
		.position(|line| line.contains("print(rows.length)"))
		.unwrap();
	assert_eq!(
		checked.lines[print_at], 5,
		"{:?}\n{}",
		checked.lines, checked.javascript
	);
}

#[tokio::test]
async fn the_report_places_each_problem() {
	let found = problems("const a = 1;\n  await files.raed({});").await;
	let text = report(&found);
	assert!(
		text.starts_with("Script rejected by the type check before it ran; no tool was called.\nline 2:15 TS"),
		"{text}"
	);
}

#[tokio::test]
async fn warming_up_is_harmless() {
	warm();
	assert_eq!(problems("print(1)").await, Vec::new());
}
