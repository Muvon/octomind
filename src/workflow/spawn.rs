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

//! Run a tap workflow by name from inside a running session. Shared by the
//! `/workflow` session command and the `tap(action="workflow")` tool.
//!
//! The run shells out to `octomind workflow <name> --format jsonl` — the exact
//! CLI path — so the session runtime never hosts the workflow's subprocess
//! tree, and the JSONL result stream stays the single contract. The child
//! inherits the environment, so capability tokens (e.g. the octoweb workspace
//! header) reach every step. Dropping the returned future kills the child.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

pub struct RunOutcome {
	/// The final step's assistant text.
	pub output: String,
	/// Aggregated `session_cost` from the trailing `cost` event, when emitted.
	pub cost: Option<f64>,
}

/// Every public workflow installed across taps, as `{name, description, source_tap}`.
pub fn list_tap_workflows() -> Result<Vec<Value>> {
	let workflows = crate::agent::registry::list_all_tap_workflows()
		.context("failed to enumerate tap workflows")?;
	Ok(workflows
		.iter()
		.map(|w| {
			json!({
				"name": w.name,
				"description": w.description,
				"source_tap": w.source_tap,
			})
		})
		.collect())
}

pub async fn run_tap_workflow(name: &str, input: &str) -> Result<RunOutcome> {
	let exe = std::env::current_exe().context("cannot locate the octomind binary")?;
	let mut child = tokio::process::Command::new(exe)
		.args(["workflow", name, "--format", "jsonl"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.kill_on_drop(true)
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
	let mut cost: Option<f64> = None;
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
			Some("cost") => cost = ev.get("session_cost").and_then(Value::as_f64),
			_ => {}
		}
	}

	match last_text {
		Some(output) if out.status.success() => Ok(RunOutcome { output, cost }),
		_ => {
			let stderr = String::from_utf8_lossy(&out.stderr);
			let tail = stderr.trim().lines().last().unwrap_or("").to_string();
			bail!(
				"workflow '{name}' failed ({}). {tail}\nValidate with `octomind workflow {name} --dry-run`.",
				out.status
			)
		}
	}
}
