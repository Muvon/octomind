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

//! /workflow command — list tap workflows or run one from inside a session.
//!
//! `/workflow`                  → list public tap workflows
//! `/workflow <name>`           → show the definition of tap workflow `<name>` (nothing runs)
//! `/workflow <name> <input…>`  → run tap workflow `<name>` with `<input…>` as its stdin
//!
//! Execution lives in `crate::workflow::spawn`, shared with the `tap` tool's
//! `workflow` action. This command blocks the slash-command path for the
//! workflow's duration; the tool action runs it as a background tap-run.

use super::{CommandOutput, CommandResult};
use anyhow::Result;
use serde_json::{json, Value};

pub async fn handle_workflow(input: &str, params: &[&str]) -> Result<CommandResult> {
	let Some(name) = params.first() else {
		let workflows = crate::workflow::spawn::list_tap_workflows()?;
		return Ok(output(
			json!({ "subcommand": "list", "workflows": workflows }),
		));
	};
	let Some(wf_input) = split_input(input) else {
		return Ok(match crate::agent::taps::fetch_workflow(name) {
			Ok((definition, source_tap)) => output(json!({
				"subcommand": "show",
				"name": name,
				"source_tap": source_tap,
				"definition": definition,
			})),
			Err(e) => error(format!("{e:#}")),
		});
	};
	match crate::workflow::spawn::run_tap_workflow(name, wf_input).await {
		Ok(run) => Ok(output(json!({
			"subcommand": "run",
			"name": name,
			"output": run.output,
			"cost": run.cost,
		}))),
		Err(e) => Ok(error(format!("{e:#}"))),
	}
}

/// Everything after `/workflow <name>`, verbatim — whitespace inside the input
/// is meaningful (a URL list, a question), so it must not be re-tokenised.
fn split_input(input: &str) -> Option<&str> {
	let rest = input
		.trim_start()
		.strip_prefix(crate::session::chat::commands::WORKFLOW_COMMAND)?
		.trim_start();
	let (_name, wf_input) = rest.split_once(char::is_whitespace)?;
	let wf_input = wf_input.trim();
	(!wf_input.is_empty()).then_some(wf_input)
}

fn output(data: Value) -> CommandResult {
	CommandResult::HandledWithOutput(Box::new(CommandOutput::Workflow { data }))
}

fn error(message: String) -> CommandResult {
	output(json!({ "subcommand": "error", "message": message }))
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
