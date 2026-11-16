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

//! Plugin host. A plugin is a separate program that takes part in the agent
//! loop: octomind sends it an event at each fixed point and applies the
//! effects it answers with. `protocol` is the wire contract; `host` keeps the
//! per-session state and sends events; `events` holds one typed function per
//! event; `transport` is how the bytes travel.
//!
//! The functions below are the process-wide entry points. A plugin that is not
//! listed in the config, or is off for the session, is never asked, so every
//! function is a cheap no-op without plugins.

pub mod apply;
pub mod events;
pub mod ground_truth;
pub mod host;
pub mod install;
pub mod judgement;
pub mod lifecycle;
pub mod payload;
pub mod project;
pub mod protocol;
pub mod read;
#[cfg(unix)]
pub mod socket;
pub mod state;
#[cfg(test)]
pub mod test_support;
pub mod transport;
pub mod workdir;

pub use events::{Judged, PromptOutcome, ToolAfterOutcome, ToolBeforeOutcome, TurnEndOutcome};
pub use host::{Effects, EventContext, Host};
pub use state::{PluginBlock, PluginCommand, PluginStatusField};

/// A plugin stopped the user's prompt. Carries the plugin's message, which the
/// user sees in place of an answer. The session goes on.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PromptBlocked(pub String);

use crate::config::PluginConfig;
use protocol::{
	Command, CommandOutput, CompactAfter, CompactBefore, Judge, PluginOptions, PromptSubmit, RequestBefore, Row,
	RuleOutcome, SessionEnd, SessionStart, SkillDir, SubagentEnd, SubagentStart, ToolAfter, ToolBefore, TurnEnd, Usage,
};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, OnceLock};

static HOST: OnceLock<Host> = OnceLock::new();

/// One host per process: the connection to a plugin is shared by all sessions.
/// The functions below only forward to it; tests drive a `Host` of their own.
fn host() -> &'static Host {
	#[cfg(not(test))]
	let connector = transport::ProcessConnector;
	// Tests of other modules switch stub plugins on with `test_support::connect_stub`.
	#[cfg(test)]
	let connector = test_support::StubConnector;
	HOST.get_or_init(|| Host::new(Arc::new(connector)))
}

// Session lifecycle ---------------------------------------------------------

/// Connect the enabled plugins whose roles match and run their handshakes.
pub async fn start_session(session_id: &str, role: &str, workdir: &Path, plugins: &[PluginConfig]) {
	host().start_session(session_id, role, workdir, plugins).await
}

pub fn end_session(session_id: &str) {
	host().end_session(session_id)
}

/// A plugin is on for the session.
pub fn active(session_id: &str) -> bool {
	host().active(session_id)
}

/// A plugin that is on for the session wants `event`.
pub fn subscribed(session_id: &str, event: &str) -> bool {
	host().subscribed(session_id, event)
}

/// The history was rewritten (compaction, resume, rollback): the next event to
/// each plugin carries the full message list.
pub fn mark_history_reset(session_id: &str) {
	host().mark_history_reset(session_id)
}

/// A rule `plugin` registered matched. It goes out with that plugin's next event.
pub fn record_rule_match(session_id: &str, plugin: &str, rule_id: &str, outcome: RuleOutcome) {
	host().record_rule_match(session_id, plugin, rule_id, outcome)
}

/// `count` judgements `plugin` gave for `purpose` changed what octomind did.
/// The plugin hears it with its next event.
pub fn record_judged(session_id: &str, plugin: &str, purpose: &str, count: u64) {
	host().record_judged(session_id, plugin, purpose, count)
}

/// The host could not apply the last `plan` effect. The plugin whose plan
/// won the last merge hears why with its next event.
pub fn record_plan_error(session_id: &str, reason: &str) {
	host().record_plan_error(session_id, reason)
}

/// The host left these context blocks out of a model request for lack of room.
pub fn record_dropped_context(session_id: &str, blocks: &[PluginBlock]) {
	host().record_dropped_context(session_id, blocks)
}

/// Options each plugin announced at its handshake, in config order.
pub fn options(session_id: &str) -> Vec<(String, PluginOptions)> {
	host().options(session_id)
}

// Events --------------------------------------------------------------------

pub async fn session_start(ctx: &EventContext<'_>, payload: SessionStart, inherited: &[(String, Value)]) -> Effects {
	host().session_start(ctx, payload, inherited).await
}

pub async fn prompt_submit(ctx: &EventContext<'_>, payload: PromptSubmit) -> PromptOutcome {
	host().prompt_submit(ctx, payload).await
}

pub async fn request_before(ctx: &EventContext<'_>, payload: RequestBefore) -> Effects {
	host().request_before(ctx, payload).await
}

pub async fn tool_before(ctx: &EventContext<'_>, payload: ToolBefore) -> ToolBeforeOutcome {
	host().tool_before(ctx, payload).await
}

pub async fn tool_after(ctx: &EventContext<'_>, payload: ToolAfter) -> ToolAfterOutcome {
	host().tool_after(ctx, payload).await
}

pub async fn turn_end(ctx: &EventContext<'_>, payload: TurnEnd) -> TurnEndOutcome {
	host().turn_end(ctx, payload).await
}

pub async fn compact_before(ctx: &EventContext<'_>, payload: CompactBefore) -> Effects {
	host().compact_before(ctx, payload).await
}

pub async fn compact_after(ctx: &EventContext<'_>, payload: CompactAfter) {
	host().compact_after(ctx, payload).await
}

pub async fn subagent_start(ctx: &EventContext<'_>, payload: SubagentStart) -> Vec<(String, Value)> {
	host().subagent_start(ctx, payload).await
}

pub async fn subagent_end(ctx: &EventContext<'_>, payload: SubagentEnd) {
	host().subagent_end(ctx, payload).await
}

pub async fn session_end(ctx: &EventContext<'_>, payload: SessionEnd) {
	host().session_end(ctx, payload).await
}

/// Ask the plugins for a judgement. `None` when no plugin gave one.
pub async fn judge(ctx: &EventContext<'_>, payload: Judge) -> Option<Judged> {
	host().judge(ctx, payload).await
}

pub async fn command(ctx: &EventContext<'_>, plugin: &str, payload: Command) -> Option<CommandOutput> {
	host().command(ctx, plugin, payload).await
}

pub async fn status(ctx: &EventContext<'_>) -> Vec<(String, Vec<Row>)> {
	host().status(ctx).await
}

// State the host keeps for the rest of octomind ------------------------------

pub fn context_blocks(session_id: &str) -> Vec<PluginBlock> {
	host().context_blocks(session_id)
}

pub fn pinned_blocks(session_id: &str) -> Vec<PluginBlock> {
	host().pinned_blocks(session_id)
}

pub fn restore_pinned_blocks(session_id: &str, blocks: Vec<PluginBlock>) {
	host().restore_pinned_blocks(session_id, blocks)
}

pub fn status_fields(session_id: &str) -> Vec<PluginStatusField> {
	host().status_fields(session_id)
}

pub fn restore_status_fields(session_id: &str, fields: Vec<PluginStatusField>) {
	host().restore_status_fields(session_id, fields)
}

pub fn commands(session_id: &str) -> Vec<PluginCommand> {
	host().commands(session_id)
}

/// `(plugin, rules text)`: the rules each plugin registered last.
pub fn registered_rules(session_id: &str) -> Vec<(String, String)> {
	host().registered_rules(session_id)
}

pub fn registered_skill_dirs(session_id: &str) -> Vec<(String, SkillDir)> {
	host().registered_skill_dirs(session_id)
}

/// What plugins spent since the last call, as `(plugin, usage)`.
pub fn take_usage(session_id: &str) -> Vec<(String, Usage)> {
	host().take_usage(session_id)
}

/// Move the usage the plugins of session `from` left unbooked to the same
/// plugins of session `to`.
pub fn pass_usage(from: &str, to: &str) {
	host().pass_usage(from, to)
}

/// Milliseconds the agent waited for the plugins of the session since the last
/// call. Events that ran at the same time count once.
pub fn take_waited_ms(session_id: &str) -> u64 {
	host().take_waited_ms(session_id)
}

/// The latest report of each plugin that sent one since the last call, as `(plugin, report)`.
pub fn take_reports(session_id: &str) -> Vec<(String, protocol::Report)> {
	host().take_reports(session_id)
}

/// Notices for the user since the last call.
pub fn take_notices(session_id: &str) -> Vec<String> {
	host().take_notices(session_id)
}
