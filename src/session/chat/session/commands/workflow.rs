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
//! `/workflow <name> <input…>`  → run tap workflow `<name>` with `<input…>` as its stdin
//!
//! The run shells out to `octomind workflow <name> --format jsonl` — the exact
//! CLI path — so the session runtime never hosts the workflow's subprocess
//! tree, and the JSONL result stream stays the single contract. The child
//! inherits the environment, so capability tokens (e.g. the octoweb workspace
//! header) reach every step.

use super::{CommandOutput, CommandResult};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

pub async fn handle_workflow(input: &str, params: &[&str]) -> Result<CommandResult> {
	let Some(name) = params.first() else {
		return list();
	};
	let Some(wf_input) = split_input(input) else {
		return Ok(error(format!("usage: /workflow {name} <input>")));
	};
	run(name, wf_input).await
}

fn list() -> Result<CommandResult> {
	let workflows = crate::agent::registry::list_all_tap_workflows()
		.context("failed to enumerate tap workflows")?;
	let items: Vec<Value> = workflows
		.iter()
		.map(|w| {
			json!({
				"name": w.name,
				"description": w.description,
				"source_tap": w.source_tap,
			})
		})
		.collect();
	Ok(output(json!({ "subcommand": "list", "workflows": items })))
}

async fn run(name: &str, input: &str) -> Result<CommandResult> {
	let exe = std::env::current_exe().context("cannot locate the octomind binary")?;
	// ponytail: blocks the command path for the workflow's duration; stream
	// per-step events as session notifications if long workflows hurt.
	let mut child = tokio::process::Command::new(exe)
		.args(["workflow", name, "--format", "jsonl"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.context("failed to spawn `octomind workflow`")?;
	if let Some(mut stdin) = child.stdin.take() {
		stdin
			.write_all(input.as_bytes())
			.await
			.context("failed to feed workflow input")?;
		// Dropping stdin closes it — the workflow reads stdin to EOF.
	}
	let out = child
		.wait_with_output()
		.await
		.context("workflow process failed")?;

	let mut last_text: Option<String> = None;
	let mut cost: Option<Value> = None;
	for line in String::from_utf8_lossy(&out.stdout).lines() {
		let Ok(ev) = serde_json::from_str::<Value>(line) else {
			continue;
		};
		match ev.get("type").and_then(Value::as_str) {
			Some("assistant") => {
				last_text = ev
					.get("content")
					.and_then(Value::as_str)
					.map(str::to_string);
			}
			Some("cost") => cost = ev.get("session_cost").cloned(),
			_ => {}
		}
	}

	match last_text {
		Some(text) if out.status.success() => Ok(output(json!({
			"subcommand": "run",
			"name": name,
			"output": text,
			"cost": cost,
		}))),
		_ => {
			let stderr = String::from_utf8_lossy(&out.stderr);
			let tail = stderr.trim().lines().last().unwrap_or("").to_string();
			Ok(error(format!(
				"workflow '{name}' failed ({}). {tail}\nValidate with `octomind workflow {name} --dry-run`.",
				out.status
			)))
		}
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
