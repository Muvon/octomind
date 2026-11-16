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

//! The plugin host: connects plugins, runs the handshake per session, sends
//! events, applies the effects that belong to the host itself and keeps the
//! per-session state. The typed event functions live in `events.rs`.

use super::protocol::{
	self, event, Answer, Event, FileText, InitializeParams, InitializeResult, PlanState, PlanUpdate, PluginOptions,
	ProgramInfo, RuleMatch, RuleOutcome, Status, ToolInfo, Usage, METHOD_INITIALIZE, METHOD_SESSION_HISTORY,
	PROTOCOL_VERSION,
};
use super::read::{self, Limits};
use super::state::{
	events_table, queue_files, rule_ids, sanitize, set_block, tools_hash, wire_message, PluginBlock, PluginCommand,
	PluginSlot, PluginStatusField, SessionState, StoredBlock, MAX_CONSECUTIVE_FAILURES,
};
use super::transport::{Connector, Transport};
use crate::config::{PluginConfig, DEFAULT_PLUGIN_TIMEOUT_SECONDS, MAX_PLUGIN_TIMEOUT_SECONDS};
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Reading the git remote is a convenience; it never holds a session start for long.
const GIT_REMOTE_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_NAME: &str = "octomind";

/// Connecting a plugin that names a signed release may download its program first.
const INSTALL_CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// The time a plugin gets for connecting and answering `initialize`.
fn plugin_timeout(config: &PluginConfig) -> Duration {
	Duration::from_secs(
		config
			.timeout_seconds
			.unwrap_or(DEFAULT_PLUGIN_TIMEOUT_SECONDS)
			.min(MAX_PLUGIN_TIMEOUT_SECONDS),
	)
}

/// What an event is about, as the agent loop knows it.
pub struct EventContext<'a> {
	pub session_id: &'a str,
	pub parent_session_id: Option<&'a str>,
	pub role: &'a str,
	pub workdir: &'a Path,
	/// Name of the project the session works in.
	pub project: &'a str,
	/// The session's full message list now. The host sends each plugin what it has not seen.
	pub messages: &'a [crate::session::Message],
	pub status: Option<Status>,
	pub plan: Option<PlanState>,
	/// The current tool list, when the caller has it. Each plugin gets it only
	/// when it differs from the list that plugin saw last.
	pub tools: Option<Vec<ToolInfo>>,
	/// Names of the skills active in the session now.
	pub active_skills: Vec<String>,
}

/// Effects every answered event can carry back to the caller.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Effects {
	/// Text for the session inbox, as feedback managed by the system.
	pub notes: Vec<String>,
	/// The host's plan change. The first plugin that sent one wins.
	pub plan: Option<PlanUpdate>,
}

/// Whether the next plugin is asked.
pub enum Flow {
	Next,
	Stop,
}

/// Everything to send one plugin one event, read from the session state.
pub struct Prepared {
	pub name: String,
	pub options: PluginOptions,
	transport: Arc<dyn Transport>,
	timeout: Duration,
	envelope: Envelope,
}

struct Envelope {
	git_remote: Option<String>,
	messages: Vec<protocol::Message>,
	messages_reset: bool,
	rule_matches: Vec<RuleMatch>,
	plan_error: Option<String>,
	dropped_context: Vec<String>,
	/// The tool list to send, and its hash to remember once the event is answered.
	tools: Option<(Vec<ToolInfo>, u64)>,
	/// Length of the session's message list when this event was built.
	sent_through: usize,
	/// Texts of files that waited for this event.
	files: Vec<FileText>,
}

/// The notice that a plugin is off for the session. A notice is shown in an
/// interactive terminal only, and a session must not lose a plugin without a
/// word, so everywhere else the same text goes out as an error.
fn off_notice(message: String) -> String {
	if !crate::session::notice::notices_are_shown() {
		crate::log_error!("{}", message);
	}
	message
}

pub struct Host {
	connector: Arc<dyn Connector>,
	/// One connection per plugin per process, shared by all sessions.
	connections: tokio::sync::Mutex<HashMap<String, Arc<dyn Transport>>>,
	/// Never held across an await.
	sessions: parking_lot::Mutex<HashMap<String, SessionState>>,
}

impl Host {
	pub fn new(connector: Arc<dyn Connector>) -> Self {
		Self {
			connector,
			connections: tokio::sync::Mutex::new(HashMap::new()),
			sessions: parking_lot::Mutex::new(HashMap::new()),
		}
	}

	// -----------------------------------------------------------------------
	// Session lifecycle
	// -----------------------------------------------------------------------

	/// Connect every enabled plugin whose roles match and run its handshake. A
	/// plugin that fails it is off for this session, with one notice.
	pub async fn start_session(&self, session_id: &str, role: &str, workdir: &Path, plugins: &[PluginConfig]) {
		let mut slots = Vec::new();
		let mut notices = Vec::new();
		for config in plugins
			.iter()
			.filter(|config| config.enabled && config.applies_to_role(role))
		{
			match self.handshake(config).await {
				Ok(slot) => slots.push(slot),
				Err(error) => {
					crate::log_debug!("Plugin '{}' failed its handshake: {:#}", config.name, error);
					notices.push(off_notice(format!(
						"plugin '{}' is off for this session: {:#}",
						config.name, error
					)));
				}
			}
		}
		if slots.is_empty() && notices.is_empty() {
			return;
		}
		let git_remote = if slots.is_empty() {
			None
		} else {
			read_git_remote(workdir).await
		};
		self.sessions.lock().insert(
			session_id.to_string(),
			SessionState {
				workdir: workdir.to_path_buf(),
				git_remote,
				slots,
				plan_owner: None,
				notices,
			},
		);
	}

	pub fn end_session(&self, session_id: &str) {
		self.sessions.lock().remove(session_id);
	}

	async fn handshake(&self, config: &PluginConfig) -> Result<PluginSlot> {
		let transport = self.transport_for(config).await?;
		let timeout = plugin_timeout(config);
		let params = serde_json::to_value(InitializeParams {
			protocol: PROTOCOL_VERSION,
			host: ProgramInfo {
				name: HOST_NAME.to_string(),
				version: env!("CARGO_PKG_VERSION").to_string(),
			},
			events: events_table(),
		})
		.expect("initialize params serialize");
		let value = tokio::time::timeout(timeout, transport.request(METHOD_INITIALIZE, params, timeout))
			.await
			.map_err(|_| anyhow!("no answer to initialize within {}s", timeout.as_secs()))??;
		let result: InitializeResult = serde_json::from_value(value).context("unreadable answer to initialize")?;
		if result.protocol != PROTOCOL_VERSION {
			bail!(
				"it speaks protocol {} and octomind speaks {}",
				result.protocol,
				PROTOCOL_VERSION
			);
		}
		let mut subscribed = HashSet::new();
		for name in &result.subscribe {
			if event::ALL.contains(&name.as_str()) {
				subscribed.insert(name.clone());
			} else {
				crate::log_debug!(
					"Plugin '{}' subscribed to unknown event '{}'; ignored",
					config.name,
					name
				);
			}
		}
		Ok(PluginSlot::new(
			&config.name,
			transport,
			config.timeout_seconds,
			subscribed,
			&result,
		))
	}

	async fn transport_for(&self, config: &PluginConfig) -> Result<Arc<dyn Transport>> {
		let mut connections = self.connections.lock().await;
		if let Some(existing) = connections
			.get(&config.name)
			.filter(|transport| transport.is_connected())
		{
			return Ok(existing.clone());
		}
		// A signed release may have to be downloaded before the program starts.
		let limit = if config.install_manifest.is_some() {
			INSTALL_CONNECT_TIMEOUT
		} else {
			plugin_timeout(config)
		};
		let transport = tokio::time::timeout(limit, self.connector.connect(config))
			.await
			.map_err(|_| anyhow!("not connected within {}s", limit.as_secs()))??;
		connections.insert(config.name.clone(), transport.clone());
		Ok(transport)
	}

	// -----------------------------------------------------------------------
	// Queries and small updates
	// -----------------------------------------------------------------------

	/// A plugin is on for the session.
	pub fn active(&self, session_id: &str) -> bool {
		self.sessions
			.lock()
			.get(session_id)
			.is_some_and(|session| session.slots.iter().any(|slot| !slot.off))
	}

	/// A plugin that is on for the session wants `event_name`.
	pub fn subscribed(&self, session_id: &str, event_name: &str) -> bool {
		self.sessions
			.lock()
			.get(session_id)
			.is_some_and(|session| session.slots.iter().any(|slot| slot.takes(event_name)))
	}

	/// The history was rewritten: every plugin gets the full list with its next event.
	pub fn mark_history_reset(&self, session_id: &str) {
		if let Some(session) = self.sessions.lock().get_mut(session_id) {
			for slot in &mut session.slots {
				slot.reset_pending = true;
			}
		}
	}

	/// A rule `plugin` registered matched. It is reported with the plugin's next event.
	pub fn record_rule_match(&self, session_id: &str, plugin: &str, rule_id: &str, outcome: RuleOutcome) {
		let mut sessions = self.sessions.lock();
		let slot = sessions
			.get_mut(session_id)
			.and_then(|session| session.slots.iter_mut().find(|slot| slot.name == plugin && !slot.off));
		if let Some(slot) = slot {
			slot.rule_matches.push(RuleMatch {
				id: rule_id.to_string(),
				outcome,
			});
		}
	}

	/// Ids of the rules `plugin` registered.
	pub(super) fn registered_rule_ids(&self, session_id: &str, plugin: &str) -> HashSet<String> {
		self.sessions
			.lock()
			.get(session_id)
			.and_then(|session| session.slots.iter().find(|slot| slot.name == plugin))
			.map(|slot| slot.rule_ids.clone())
			.unwrap_or_default()
	}

	/// Context blocks of the plugins that are on, in config order.
	pub fn context_blocks(&self, session_id: &str) -> Vec<PluginBlock> {
		self.collect_blocks(session_id, false)
	}

	/// Pinned blocks of every plugin of the session, in config order. They stay when a plugin goes off.
	pub fn pinned_blocks(&self, session_id: &str) -> Vec<PluginBlock> {
		self.collect_blocks(session_id, true)
	}

	fn collect_blocks(&self, session_id: &str, pinned: bool) -> Vec<PluginBlock> {
		let sessions = self.sessions.lock();
		let Some(session) = sessions.get(session_id) else {
			return Vec::new();
		};
		session
			.slots
			.iter()
			.filter(|slot| pinned || !slot.off)
			.flat_map(|slot| {
				let blocks = if pinned { &slot.pinned } else { &slot.context };
				blocks.iter().map(|held| PluginBlock {
					plugin: slot.name.clone(),
					name: held.name.clone(),
					text: held.text.clone(),
					keep: held.keep,
					items: held.items.clone(),
				})
			})
			.collect()
	}

	/// Put back pinned blocks stored with a resumed session. Blocks of a plugin
	/// that is not connected are dropped; a block the plugin already set again wins.
	pub fn restore_pinned_blocks(&self, session_id: &str, blocks: Vec<PluginBlock>) {
		let mut sessions = self.sessions.lock();
		let Some(session) = sessions.get_mut(session_id) else {
			return;
		};
		for block in blocks {
			let Some(slot) = session.slots.iter_mut().find(|slot| slot.name == block.plugin) else {
				continue;
			};
			if !slot.pinned.iter().any(|held| held.name == block.name) {
				slot.pinned.push(StoredBlock {
					name: block.name,
					text: block.text,
					keep: block.keep,
					items: block.items,
				});
			}
		}
	}

	/// Status-line fields of every plugin of the session, from their handshakes.
	pub fn status_fields(&self, session_id: &str) -> Vec<PluginStatusField> {
		let sessions = self.sessions.lock();
		let Some(session) = sessions.get(session_id) else {
			return Vec::new();
		};
		session
			.slots
			.iter()
			.flat_map(|slot| {
				slot.status_fields.iter().map(|field| PluginStatusField {
					plugin: slot.name.clone(),
					name: field.name.clone(),
					instruction: field.instruction.clone(),
					default: field.default.clone(),
					example: field.example.clone(),
				})
			})
			.collect()
	}

	/// Put back status fields stored with a resumed session. Fields of a plugin
	/// that is not connected are dropped; a field the handshake supplied wins.
	pub fn restore_status_fields(&self, session_id: &str, fields: Vec<PluginStatusField>) {
		let mut sessions = self.sessions.lock();
		let Some(session) = sessions.get_mut(session_id) else {
			return;
		};
		for field in fields {
			let Some(slot) = session.slots.iter_mut().find(|slot| slot.name == field.plugin) else {
				continue;
			};
			if !slot.status_fields.iter().any(|known| known.name == field.name) {
				slot.status_fields.push(protocol::StatusField {
					name: field.name,
					instruction: field.instruction,
					default: field.default,
					example: field.example,
				});
			}
		}
	}

	/// Slash commands of the plugins that are on.
	pub fn commands(&self, session_id: &str) -> Vec<PluginCommand> {
		let sessions = self.sessions.lock();
		let Some(session) = sessions.get(session_id) else {
			return Vec::new();
		};
		session
			.slots
			.iter()
			.filter(|slot| !slot.off)
			.flat_map(|slot| {
				slot.commands.iter().map(|spec| PluginCommand {
					plugin: slot.name.clone(),
					spec: spec.clone(),
				})
			})
			.collect()
	}

	/// Rules text each plugin registered last.
	pub fn registered_rules(&self, session_id: &str) -> Vec<(String, String)> {
		let sessions = self.sessions.lock();
		let Some(session) = sessions.get(session_id) else {
			return Vec::new();
		};
		session
			.slots
			.iter()
			.filter_map(|slot| Some((slot.name.clone(), slot.rules.clone()?)))
			.collect()
	}

	/// Skill directories each plugin registered last.
	pub fn registered_skill_dirs(&self, session_id: &str) -> Vec<(String, protocol::SkillDir)> {
		let sessions = self.sessions.lock();
		let Some(session) = sessions.get(session_id) else {
			return Vec::new();
		};
		session
			.slots
			.iter()
			.flat_map(|slot| slot.skill_dirs.iter().map(|dir| (slot.name.clone(), dir.clone())))
			.collect()
	}

	/// What plugins spent since the last call.
	pub fn take_usage(&self, session_id: &str) -> Vec<(String, Usage)> {
		let mut sessions = self.sessions.lock();
		let Some(session) = sessions.get_mut(session_id) else {
			return Vec::new();
		};
		session
			.slots
			.iter_mut()
			.flat_map(|slot| {
				let name = slot.name.clone();
				std::mem::take(&mut slot.usage)
					.into_iter()
					.map(move |usage| (name.clone(), usage))
			})
			.collect()
	}

	/// The latest report of each plugin that sent one since the last call.
	pub fn take_reports(&self, session_id: &str) -> Vec<(String, protocol::Report)> {
		let mut sessions = self.sessions.lock();
		let Some(session) = sessions.get_mut(session_id) else {
			return Vec::new();
		};
		session
			.slots
			.iter_mut()
			.filter_map(|slot| Some((slot.name.clone(), slot.report.take()?)))
			.collect()
	}

	/// Notices for the user since the last call.
	pub fn take_notices(&self, session_id: &str) -> Vec<String> {
		self.sessions
			.lock()
			.get_mut(session_id)
			.map(|session| std::mem::take(&mut session.notices))
			.unwrap_or_default()
	}

	/// Options each plugin announced at its handshake, in config order.
	pub fn options(&self, session_id: &str) -> Vec<(String, PluginOptions)> {
		self.sessions
			.lock()
			.get(session_id)
			.map(|session| {
				session
					.slots
					.iter()
					.map(|slot| (slot.name.clone(), slot.options.clone()))
					.collect()
			})
			.unwrap_or_default()
	}

	/// The host could not apply the last `plan` effect. The plugin whose plan
	/// won the last merge hears why with its next event.
	pub fn record_plan_error(&self, session_id: &str, reason: &str) {
		let mut sessions = self.sessions.lock();
		let Some(session) = sessions.get_mut(session_id) else {
			return;
		};
		let Some(owner) = session.plan_owner.clone() else {
			return;
		};
		if let Some(slot) = session.slots.iter_mut().find(|slot| slot.name == owner) {
			slot.plan_error = Some(reason.to_string());
		}
	}

	/// The host left these blocks out of a model request for lack of room. Each
	/// plugin hears the names of its own with its next event.
	pub fn record_dropped_context(&self, session_id: &str, blocks: &[PluginBlock]) {
		let mut sessions = self.sessions.lock();
		let Some(session) = sessions.get_mut(session_id) else {
			return;
		};
		for block in blocks {
			if let Some(slot) = session.slots.iter_mut().find(|slot| slot.name == block.plugin) {
				if !slot.dropped_context.contains(&block.name) {
					slot.dropped_context.push(block.name.clone());
				}
			}
		}
	}

	// -----------------------------------------------------------------------
	// Sending
	// -----------------------------------------------------------------------

	/// Ask each plugin that takes `event_name`, in config order, and fold its
	/// answer into `state`. `build` makes the payload for one plugin from the
	/// state so far; `handle` applies the event's own effects and says whether
	/// the next plugin is asked. `only` limits the event to one plugin.
	pub(super) async fn run_event<S, P>(
		&self,
		event_name: &'static str,
		ctx: &EventContext<'_>,
		only: Option<&str>,
		mut state: S,
		build: impl Fn(&S, &Prepared) -> P,
		handle: impl Fn(&mut S, &str, &mut Answer) -> Flow,
	) -> (S, Effects)
	where
		P: Serialize,
	{
		let mut effects = Effects::default();
		for name in self.targets(ctx.session_id, event_name, only) {
			self.refresh_files(ctx, &name, event_name).await;
			let Some(prepared) = self.prepare(ctx, &name, event_name) else {
				continue;
			};
			let payload = build(&state, &prepared);
			let Some(mut answer) = self.exchange(ctx, event_name, prepared, payload).await else {
				continue;
			};
			effects.notes.append(&mut answer.note);
			if let Some(plan) = answer.plan.take() {
				if effects.plan.is_none() {
					effects.plan = Some(plan);
					if let Some(session) = self.sessions.lock().get_mut(ctx.session_id) {
						session.plan_owner = Some(name.clone());
					}
				} else {
					crate::log_debug!("Plugin '{}' sent a plan change after another plugin's; ignored", name);
				}
			}
			if matches!(handle(&mut state, &name, &mut answer), Flow::Stop) {
				break;
			}
		}
		(state, effects)
	}

	fn targets(&self, session_id: &str, event_name: &str, only: Option<&str>) -> Vec<String> {
		self.sessions
			.lock()
			.get(session_id)
			.map(|session| {
				session
					.slots
					.iter()
					.filter(|slot| slot.takes(event_name) && only.is_none_or(|name| name == slot.name))
					.map(|slot| slot.name.clone())
					.collect()
			})
			.unwrap_or_default()
	}

	fn prepare(&self, ctx: &EventContext<'_>, name: &str, event_name: &str) -> Option<Prepared> {
		let mut sessions = self.sessions.lock();
		let session = sessions.get_mut(ctx.session_id)?;
		let git_remote = session.git_remote.clone();
		let slot = session
			.slots
			.iter_mut()
			.find(|slot| slot.name == name && slot.takes(event_name))?;
		// A cursor past the list means the history shrank without a reset call.
		let messages_reset = slot.reset_pending || slot.seen > ctx.messages.len();
		let first = if messages_reset { 0 } else { slot.seen };
		let messages = ctx.messages[first..]
			.iter()
			.map(|message| wire_message(message, slot.options.result_chars))
			.collect();
		// `session.start` always carries the list and it is the reference; later
		// events carry it only when it differs from what this plugin saw last.
		let tools = match &ctx.tools {
			Some(list) if event_name != event::SESSION_START => {
				let hash = tools_hash(list);
				(slot.tools_hash != Some(hash)).then(|| (list.clone(), hash))
			}
			Some(list) => Some((list.clone(), tools_hash(list))),
			None if event_name == event::SESSION_START => Some((Vec::new(), tools_hash(&[]))),
			None => None,
		};
		Some(Prepared {
			name: slot.name.clone(),
			options: slot.options.clone(),
			transport: slot.transport.clone(),
			timeout: slot.limit(event_name),
			envelope: Envelope {
				git_remote,
				messages,
				messages_reset,
				rule_matches: slot.rule_matches.clone(),
				plan_error: slot.plan_error.clone(),
				dropped_context: slot.dropped_context.clone(),
				files: slot.waiting_files.clone(),
				tools,
				sent_through: ctx.messages.len(),
			},
		})
	}

	/// Send one event to one plugin. `None` means the plugin did not answer; the
	/// failure is already counted and the agent goes on without it.
	async fn exchange<P: Serialize>(
		&self,
		ctx: &EventContext<'_>,
		event_name: &str,
		prepared: Prepared,
		payload: P,
	) -> Option<Answer> {
		let Prepared {
			name,
			transport,
			timeout,
			envelope,
			..
		} = prepared;
		let params = Event {
			session_id: ctx.session_id.to_string(),
			parent_session_id: ctx.parent_session_id.map(str::to_string),
			role: ctx.role.to_string(),
			workdir: ctx.workdir.to_string_lossy().into_owned(),
			project: ctx.project.to_string(),
			git_remote: envelope.git_remote.clone(),
			messages: envelope.messages.clone(),
			messages_reset: envelope.messages_reset,
			status: ctx.status.clone(),
			rule_matches: envelope.rule_matches.clone(),
			plan: ctx.plan.clone(),
			plan_error: envelope.plan_error.clone(),
			tools: envelope.tools.as_ref().map(|(tools, _)| tools.clone()),
			active_skills: ctx.active_skills.clone(),
			dropped_context: envelope.dropped_context.clone(),
			files: envelope.files.clone(),
			payload,
		};
		match deliver(transport.as_ref(), event_name, &params, timeout).await {
			Ok(mut answer) => {
				sanitize(&name, event_name, &mut answer);
				let wants_history = answer.history;
				let asked = self.accept(ctx.session_id, &name, &envelope, &mut answer);
				if !asked.is_empty() {
					self.read_files(ctx, &name, &asked).await;
				}
				if wants_history {
					self.send_history(ctx, &name).await;
				}
				Some(answer)
			}
			Err(error) => {
				self.reject(ctx.session_id, &name, event_name, &error);
				None
			}
		}
	}

	/// Apply the effects that belong to the host: what it keeps per plugin.
	/// Returns the paths and globs the plugin asked to read.
	fn accept(&self, session_id: &str, name: &str, envelope: &Envelope, answer: &mut Answer) -> Vec<String> {
		let mut sessions = self.sessions.lock();
		let Some(session) = sessions.get_mut(session_id) else {
			return Vec::new();
		};
		let SessionState { slots, notices, .. } = session;
		let Some(slot) = slots.iter_mut().find(|slot| slot.name == name) else {
			return Vec::new();
		};
		slot.failures = 0;
		slot.seen = envelope.sent_through;
		slot.reset_pending = false;
		// Taken out only now, so an event that is dropped before it is answered
		// leaves them for the next one. Entries added while it ran stay.
		slot.rule_matches
			.drain(..envelope.rule_matches.len().min(slot.rule_matches.len()));
		slot.waiting_files
			.drain(..envelope.files.len().min(slot.waiting_files.len()));
		if slot.plan_error == envelope.plan_error {
			slot.plan_error = None;
		}
		slot.dropped_context
			.retain(|name| !envelope.dropped_context.contains(name));
		if let Some((_, hash)) = &envelope.tools {
			slot.tools_hash = Some(*hash);
		}
		for block in answer.context.drain(..) {
			set_block(&mut slot.context, block);
		}
		for block in answer.pin.drain(..) {
			set_block(&mut slot.pinned, block);
		}
		slot.usage.append(&mut answer.usage);
		if let Some(report) = answer.report.take() {
			slot.report = Some(report);
		}
		notices.append(&mut answer.notice);
		if let Some(register) = answer.register.take() {
			slot.rule_ids = register.rules.as_deref().map(rule_ids).unwrap_or_default();
			slot.rules = register.rules;
			slot.skill_dirs = register.skill_dirs;
		}
		let asked = std::mem::take(&mut answer.read);
		for entry in &asked {
			if !slot.read_entries.contains(entry) {
				slot.read_entries.push(entry.clone());
			}
		}
		asked
	}

	/// Run `change` on one plugin's slot. `None` when the session or the plugin is gone.
	fn with_slot<T>(&self, session_id: &str, name: &str, change: impl FnOnce(&mut PluginSlot) -> T) -> Option<T> {
		let mut sessions = self.sessions.lock();
		let slot = sessions
			.get_mut(session_id)?
			.slots
			.iter_mut()
			.find(|slot| slot.name == name)?;
		Some(change(slot))
	}

	/// Read what the plugin asked for with `read`. The disk is touched with no lock held.
	async fn read_files(&self, ctx: &EventContext<'_>, name: &str, entries: &[String]) {
		let Some((limits, mut stamps)) = self.with_slot(ctx.session_id, name, |slot| {
			(Limits::of(&slot.options), slot.file_stamps.clone())
		}) else {
			return;
		};
		let texts = read::read_entries(ctx.workdir, entries, &mut stamps, limits).await;
		self.with_slot(ctx.session_id, name, |slot| {
			slot.file_stamps = stamps;
			queue_files(&mut slot.waiting_files, texts);
		});
	}

	/// Before `prompt.submit` and `tool.after`: queue the files that changed
	/// since they were sent. Before `prompt.submit` only, files that match a
	/// glob the plugin gave and were not sent yet are read too.
	async fn refresh_files(&self, ctx: &EventContext<'_>, name: &str, event_name: &str) {
		if !matches!(event_name, event::PROMPT_SUBMIT | event::TOOL_AFTER) {
			return;
		}
		let Some((limits, mut stamps, mut globs)) = self.with_slot(ctx.session_id, name, |slot| {
			let globs: Vec<String> = slot
				.read_entries
				.iter()
				.filter(|entry| read::is_glob(entry))
				.cloned()
				.collect();
			(Limits::of(&slot.options), slot.file_stamps.clone(), globs)
		}) else {
			return;
		};
		if event_name != event::PROMPT_SUBMIT {
			globs.clear();
		}
		if stamps.is_empty() && globs.is_empty() {
			return;
		}
		let texts = read::refresh(ctx.workdir, &mut stamps, &globs, limits).await;
		self.with_slot(ctx.session_id, name, |slot| {
			slot.file_stamps = stamps;
			queue_files(&mut slot.waiting_files, texts);
		});
	}

	/// The event failed or timed out: count it, tell the user, and switch the
	/// plugin off for the session after too many in a row.
	fn reject(&self, session_id: &str, name: &str, event_name: &str, error: &anyhow::Error) {
		crate::log_debug!("Plugin '{}' did not answer {}: {:#}", name, event_name, error);
		let mut sessions = self.sessions.lock();
		let Some(session) = sessions.get_mut(session_id) else {
			return;
		};
		let SessionState { slots, notices, .. } = session;
		let Some(slot) = slots.iter_mut().find(|slot| slot.name == name) else {
			return;
		};
		slot.failures += 1;
		if slot.failures >= MAX_CONSECUTIVE_FAILURES {
			slot.off = true;
			notices.push(off_notice(format!(
				"plugin '{}' is off for this session after {} failures in a row",
				name, MAX_CONSECUTIVE_FAILURES
			)));
		} else {
			notices.push(format!("plugin '{}' did not answer {}: {:#}", name, event_name, error));
		}
	}

	/// `history: true`: send the session's full message list once, as a notification.
	async fn send_history(&self, ctx: &EventContext<'_>, name: &str) {
		let target = {
			let sessions = self.sessions.lock();
			sessions.get(ctx.session_id).and_then(|session| {
				let slot = session.slots.iter().find(|slot| slot.name == name)?;
				Some((
					slot.transport.clone(),
					slot.options.result_chars,
					session.git_remote.clone(),
					slot.limit(METHOD_SESSION_HISTORY),
				))
			})
		};
		let Some((transport, result_chars, git_remote, timeout)) = target else {
			return;
		};
		let params = Event {
			session_id: ctx.session_id.to_string(),
			parent_session_id: ctx.parent_session_id.map(str::to_string),
			role: ctx.role.to_string(),
			workdir: ctx.workdir.to_string_lossy().into_owned(),
			project: ctx.project.to_string(),
			git_remote,
			messages: ctx
				.messages
				.iter()
				.map(|message| wire_message(message, result_chars))
				.collect(),
			messages_reset: true,
			status: ctx.status.clone(),
			rule_matches: Vec::new(),
			plan: ctx.plan.clone(),
			plan_error: None,
			tools: None,
			active_skills: ctx.active_skills.clone(),
			dropped_context: Vec::new(),
			files: Vec::new(),
			payload: serde_json::Map::<String, Value>::new(),
		};
		let params = serde_json::to_value(&params).expect("history event serializes");
		let sent = tokio::time::timeout(timeout, transport.notify(METHOD_SESSION_HISTORY, params))
			.await
			.map_err(|_| anyhow!("not taken within {}s", timeout.as_secs()))
			.and_then(|result| result);
		if let Err(error) = sent {
			self.reject(ctx.session_id, name, METHOD_SESSION_HISTORY, &error);
		}
	}
}

/// Send the event as a request, or as a notification when it has no answer.
async fn deliver<P: Serialize>(
	transport: &dyn Transport,
	event_name: &str,
	params: &Event<P>,
	timeout: Duration,
) -> Result<Answer> {
	let params = serde_json::to_value(params).context("Failed to encode the event")?;
	if event::NOTIFICATIONS.contains(&event_name) {
		tokio::time::timeout(timeout, transport.notify(event_name, params))
			.await
			.map_err(|_| anyhow!("not taken within {}s", timeout.as_secs()))??;
		return Ok(Answer::default());
	}
	let value = tokio::time::timeout(timeout, transport.request(event_name, params, timeout))
		.await
		.map_err(|_| anyhow!("no answer within {}s", timeout.as_secs()))??;
	if value.is_null() {
		return Ok(Answer::default());
	}
	serde_json::from_value(value).context("unreadable answer")
}

async fn read_git_remote(workdir: &Path) -> Option<String> {
	let output = tokio::process::Command::new("git")
		.args(["remote", "get-url", "origin"])
		.current_dir(workdir)
		.stdin(std::process::Stdio::null())
		.kill_on_drop(true)
		.output();
	match tokio::time::timeout(GIT_REMOTE_TIMEOUT, output).await {
		Ok(Ok(output)) if output.status.success() => {
			let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
			(!remote.is_empty()).then_some(remote)
		}
		Ok(Ok(_)) => None,
		Ok(Err(error)) => {
			crate::log_debug!("Could not run git to read the remote: {}", error);
			None
		}
		Err(_) => {
			crate::log_debug!("Reading the git remote timed out");
			None
		}
	}
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
