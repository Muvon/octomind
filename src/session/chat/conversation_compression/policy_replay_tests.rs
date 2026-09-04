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

//! Replay benchmark: the previous compression policy against the current one
//! over identical scripted session traces, driven by the real decision math.
//! No LLM — a fold is modelled as "drained range → summary at the chosen
//! ratio". Measures what each policy makes the provider process: context
//! tokens carried into every call plus fold-prompt input, and the same in
//! price-ratio units (`FoldEconomics`). The assertions pin the measured gains
//! so a regression in timing shows up as a number, not a vibe.
//!
//! Also the record of what did NOT pay: a "cold prefix cache → cache-write
//! term is free" variant was replayed here and lost on every trace (more
//! shallow mid-turn folds, higher weighted cost), so it was not shipped.

use super::*;
use crate::session::SessionInfo;

const CEILING: usize = 200_000;
const THRESHOLD: usize = 80_000;
const TOOL_CAP: usize = 6_000;
const SYSTEM_PROMPT: usize = 12_000;

#[derive(Clone, Copy)]
struct Policy {
	/// Cut oversized stored tool bodies to the cap before pricing a paid fold.
	free_tier: bool,
	/// A decision-model veto starts a cooldown instead of climbing the ladder.
	veto_cooldown: bool,
}

const PREVIOUS: Policy = Policy {
	free_tier: false,
	veto_cooldown: false,
};
const CURRENT: Policy = Policy {
	free_tier: true,
	veto_cooldown: true,
};

#[derive(Clone, Copy)]
enum Step {
	/// One API round: assistant output and a tool result of this size.
	Call { assistant: usize, tool: usize },
	/// A genuine user message: resets the ladder, records the turn's pace.
	User(usize),
}

#[derive(Clone, Copy)]
struct Msg {
	tokens: usize,
	tool: bool,
}

#[derive(Default, Debug)]
struct Metrics {
	carried_tokens: usize,
	fold_input_tokens: usize,
	paid_folds: usize,
	paid_declines: usize,
	free_trims: usize,
	/// Everything above weighted by `FoldEconomics` (uncached-input-token units).
	weighted_cost: f64,
	peak_context: usize,
}

impl Metrics {
	fn total_tokens(&self) -> usize {
		self.carried_tokens + self.fold_input_tokens
	}
}

struct Sim {
	policy: Policy,
	econ: FoldEconomics,
	messages: Vec<Msg>,
	info: SessionInfo,
	cooldown_until: usize,
	/// Scripted decision-model vetoes: fold attempt indices that decline.
	veto_attempts: Vec<usize>,
	attempts: usize,
	m: Metrics,
}

impl Sim {
	fn new(policy: Policy, econ: FoldEconomics, vetoes: &[usize]) -> Self {
		Self {
			policy,
			econ,
			messages: vec![Msg {
				tokens: SYSTEM_PROMPT,
				tool: false,
			}],
			info: SessionInfo::default(),
			cooldown_until: 0,
			veto_attempts: vetoes.to_vec(),
			attempts: 0,
			m: Metrics::default(),
		}
	}

	fn context(&self) -> usize {
		self.messages.iter().map(|m| m.tokens).sum()
	}

	fn run(mut self, trace: &[Step]) -> Metrics {
		for step in trace {
			match *step {
				Step::User(tokens) => {
					self.info.note_turn_start();
					self.info.consecutive_compressions = 0;
					self.messages.push(Msg {
						tokens,
						tool: false,
					});
				}
				Step::Call { assistant, tool } => {
					self.maybe_fold();
					let ctx = self.context();
					self.m.carried_tokens += ctx;
					self.m.weighted_cost += ctx as f64 * self.econ.cache_read;
					self.m.peak_context = self.m.peak_context.max(ctx);
					self.info.total_api_calls += 1;
					self.messages.push(Msg {
						tokens: assistant,
						tool: false,
					});
					self.messages.push(Msg {
						tokens: tool,
						tool: true,
					});
				}
			}
		}
		self.m
	}

	fn maybe_fold(&mut self) {
		let mut current = self.context();
		let growth = measured_growth_rate(&self.info, current);
		let forced = ceiling_reached(&self.info, current, CEILING);
		let runway = autonomous_runway(self.info.consecutive_compressions);
		let fire_line = adaptive_fire_line(
			THRESHOLD,
			CEILING,
			self.info.context_tokens_after_last_compression,
			growth,
			self.info.consecutive_compressions,
		);
		if !forced && current < fire_line {
			return;
		}
		if self.policy.free_tier {
			for msg in self
				.messages
				.iter_mut()
				.filter(|m| m.tool && m.tokens > TOOL_CAP)
			{
				msg.tokens = TOOL_CAP;
				self.m.free_trims += 1;
			}
			current = self.context();
			if !forced && current < fire_line {
				return;
			}
		}
		if !forced && self.info.total_api_calls < self.cooldown_until {
			return;
		}
		// Drain range: everything between the system anchor and the live exchange.
		let live = 2.min(self.messages.len() - 1);
		let end = self.messages.len() - live;
		if end <= 1 {
			return;
		}
		let compressible: usize = self.messages[1..end].iter().map(|m| m.tokens).sum();
		let ratio = if forced {
			MAX_COMPRESSION_RATIO
		} else {
			match compression_depth(current, compressible as u64, fire_line, growth, runway) {
				Some(r) => r,
				None => return,
			}
		};
		let compressible_f = compressible as f64;
		let summary = compressible_f / MAX_COMPRESSION_RATIO;
		let target_after = current as f64 - compressible_f + compressible_f / ratio;
		if !forced
			&& !fold_decision(
				&self.info,
				current as f64,
				target_after,
				compressible_f,
				summary,
				runway,
				self.econ,
			) {
			return;
		}
		// The paid call happens now, veto or not.
		let sent = compressible_f * FOLD_SENT_FRACTION;
		self.m.fold_input_tokens += sent as usize;
		self.m.weighted_cost += sent * self.econ.folder_input;
		self.attempts += 1;
		if !forced && self.veto_attempts.contains(&self.attempts) {
			self.m.paid_declines += 1;
			if self.policy.veto_cooldown {
				self.cooldown_until = self.info.total_api_calls + runway as usize;
			} else {
				self.info.consecutive_compressions += 1;
			}
			return;
		}
		self.m.weighted_cost += summary * self.econ.folder_output;
		self.m.weighted_cost += target_after * self.econ.cache_write;
		let summary_msg = Msg {
			tokens: (compressible_f / ratio) as usize,
			tool: false,
		};
		let tail: Vec<Msg> = self.messages[end..].to_vec();
		self.messages.truncate(1);
		self.messages.push(summary_msg);
		self.messages.extend(tail);
		self.info.context_tokens_after_last_compression = self.context();
		self.info.api_calls_at_last_compression = self.info.total_api_calls;
		self.info.consecutive_compressions += 1;
		self.m.paid_folds += 1;
	}
}

fn pct_less(before: f64, after: f64) -> f64 {
	(before - after) / before * 100.0
}

fn report(name: &str, prev: &Metrics, cur: &Metrics) {
	std::eprintln!(
		"[{name}] tokens {} -> {} ({:.0}% less) | weighted {:.0} -> {:.0} ({:.0}% less) | folds {}->{} declines {}->{} trims {} peak {}->{}",
		prev.total_tokens(),
		cur.total_tokens(),
		pct_less(prev.total_tokens() as f64, cur.total_tokens() as f64),
		prev.weighted_cost,
		cur.weighted_cost,
		pct_less(prev.weighted_cost, cur.weighted_cost),
		prev.paid_folds,
		cur.paid_folds,
		prev.paid_declines,
		cur.paid_declines,
		cur.free_trims,
		prev.peak_context,
		cur.peak_context,
	);
}

/// Tool loop: one user ask, then 60 autonomous rounds; every sixth tool result
/// is a 40k body that entered the context past the ingest cap (pre-cap
/// session, bypass path). The previous policy could only pay a fold for it.
fn oversized_tool_trace() -> Vec<Step> {
	let mut t = vec![Step::User(400)];
	for i in 1..=60 {
		t.push(Step::Call {
			assistant: 800,
			tool: if i % 6 == 0 { 40_000 } else { 3_000 },
		});
	}
	t
}

/// One long autonomous turn whose first fold attempt the decision model vetoes.
fn veto_trace() -> Vec<Step> {
	let mut t = vec![Step::User(400)];
	for _ in 0..45 {
		t.push(Step::Call {
			assistant: 1_000,
			tool: 3_500,
		});
	}
	t
}

#[test]
fn free_tier_avoids_paid_folds_for_oversized_tool_bodies() {
	let trace = oversized_tool_trace();
	let prev = Sim::new(PREVIOUS, FoldEconomics::DEFAULT, &[]).run(&trace);
	let cur = Sim::new(CURRENT, FoldEconomics::DEFAULT, &[]).run(&trace);
	report("oversized-tool", &prev, &cur);
	// Measured: 13% fewer tokens processed, 31% lower weighted cost, one paid
	// fold fewer. Pinned with a margin so a timing regression shows as a number.
	assert!(cur.free_trims > 0);
	assert!(cur.paid_folds < prev.paid_folds);
	assert!(pct_less(prev.total_tokens() as f64, cur.total_tokens() as f64) >= 10.0);
	assert!(pct_less(prev.weighted_cost, cur.weighted_cost) >= 25.0);
}

#[test]
fn veto_cooldown_keeps_the_fire_line_instead_of_doubling_it() {
	let trace = veto_trace();
	let prev = Sim::new(PREVIOUS, FoldEconomics::DEFAULT, &[1]).run(&trace);
	let cur = Sim::new(CURRENT, FoldEconomics::DEFAULT, &[1]).run(&trace);
	report("veto", &prev, &cur);
	// Measured: 13% fewer tokens, 17% lower weighted cost, peak 156k -> 134k.
	assert_eq!(prev.paid_declines, 1);
	assert_eq!(cur.paid_declines, 1);
	assert!(cur.peak_context < prev.peak_context);
	assert!(pct_less(prev.total_tokens() as f64, cur.total_tokens() as f64) >= 10.0);
}
