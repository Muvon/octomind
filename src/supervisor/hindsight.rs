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

//! hindsight seam — observe-only. When the agent hands a turn back, the
//! session's messages are rendered as the canonical event trace and scored by
//! `octolib::hindsight`: the calibrated probability that the user's next turn
//! is a correction, plus one probability per correction class. Nothing acts
//! on the numbers yet: they go to the debug log, a supervisor notice, the
//! `/info` tally and `hindsight.jsonl` in the data directory, where the
//! verify-gate verdict of the same turn is appended so the two can be
//! compared offline.
//!
//! The model loads once per process (a second or two for the graphs) and the
//! trace is encoded incrementally: events already encoded for this session are
//! reused as long as the message prefix they came from is unchanged, so a turn
//! end costs the new events only (~30 ms each on CPU).

use crate::session::Message;
use octolib::hindsight::trace::{op_for, Event, Op, Status};
use octolib::hindsight::{Hindsight, Scores, Session};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::sync::{Arc, Mutex};

/// `[supervisor.hindsight]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HindsightConfig {
	pub enabled: bool,
	/// Hugging Face repo (files under `onnx/`) or a local export directory.
	pub model: String,
}

/// Records of every scored boundary and every gate verdict, one JSON object
/// per line, in the data directory.
pub const RECORD_FILE: &str = "hindsight.jsonl";

static MODEL: tokio::sync::OnceCell<Arc<Hindsight>> = tokio::sync::OnceCell::const_new();
/// The "unavailable" notice is shown once per process; a failed load (a
/// download that broke, a lock held by a parallel session) is retried at the
/// next turn end without repeating it.
static NOTIFIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The encoded trace of each session seen by this process, keyed by session
/// name, with the message prefix it was built from.
struct Cached {
	session: Session,
	consumed: usize,
	fingerprint: u64,
}

static TRACES: Mutex<Option<HashMap<String, Cached>>> = Mutex::new(None);
/// `(session, events)` of the last scored boundary, so the gate verdict that
/// follows can be recorded against it.
static LAST: Mutex<Option<(String, usize)>> = Mutex::new(None);

pub fn enabled(config: &crate::config::Config) -> bool {
	config.supervisor.enabled && config.supervisor.hindsight.enabled
}

async fn model(spec: &str) -> Option<Arc<Hindsight>> {
	let started = std::time::Instant::now();
	let loaded = MODEL
		.get_or_try_init(|| async {
			let model = Hindsight::load(spec).await?;
			crate::log_debug!(
				"hindsight: loaded {} in {} ms",
				model.config().model,
				started.elapsed().as_millis()
			);
			Ok::<_, anyhow::Error>(Arc::new(model))
		})
		.await;
	match loaded {
		Ok(model) => Some(model.clone()),
		Err(error) => {
			crate::log_debug!("hindsight unavailable: {:#}", error);
			if !NOTIFIED.swap(true, std::sync::atomic::Ordering::Relaxed) {
				super::notify(&format!("hindsight unavailable: {error}"));
			}
			None
		}
	}
}

/// Score the boundary at the end of `messages`. `None` when the seam is off,
/// the model failed to load, or the trace has no user turn yet.
pub async fn observe(
	config: &crate::config::Config,
	session_name: &str,
	messages: &[Message],
) -> Option<Scores> {
	if !enabled(config) {
		return None;
	}
	let model = model(&config.supervisor.hindsight.model).await?;
	let events = trace(messages);
	if events.is_empty() {
		return None;
	}
	let name = session_name.to_string();
	let started = std::time::Instant::now();
	let scored = tokio::task::spawn_blocking(move || score_cached(model, &name, events))
		.await
		.unwrap_or_else(|join| Err(anyhow::anyhow!("hindsight task: {join}")));
	let (scores, events) = match scored {
		Ok(scored) => scored,
		Err(error) => {
			crate::log_debug!("hindsight: {:#}", error);
			return None;
		}
	};
	let ms = started.elapsed().as_millis();
	crate::log_debug!(
		"hindsight: {} ({} events, {} ms)",
		scores.summary(),
		events,
		ms
	);
	super::notify(&format!("hindsight {}", scores.summary()));
	super::stats::hindsight(scores.p_correction);
	*LAST.lock().unwrap_or_else(|p| p.into_inner()) = Some((session_name.to_string(), events));
	let classes: serde_json::Map<String, serde_json::Value> = scores
		.classes
		.iter()
		.map(|(name, p)| (name.clone(), serde_json::json!(p)))
		.collect();
	record(serde_json::json!({
		"ts": crate::utils::time::now_secs(),
		"session": session_name,
		"events": events,
		"model": config.supervisor.hindsight.model,
		"p_correction": scores.p_correction,
		"classes": classes,
		"ms": ms,
	}));
	Some(scores)
}

/// Append the verify-gate verdict (`pass`, `gaps`, `stall`, `indeterminate`)
/// of the boundary scored last.
pub fn record_gate(config: &crate::config::Config, verdict: &str) {
	if !enabled(config) {
		return;
	}
	let Some((session, events)) = LAST.lock().unwrap_or_else(|p| p.into_inner()).clone() else {
		return;
	};
	record(serde_json::json!({
		"ts": crate::utils::time::now_secs(),
		"session": session,
		"events": events,
		"gate": verdict,
	}));
}

fn record(line: serde_json::Value) {
	let path = match crate::directories::get_octomind_data_dir() {
		Ok(dir) => dir.join(RECORD_FILE),
		Err(error) => {
			crate::log_debug!("hindsight record: {}", error);
			return;
		}
	};
	let written = std::fs::OpenOptions::new()
		.create(true)
		.append(true)
		.open(&path)
		.and_then(|mut file| writeln!(file, "{line}"));
	if let Err(error) = written {
		crate::log_debug!("hindsight record {}: {}", path.display(), error);
	}
}

fn score_cached(
	model: Arc<Hindsight>,
	name: &str,
	events: Vec<Event>,
) -> anyhow::Result<(Scores, usize)> {
	let mut traces = TRACES.lock().unwrap_or_else(|p| p.into_inner());
	let traces = traces.get_or_insert_with(HashMap::new);
	let cached = traces.entry(name.to_string()).or_insert_with(|| Cached {
		session: Session::new(model.clone()),
		consumed: 0,
		fingerprint: 0,
	});
	// Compression rewrites the message list; a changed prefix means the cached
	// vectors no longer describe this trace.
	if cached.consumed > events.len()
		|| fingerprint(&events[..cached.consumed]) != cached.fingerprint
	{
		cached.session = Session::new(model);
		cached.consumed = 0;
	}
	for event in &events[cached.consumed..] {
		cached.session.push(event)?;
	}
	cached.consumed = events.len();
	cached.fingerprint = fingerprint(&events);
	Ok((cached.session.score()?, events.len()))
}

fn fingerprint(events: &[Event]) -> u64 {
	let mut hasher = std::hash::DefaultHasher::new();
	for event in events {
		serde_json::to_string(event)
			.unwrap_or_default()
			.hash(&mut hasher);
	}
	hasher.finish()
}

/// The canonical trace of a message list: genuine user turns, assistant
/// thinking and text, tool calls and results, supervisor injections as harness
/// steers, a continuation wrapper as the task plus a compaction mark. Leading
/// system-managed messages before the first user turn are dropped — a trace
/// opens with the task.
pub fn trace(messages: &[Message]) -> Vec<Event> {
	let mut events = Vec::new();
	for message in messages {
		match message.role.as_str() {
			"user" => {
				if crate::session::is_real_user_task_message(message) {
					events.push(Event::user_turn(&message.content));
				} else if let Some(task) = crate::session::continuation_task(&message.content) {
					if events.is_empty() {
						events.push(Event::user_turn(task));
					}
					events.push(Event::harness("compaction", "continuation", task));
				} else if !message.content.trim().is_empty() {
					let kind = if super::gate::is_supervisor_injection(&message.content) {
						"gate"
					} else {
						"injected"
					};
					events.push(Event::harness("steer", kind, &message.content));
				}
			}
			"assistant" => {
				if let Some(thinking) = crate::session::message_thinking_content(message) {
					events.push(Event::thinking(thinking));
				}
				if !message.content.trim().is_empty() {
					events.push(Event::assistant(&message.content));
				}
				if let Some(calls) = message.tool_calls.as_ref().and_then(|v| v.as_array()) {
					events.extend(calls.iter().filter_map(call_event));
				}
			}
			"tool" => {
				events.push(Event::tool_result(
					&message.content,
					result_status(&message.content),
				));
			}
			_ => {}
		}
	}
	match events
		.iter()
		.position(|e| matches!(e, Event::UserTurn { .. }))
	{
		Some(first) => {
			events.drain(..first);
		}
		None => events.clear(),
	}
	events
}

/// One recorded tool call (OpenAI `function` shape or Anthropic `input`
/// shape) as a canonical call event. octofs tools get their op from what the
/// call does; everything else goes through the reference name table.
fn call_event(call: &serde_json::Value) -> Option<Event> {
	let function = call.get("function");
	let name = function
		.and_then(|f| f.get("name"))
		.or_else(|| call.get("name"))
		.and_then(serde_json::Value::as_str)?;
	let raw_args = function
		.and_then(|f| f.get("arguments"))
		.or_else(|| call.get("arguments"))
		.or_else(|| call.get("input"))
		.cloned()
		.unwrap_or(serde_json::Value::Null);
	let mut args = match raw_args {
		serde_json::Value::String(text) => {
			serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
		}
		other => other,
	};
	if !args.is_object() {
		args = serde_json::json!({});
	}
	let command = args
		.get("command")
		.or_else(|| args.get("cmd"))
		.and_then(serde_json::Value::as_str);
	let op = match name {
		"view" => {
			if args.get("content").is_some() {
				Op::Search
			} else {
				Op::Read
			}
		}
		"text_editor" => {
			if command == Some("create") {
				Op::Write
			} else {
				Op::Edit
			}
		}
		"batch_edit" | "extract_lines" => Op::Edit,
		_ => op_for(name, command),
	};
	// `text_editor`'s `command` is a mode, not a shell line: renamed so the
	// rendering shows the edit's arguments instead of the word "str_replace".
	if name == "text_editor" {
		if let Some(map) = args.as_object_mut() {
			if let Some(mode) = map.remove("command") {
				map.insert("mode".to_string(), mode);
			}
		}
	}
	let path = matches!(op, Op::Read | Op::Edit | Op::Write)
		.then(|| {
			args.get("path")
				.or_else(|| args.get("file_path"))
				.and_then(serde_json::Value::as_str)
				.map(str::to_string)
		})
		.flatten();
	Some(Event::tool_call_with_op(name, op, &args, path))
}

/// Tool messages carry no error flag; the status is read off the text the
/// way the reference normalizers read harness markers.
fn result_status(content: &str) -> Status {
	let head: String = content
		.trim_start()
		.chars()
		.take(64)
		.collect::<String>()
		.to_ascii_lowercase();
	if head.starts_with("[request interrupted") || head.contains("interrupted by user") {
		Status::Interrupted
	} else if head.starts_with("blocked")
		|| head.starts_with("denied")
		|| head.starts_with("permission denied")
	{
		Status::Denied
	} else if head.starts_with("error") {
		Status::Error
	} else {
		Status::Ok
	}
}

#[cfg(test)]
#[path = "hindsight_tests.rs"]
mod tests;
