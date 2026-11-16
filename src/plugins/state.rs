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

//! What the host keeps per session and per plugin, and the pure helpers that
//! decide what an answer may do.

use super::protocol::{
	self, effect, event, Answer, Block, CommandSpec, FileText, PluginOptions, RuleMatch, SkillDir, StatusField, Usage,
};
use super::read::Stamp;
use super::transport::Transport;
use crate::config::{DEFAULT_PLUGIN_TIMEOUT_SECONDS, MAX_PLUGIN_TIMEOUT_SECONDS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Failed or unanswered events in a row after which a plugin is off for the session.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Effects every answered event allows.
const COMMON_EFFECTS: [&str; 10] = [
	effect::CONTEXT,
	effect::PIN,
	effect::USAGE,
	effect::REPORT,
	effect::READ,
	effect::NOTICE,
	effect::REGISTER,
	effect::HISTORY,
	effect::PLAN,
	effect::NOTE,
];

/// A named block of text that a plugin set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginBlock {
	pub plugin: String,
	pub name: String,
	pub text: String,
	/// `context`: keep the block when the host drops blocks for room.
	#[serde(default)]
	pub keep: bool,
	/// `pin`: lines the host keeps as separate items when it folds history.
	#[serde(default)]
	pub items: Vec<String>,
}

/// A status-line field a plugin added at its handshake.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginStatusField {
	pub plugin: String,
	pub name: String,
	pub instruction: String,
	#[serde(default)]
	pub default: Value,
	#[serde(default)]
	pub example: Option<Value>,
}

/// A slash command a plugin provides.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginCommand {
	pub plugin: String,
	pub spec: CommandSpec,
}

/// A block as one plugin holds it, in the order the plugin first set it.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredBlock {
	pub name: String,
	pub text: String,
	pub keep: bool,
	pub items: Vec<String>,
}

/// Set a block, replacing one of the same name in place; no text removes it.
pub fn set_block(blocks: &mut Vec<StoredBlock>, block: Block) {
	let position = blocks.iter().position(|held| held.name == block.name);
	match (block.text, position) {
		(Some(text), Some(at)) => {
			blocks[at] = StoredBlock {
				name: block.name,
				text,
				keep: block.keep,
				items: block.items,
			}
		}
		(Some(text), None) => blocks.push(StoredBlock {
			name: block.name,
			text,
			keep: block.keep,
			items: block.items,
		}),
		(None, Some(at)) => {
			blocks.remove(at);
		}
		(None, None) => {}
	}
}

/// Queue file texts for the next event; a newer text of a file replaces the one waiting.
pub fn queue_files(waiting: &mut Vec<FileText>, texts: Vec<FileText>) {
	for text in texts {
		waiting.retain(|held| held.path != text.path);
		waiting.push(text);
	}
}

/// The time one event gets. A plugin declares what it needs per event; the
/// default covers an event it did not list. A limit the user set caps it.
pub fn event_limit(user_limit: Option<u64>, declared: Option<u64>) -> Duration {
	let declared = declared.unwrap_or(DEFAULT_PLUGIN_TIMEOUT_SECONDS);
	let seconds = user_limit.map_or(declared, |user| user.min(declared));
	Duration::from_secs(seconds.clamp(1, MAX_PLUGIN_TIMEOUT_SECONDS))
}

/// One plugin inside one session.
pub struct PluginSlot {
	pub name: String,
	pub transport: Arc<dyn Transport>,
	/// `timeout_seconds` of the config entry.
	pub user_limit: Option<u64>,
	/// Seconds per event, as the plugin declared them.
	pub declared_limits: BTreeMap<String, u64>,
	pub subscribed: HashSet<String>,
	pub options: PluginOptions,
	pub status_fields: Vec<StatusField>,
	pub commands: Vec<CommandSpec>,
	/// Off for the rest of the session: it sends nothing and is asked nothing.
	pub off: bool,
	pub failures: u32,
	/// How many messages of the session this plugin has been sent.
	pub seen: usize,
	/// The host rewrote the history: the next event carries the full list.
	pub reset_pending: bool,
	pub rule_matches: Vec<RuleMatch>,
	/// Hash of the tool list this plugin saw last.
	pub tools_hash: Option<u64>,
	/// The host could not apply this plugin's last plan change.
	pub plan_error: Option<String>,
	/// Names of this plugin's context blocks the host left out of a request.
	pub dropped_context: Vec<String>,
	pub context: Vec<StoredBlock>,
	pub pinned: Vec<StoredBlock>,
	pub rules: Option<String>,
	/// Ids found in `rules`; only these may be lifted by this plugin.
	pub rule_ids: HashSet<String>,
	pub skill_dirs: Vec<SkillDir>,
	pub usage: Vec<Usage>,
	/// The plugin's latest report, until the session books it.
	pub report: Option<protocol::Report>,
	/// `read`: the paths and globs the plugin asked for so far, without repeats.
	pub read_entries: Vec<String>,
	/// `read`: what was sent for each file, to tell when it changed.
	pub file_stamps: BTreeMap<String, Stamp>,
	/// `read`: texts that go out with the next event.
	pub waiting_files: Vec<FileText>,
}

impl PluginSlot {
	pub fn new(
		name: &str,
		transport: Arc<dyn Transport>,
		user_limit: Option<u64>,
		subscribed: HashSet<String>,
		result: &protocol::InitializeResult,
	) -> Self {
		Self {
			name: name.to_string(),
			transport,
			user_limit,
			declared_limits: result.timeouts.clone(),
			subscribed,
			options: result.options.clone(),
			status_fields: result.status_fields.clone(),
			commands: result.commands.clone(),
			off: false,
			failures: 0,
			seen: 0,
			reset_pending: false,
			rule_matches: Vec::new(),
			tools_hash: None,
			plan_error: None,
			dropped_context: Vec::new(),
			context: Vec::new(),
			pinned: Vec::new(),
			rules: None,
			rule_ids: HashSet::new(),
			skill_dirs: Vec::new(),
			usage: Vec::new(),
			report: None,
			read_entries: Vec::new(),
			file_stamps: BTreeMap::new(),
			waiting_files: Vec::new(),
		}
	}

	pub fn takes(&self, event: &str) -> bool {
		!self.off && self.subscribed.contains(event)
	}

	/// Time one `event` gets.
	pub fn limit(&self, event: &str) -> Duration {
		event_limit(self.user_limit, self.declared_limits.get(event).copied())
	}
}

pub struct SessionState {
	pub workdir: PathBuf,
	pub git_remote: Option<String>,
	/// In config order.
	pub slots: Vec<PluginSlot>,
	/// The plugin whose `plan` effect won the last merge.
	pub plan_owner: Option<String>,
	pub notices: Vec<String>,
}

/// Effects `event` allows. A notification allows none.
pub fn allowed_effects(event_name: &str) -> Vec<&'static str> {
	if event::NOTIFICATIONS.contains(&event_name) {
		return Vec::new();
	}
	let mut allowed = COMMON_EFFECTS.to_vec();
	if matches!(event_name, event::COMMAND | event::STATUS) {
		allowed.retain(|name| !matches!(*name, effect::NOTE | effect::PLAN));
	}
	let extra: &[&str] = match event_name {
		event::PROMPT_SUBMIT => &[effect::REPLACE_PROMPT, effect::BLOCK],
		event::TOOL_BEFORE => &[effect::DENY, effect::LIFT],
		event::TOOL_AFTER => &[effect::REPLACE, effect::STOP],
		event::TURN_END => &[effect::CONTINUE, effect::VERIFIED],
		event::SUBAGENT_START => &[effect::INHERIT],
		event::COMMAND => &[effect::OUTPUT],
		event::STATUS => &[effect::ROWS],
		_ => &[],
	};
	allowed.extend(extra);
	allowed
}

/// The events octomind can send, each with the effects it applies for it.
pub fn events_table() -> BTreeMap<String, Vec<String>> {
	event::ALL
		.iter()
		.map(|name| {
			let effects = allowed_effects(name).into_iter().map(str::to_string).collect();
			(name.to_string(), effects)
		})
		.collect()
}

/// Drop every effect `event_name` does not allow. Every field of [`Answer`] is
/// named like its effect, so the answer's own JSON is the list of what was asked.
pub fn sanitize(plugin: &str, event_name: &str, answer: &mut Answer) {
	let allowed = allowed_effects(event_name);
	let Value::Object(mut fields) = serde_json::to_value(&*answer).expect("an answer serializes") else {
		unreachable!("an answer serializes to an object");
	};
	let asked = fields.len();
	fields.retain(|name, _| {
		let keep = allowed.contains(&name.as_str());
		if !keep {
			crate::log_debug!(
				"Plugin '{}' sent effect '{}' that {} does not allow; ignored",
				plugin,
				name,
				event_name
			);
		}
		keep
	});
	if fields.len() != asked {
		*answer = serde_json::from_value(Value::Object(fields)).expect("a subset of an answer is an answer");
	}
}

/// Ids of the rules in a `register.rules` text (the format of `.agents/guardrails.toml`).
pub fn rule_ids(rules: &str) -> HashSet<String> {
	let table: toml::Table = match toml::from_str(rules) {
		Ok(table) => table,
		Err(error) => {
			crate::log_debug!("Registered rules are not valid TOML: {}", error);
			return HashSet::new();
		}
	};
	table
		.values()
		.filter_map(toml::Value::as_array)
		.flatten()
		.filter_map(|rule| rule.get("id")?.as_str())
		.map(str::to_string)
		.collect()
}

/// Cut `text` to `limit` characters.
fn truncate_chars(text: &str, limit: usize) -> String {
	match text.char_indices().nth(limit) {
		Some((end, _)) => text[..end].to_string(),
		None => text.to_string(),
	}
}

/// The wire form of a session message. `result_chars` cuts tool results.
pub fn wire_message(message: &crate::session::Message, result_chars: Option<usize>) -> protocol::Message {
	let content = match (message.role.as_str(), result_chars) {
		("tool", Some(limit)) => truncate_chars(&message.content, limit),
		_ => message.content.clone(),
	};
	protocol::Message {
		managed: message.role == "user" && crate::session::is_system_managed_user_content(&message.content),
		role: message.role.clone(),
		content,
		timestamp: message.timestamp,
		id: message.id.clone(),
		name: message.name.clone(),
		tool_call_id: message.tool_call_id.clone(),
		tool_calls: message.tool_calls.clone(),
		thinking: crate::session::message_thinking_content(message).map(str::to_string),
	}
}

/// Hash of a tool list, to tell a plugin only when the list changed.
pub fn tools_hash(tools: &[protocol::ToolInfo]) -> u64 {
	use std::hash::{Hash, Hasher};
	let mut hasher = std::collections::hash_map::DefaultHasher::new();
	serde_json::to_string(tools).expect("tools serialize").hash(&mut hasher);
	hasher.finish()
}
#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
