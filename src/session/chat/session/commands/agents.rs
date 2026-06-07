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

//! `/agents` command — human-facing panel over the tap-run registry.
//!
//! Read-only view of the agents this session offloaded via the `tap` core tool
//! (the same registry in `crate::session::tap_runs`). `tap` is the MCP tool the
//! *model* drives; `/agents` is the *user's* window into what those runs are
//! doing right now.
//!
//! - `/agents`        — list running runs (with live last-action) on top, then
//!   recently finished/failed/cancelled below.
//! - `/agents <id>`   — summary card for one run: role, status, elapsed,
//!   workdir, tokens/cost, last action.
//!
//! Live signal comes from the run's on-disk session file (`<id>.jsonl.zst`),
//! which the agent subprocess appends to as it works (one independent zstd
//! frame per message — see `persistence::append_to_session_file`). We read it
//! tolerantly so a frame being written mid-call never breaks the panel.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::time::SystemTime;

use anyhow::Result;
use serde_json::json;
use zstd::stream::read::Decoder as ZstdDecoder;

use super::{CommandOutput, CommandResult};
use crate::session::tap_runs::{self, TapJobStatus};
use crate::session::{Message, SessionInfo};

/// What we can recover from a run's on-disk session file.
struct AgentSnapshot {
	/// Latest `SUMMARY` snapshot — token/cost/model state (lags slightly for a
	/// live run, fresh enough for a panel).
	info: Option<SessionInfo>,
	/// Last meaningful step the run took (tool call or assistant message).
	last_action: Option<String>,
}

pub fn handle_agents(params: &[&str]) -> Result<CommandResult> {
	// Detail mode: `/agents <id>`
	if let Some(raw) = params.first() {
		let id = raw.trim();
		let info = match tap_runs::find_job(id) {
			Some(i) => i,
			None => {
				return Ok(CommandResult::HandledWithOutput(Box::new(
					CommandOutput::Error {
						error: format!("No agent with id '{id}' in this session."),
						context: None,
					},
				)));
			}
		};
		let snap = read_agent_snapshot(id);
		let detail = json!({
			"id": info.id,
			"role": info.role,
			"status": info.status.as_str(),
			"workdir": info.workdir,
			"elapsed_secs": elapsed_secs(info.started_at),
			"last_action": snap.last_action,
			"model": snap.info.as_ref().map(|i| i.model.clone()),
			"tokens_input": snap.info.as_ref().map(|i| i.input_tokens),
			"tokens_output": snap.info.as_ref().map(|i| i.output_tokens),
			"tokens_cached": snap.info.as_ref().map(|i| i.cache_read_tokens),
			"cost": snap.info.as_ref().map(|i| i.total_cost),
			"tool_calls": snap.info.as_ref().map(|i| i.tool_calls),
		});
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Agents {
				running: vec![],
				finished: vec![],
				detail: Some(detail),
				total: 1,
			},
		)));
	}

	// List mode: running on top (with live last-action), finished below.
	let jobs = tap_runs::list_jobs();
	let total = jobs.len();
	let mut running = Vec::new();
	let mut finished = Vec::new();
	for j in &jobs {
		if j.status == TapJobStatus::Running {
			let snap = read_agent_snapshot(&j.id);
			running.push(json!({
				"id": j.id,
				"role": j.role,
				"status": j.status.as_str(),
				"workdir": j.workdir,
				"elapsed_secs": elapsed_secs(j.started_at),
				"last_action": snap.last_action,
				"model": snap.info.as_ref().map(|i| i.model.clone()),
				"tokens_input": snap.info.as_ref().map(|i| i.input_tokens),
				"tokens_output": snap.info.as_ref().map(|i| i.output_tokens),
				"tokens_cached": snap.info.as_ref().map(|i| i.cache_read_tokens),
				"cost": snap.info.as_ref().map(|i| i.total_cost),
			}));
		} else {
			let snap = read_agent_snapshot(&j.id);
			finished.push(json!({
				"id": j.id,
				"role": j.role,
				"status": j.status.as_str(),
				"workdir": j.workdir,
				"ago_secs": ago_secs(&j.id),
				"model": snap.info.as_ref().map(|i| i.model.clone()),
				"tokens_input": snap.info.as_ref().map(|i| i.input_tokens),
				"tokens_output": snap.info.as_ref().map(|i| i.output_tokens),
				"tokens_cached": snap.info.as_ref().map(|i| i.cache_read_tokens),
				"cost": snap.info.as_ref().map(|i| i.total_cost),
			}));
		}
	}

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Agents {
			running,
			finished,
			detail: None,
			total,
		},
	)))
}

/// Seconds since `started`, saturating at 0.
fn elapsed_secs(started: SystemTime) -> u64 {
	started.elapsed().map(|d| d.as_secs()).unwrap_or(0)
}

/// Seconds since the run's session file was last written (its finish time, near
/// enough). `None` if the file is missing or its mtime is unreadable.
fn ago_secs(id: &str) -> Option<u64> {
	let path = crate::session::logger::get_session_log_path(id).ok()?;
	let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
	modified.elapsed().ok().map(|d| d.as_secs())
}

/// Tolerantly read a run's session file: decode complete frames, stop cleanly at
/// a partial trailing frame the subprocess may be mid-write on. Captures the
/// latest `SUMMARY` (token/cost state) and the last meaningful step.
fn read_agent_snapshot(id: &str) -> AgentSnapshot {
	let mut snap = AgentSnapshot {
		info: None,
		last_action: None,
	};
	let path = match crate::session::logger::get_session_log_path(id) {
		Ok(p) => p,
		Err(_) => return snap,
	};
	let file = match File::open(&path) {
		Ok(f) => f,
		Err(_) => return snap,
	};
	let decoder = match ZstdDecoder::new(file) {
		Ok(d) => d,
		Err(_) => return snap,
	};
	for line in BufReader::new(decoder).lines() {
		// A partial trailing frame surfaces as a read error — stop, keep what
		// we have rather than discarding the whole snapshot.
		let line = match line {
			Ok(l) => l,
			Err(_) => break,
		};
		if line.trim().is_empty() {
			continue;
		}
		let val: serde_json::Value = match serde_json::from_str(&line) {
			Ok(v) => v,
			Err(_) => continue,
		};
		// Marker lines carry a `type`; plain conversation messages don't.
		if let Some(kind) = val.get("type").and_then(|t| t.as_str()) {
			match kind {
				"SUMMARY" => {
					if let Some(info) = val
						.get("session_info")
						.and_then(|i| serde_json::from_value::<SessionInfo>(i.clone()).ok())
					{
						snap.info = Some(info);
					}
				}
				// A restoration point clears prior conversation on reload, so any
				// last-action before it is stale.
				"RESTORATION_POINT" => snap.last_action = None,
				_ => {}
			}
			continue;
		}
		if let Ok(msg) = serde_json::from_value::<Message>(val) {
			if let Some(action) = last_action_from_message(&msg) {
				snap.last_action = Some(action);
			}
		}
	}
	snap
}

/// Describe an assistant step: prefer the first tool call (verb + target),
/// else the assistant's text. Returns `None` for non-assistant / empty turns
/// so earlier actions aren't overwritten.
fn last_action_from_message(msg: &Message) -> Option<String> {
	if msg.role != "assistant" {
		return None;
	}
	if let Some(calls) = &msg.tool_calls {
		if let Some(first) = calls.as_array().and_then(|a| a.first()) {
			let func = first.get("function");
			let name = func
				.and_then(|f| f.get("name"))
				.and_then(|n| n.as_str())
				.unwrap_or("");
			if !name.is_empty() {
				return Some(summarize_tool(name, func.and_then(|f| f.get("arguments"))));
			}
		}
	}
	let text = msg.content.trim();
	if text.is_empty() {
		None
	} else {
		Some(truncate(text, 60))
	}
}

/// `"<tool> <hint>"`, where the hint is the most descriptive scalar arg (path,
/// command, query, …). Arguments may be a JSON string or object.
fn summarize_tool(name: &str, args: Option<&serde_json::Value>) -> String {
	let parsed = match args {
		Some(serde_json::Value::String(s)) => serde_json::from_str::<serde_json::Value>(s).ok(),
		Some(v) => Some(v.clone()),
		None => None,
	};
	let hint = parsed.as_ref().and_then(|v| {
		for key in [
			"file_path",
			"path",
			"command",
			"pattern",
			"query",
			"url",
			"intent",
			"prompt",
			"name",
		] {
			if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
				let s = s.trim();
				if !s.is_empty() {
					return Some(truncate(s, 48));
				}
			}
		}
		None
	});
	match hint {
		Some(h) => format!("{name} {h}"),
		None => name.to_string(),
	}
}

/// Aggregate token/cost stats across all agent runs for this session.
/// Returns `None` when there are no runs. Used by `/info`.
pub fn get_agents_stats() -> Option<serde_json::Value> {
	let jobs = tap_runs::list_jobs();
	if jobs.is_empty() {
		return None;
	}
	let mut total_input: u64 = 0;
	let mut total_output: u64 = 0;
	let mut total_cached: u64 = 0;
	let mut total_cost: f64 = 0.0;
	let mut count_running: usize = 0;
	let mut count_done: usize = 0;
	let mut count_failed: usize = 0;
	for j in &jobs {
		match j.status {
			TapJobStatus::Running => count_running += 1,
			TapJobStatus::Done => count_done += 1,
			TapJobStatus::Failed => count_failed += 1,
			TapJobStatus::Cancelled => {}
		}
		let snap = read_agent_snapshot(&j.id);
		if let Some(info) = snap.info {
			total_input += info.input_tokens;
			total_output += info.output_tokens;
			total_cached += info.cache_read_tokens;
			total_cost += info.total_cost;
		}
	}
	Some(serde_json::json!({
		"total": jobs.len(),
		"running": count_running,
		"done": count_done,
		"failed": count_failed,
		"tokens_input": total_input,
		"tokens_output": total_output,
		"tokens_cached": total_cached,
		"total_cost": total_cost,
	}))
}

/// Single-line, length-capped (ellipsis on overflow).
fn truncate(s: &str, max: usize) -> String {
	let s = s.replace(['\n', '\r'], " ");
	if s.chars().count() <= max {
		s
	} else {
		let head: String = s.chars().take(max.saturating_sub(1)).collect();
		format!("{head}…")
	}
}
