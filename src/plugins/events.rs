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

//! One typed function per event. Each says which plugin answers win, and how
//! the answers of several plugins combine.

use super::host::{Effects, EventContext, Flow, Host};
use super::protocol::{
	event, Command, CommandOutput, CompactAfter, CompactBefore, Deny, Judge, Judgement, Lift, PromptSubmit, Replace,
	RequestBefore, Returned, Row, SessionEnd, SessionStart, StatusRequest, SubagentEnd, SubagentStart, ToolAfter,
	ToolBefore, TurnEnd,
};
use super::state::truncate_chars;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub struct PromptOutcome {
	/// The prompt after every plugin's replacement.
	pub prompt: String,
	/// A plugin stopped the prompt and wants the user to see this.
	pub block: Option<String>,
	pub effects: Effects,
}

pub struct ToolBeforeOutcome {
	pub denies: Vec<Deny>,
	/// Only for rules the same plugin registered.
	pub lifts: Vec<Lift>,
	pub effects: Effects,
}

pub struct ToolAfterOutcome {
	/// At most one per call: a later plugin's text replaces an earlier one's.
	pub replaces: Vec<Replace>,
	/// End the turn with this notice.
	pub stop: Option<String>,
	pub effects: Effects,
}

pub struct TurnEndOutcome {
	/// Keep the turn going with this text as the next input.
	pub continue_with: Option<String>,
	/// Every plugin that judged the turn found it complete.
	pub verified: Option<bool>,
	pub effects: Effects,
}

/// The judgements one plugin gave for a `judge` event.
pub struct Judged {
	/// The plugin that answered: [`Host::record_judged`] names it.
	pub plugin: String,
	/// By question id, and by item id for the items.
	pub judgements: BTreeMap<String, Judgement>,
}

struct ToolAfterState {
	/// The payload as the next plugin sees it: results already replaced are patched in.
	payload: ToolAfter,
	replaced: HashSet<String>,
	replaces: Vec<Replace>,
	stop: Option<String>,
}

impl Host {
	/// `inherited` holds the value each plugin's parent handed down.
	pub async fn session_start(
		&self,
		ctx: &EventContext<'_>,
		payload: SessionStart,
		inherited: &[(String, Value)],
	) -> Effects {
		let (_, effects) = self
			.run_event(
				event::SESSION_START,
				ctx,
				None,
				(),
				|_, plugin| {
					let mut payload = payload.clone();
					payload.inherited = inherited
						.iter()
						.find(|(name, _)| *name == plugin.name)
						.map(|(_, value)| value.clone());
					payload
				},
				|_, _, _| Flow::Next,
			)
			.await;
		effects
	}

	/// The first plugin that blocks wins and later plugins are not asked. Each
	/// plugin sees the prompt the one before it produced.
	pub async fn prompt_submit(&self, ctx: &EventContext<'_>, payload: PromptSubmit) -> PromptOutcome {
		let (state, effects) = self
			.run_event(
				event::PROMPT_SUBMIT,
				ctx,
				None,
				(payload.prompt.clone(), None::<String>),
				|(prompt, _), _| PromptSubmit {
					prompt: prompt.clone(),
					original: payload.original.clone(),
					carried_summary: payload.carried_summary.clone(),
					goal: payload.goal.clone(),
				},
				|(prompt, block), _, answer| {
					if let Some(text) = answer.replace_prompt.take() {
						*prompt = text;
					}
					match answer.block.take() {
						Some(message) => {
							*block = Some(message);
							Flow::Stop
						}
						None => Flow::Next,
					}
				},
			)
			.await;
		PromptOutcome {
			prompt: state.0,
			block: state.1,
			effects,
		}
	}

	pub async fn request_before(&self, ctx: &EventContext<'_>, payload: RequestBefore) -> Effects {
		let (_, effects) = self
			.run_event(
				event::REQUEST_BEFORE,
				ctx,
				None,
				(),
				|_, _| &payload,
				|_, _, _| Flow::Next,
			)
			.await;
		effects
	}

	/// The first plugin that denies wins and later plugins are not asked.
	pub async fn tool_before(&self, ctx: &EventContext<'_>, payload: ToolBefore) -> ToolBeforeOutcome {
		let (state, effects) = self
			.run_event(
				event::TOOL_BEFORE,
				ctx,
				None,
				(Vec::<Deny>::new(), Vec::<Lift>::new()),
				|_, _| &payload,
				|(denies, lifts), plugin, answer| {
					let own_rules = self.registered_rule_ids(ctx.session_id, plugin);
					for mut lift in std::mem::take(&mut answer.lift) {
						lift.rules.retain(|rule| own_rules.contains(rule));
						if !lift.rules.is_empty() {
							lifts.push(lift);
						}
					}
					if answer.deny.is_empty() {
						return Flow::Next;
					}
					*denies = std::mem::take(&mut answer.deny);
					Flow::Stop
				},
			)
			.await;
		ToolBeforeOutcome {
			denies: state.0,
			lifts: state.1,
			effects,
		}
	}

	/// Each plugin sees the results the ones before it replaced. The first plugin that stops wins.
	pub async fn tool_after(&self, ctx: &EventContext<'_>, payload: ToolAfter) -> ToolAfterOutcome {
		let state = ToolAfterState {
			payload,
			replaced: HashSet::new(),
			replaces: Vec::new(),
			stop: None,
		};
		let (state, effects) = self
			.run_event(
				event::TOOL_AFTER,
				ctx,
				None,
				state,
				|state, plugin| {
					let mut payload = state.payload.clone();
					if let Some(minimum) = plugin.options.min_output_tokens {
						for call in &mut payload.calls {
							if call.size_tokens < minimum && !state.replaced.contains(&call.id) {
								call.result = None;
							}
						}
					}
					for call in &mut payload.calls {
						call.returned = match (call.returned.take(), plugin.options.returned_chars) {
							(Some(returned), Some(limit)) => Some(Returned {
								text: truncate_chars(&returned.text, limit),
								..returned
							}),
							_ => None,
						};
					}
					payload
				},
				|state, _, answer| {
					for replace in std::mem::take(&mut answer.replace) {
						apply_replace(state, replace);
					}
					match answer.stop.take() {
						Some(notice) => {
							state.stop = Some(notice);
							Flow::Stop
						}
						None => Flow::Next,
					}
				},
			)
			.await;
		ToolAfterOutcome {
			replaces: state.replaces,
			stop: state.stop,
			effects,
		}
	}

	/// The first plugin that continues wins and later plugins are not asked.
	pub async fn turn_end(&self, ctx: &EventContext<'_>, payload: TurnEnd) -> TurnEndOutcome {
		let (state, effects) = self
			.run_event(
				event::TURN_END,
				ctx,
				None,
				(None::<String>, None::<bool>),
				|_, _| &payload,
				|(continue_with, verified), _, answer| {
					if let Some(complete) = answer.verified.take() {
						*verified = Some(verified.unwrap_or(true) && complete);
					}
					match answer.continue_with.take() {
						Some(text) => {
							*continue_with = Some(text);
							Flow::Stop
						}
						None => Flow::Next,
					}
				},
			)
			.await;
		TurnEndOutcome {
			continue_with: state.0,
			verified: state.1,
			effects,
		}
	}

	pub async fn compact_before(&self, ctx: &EventContext<'_>, payload: CompactBefore) -> Effects {
		let (_, effects) = self
			.run_event(
				event::COMPACT_BEFORE,
				ctx,
				None,
				(),
				|_, _| &payload,
				|_, _, _| Flow::Next,
			)
			.await;
		effects
	}

	pub async fn compact_after(&self, ctx: &EventContext<'_>, payload: CompactAfter) {
		self.run_event(
			event::COMPACT_AFTER,
			ctx,
			None,
			(),
			|_, _| &payload,
			|_, _, _| Flow::Next,
		)
		.await;
	}

	pub async fn subagent_end(&self, ctx: &EventContext<'_>, payload: SubagentEnd) {
		self.run_event(
			event::SUBAGENT_END,
			ctx,
			None,
			(),
			|_, _| &payload,
			|_, _, _| Flow::Next,
		)
		.await;
	}

	/// The notices, usage and report of the answers stay in the host until the
	/// caller settles them.
	pub async fn session_end(&self, ctx: &EventContext<'_>, payload: SessionEnd) {
		self.run_event(event::SESSION_END, ctx, None, (), |_, _| &payload, |_, _, _| Flow::Next)
			.await;
	}

	/// The first plugin that gives a judgement wins and later plugins are not
	/// asked. `None` when no plugin gave one: the caller keeps the outcome of
	/// its own rule.
	pub async fn judge(&self, ctx: &EventContext<'_>, payload: Judge) -> Option<Judged> {
		let (judged, _) = self
			.run_event(
				event::JUDGE,
				ctx,
				None,
				None::<Judged>,
				|_, _| &payload,
				|judged, plugin, answer| {
					if answer.judgements.is_empty() {
						return Flow::Next;
					}
					*judged = Some(Judged {
						plugin: plugin.to_string(),
						judgements: std::mem::take(&mut answer.judgements),
					});
					Flow::Stop
				},
			)
			.await;
		judged
	}

	/// The value each plugin hands to its subagent's `session.start`, by plugin name.
	pub async fn subagent_start(&self, ctx: &EventContext<'_>, payload: SubagentStart) -> Vec<(String, Value)> {
		let (inherited, effects) = self
			.run_event(
				event::SUBAGENT_START,
				ctx,
				None,
				Vec::<(String, Value)>::new(),
				|_, _| &payload,
				|inherited, plugin, answer| {
					if let Some(value) = answer.inherit.take() {
						inherited.push((plugin.to_string(), value));
					}
					Flow::Next
				},
			)
			.await;
		if !effects.notes.is_empty() {
			// No session inbox belongs to a subagent that has not started.
			crate::log_debug!("subagent.start answered with {} note(s); ignored", effects.notes.len());
		}
		inherited
	}

	/// Run a command a plugin provides. `None` when the plugin is off, does not
	/// take commands or did not answer.
	pub async fn command(&self, ctx: &EventContext<'_>, plugin: &str, payload: Command) -> Option<CommandOutput> {
		let (output, _) = self
			.run_event(
				event::COMMAND,
				ctx,
				Some(plugin),
				None::<CommandOutput>,
				|_, _| &payload,
				|output, _, answer| {
					*output = answer.output.take();
					Flow::Next
				},
			)
			.await;
		output
	}

	/// The rows each plugin shows in `/info`, by plugin name.
	pub async fn status(&self, ctx: &EventContext<'_>) -> Vec<(String, Vec<Row>)> {
		let request = StatusRequest::default();
		let (rows, _) = self
			.run_event(
				event::STATUS,
				ctx,
				None,
				Vec::<(String, Vec<Row>)>::new(),
				|_, _| &request,
				|rows, plugin, answer| {
					if !answer.rows.is_empty() {
						rows.push((plugin.to_string(), std::mem::take(&mut answer.rows)));
					}
					Flow::Next
				},
			)
			.await;
		rows
	}
}

/// Patch the payload so the next plugin sees the text, and keep one `Replace`
/// per call. The id may name a call of the round or a background output.
fn apply_replace(state: &mut ToolAfterState, replace: Replace) {
	if let Some(call) = state.payload.calls.iter_mut().find(|call| call.id == replace.call_id) {
		call.result = Some(replace.text.clone());
		state.replaced.insert(call.id.clone());
	} else if let Some(job) = state
		.payload
		.background
		.iter_mut()
		.find(|job| job.call_id == replace.call_id)
	{
		job.output = replace.text.clone();
	}
	match state.replaces.iter_mut().find(|known| known.call_id == replace.call_id) {
		Some(known) => {
			known.text = replace.text;
			known.keep_original |= replace.keep_original;
		}
		None => state.replaces.push(replace),
	}
}
