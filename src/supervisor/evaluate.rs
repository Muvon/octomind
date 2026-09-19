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

//! Evaluation gates: calibrated answers from an evaluation model (TypeSafe
//! Jev through octolib's `evaluation` module) at eight supervisor seams —
//! a relevance filter after recall ranking, a roster choice when skill rules
//! abstain, a pre-screen that decides whether the authorizer needs to wake
//! the supervisor model at all, chunk scoring of oversized tool results in
//! place of the supervisor-model condenser, pre-fold demotion of dead tool
//! packets in the PACT evidence set, grounding of extracted lessons in place
//! of the chat verifier, a pre-screen before the external planner, and
//! per-finding refutation of a blocking verify-gate verdict.
//!
//! This is the only module that builds evaluation requests. It owns every
//! question text and threshold, applies the master and per-seam switches,
//! bounds each call to one attempt under a short timeout, caps the state
//! size, and attributes usage. Callers never see a provider error: they get
//! answers or `None`, and `None` always means "keep the pre-change result".
//!
//! Security rule for the recall, skills, and authorizer seams: the state
//! never carries tool results, assistant messages, or condensed-output
//! notices — the reviewer must never read the thing that might be arguing.
//! The condense and compression seams judge tool output by design; their
//! states carry that output and nothing else from the transcript, and their
//! answers can only drop or demote text, never author it. The distill, plan,
//! and gate seams receive exactly the payload the chat model they replace
//! receives, and may only reject a lesson, skip a planner call, or drop a
//! finding — never store, advance, create, or pass.

use octolib::evaluation::{
	Answer, EvaluationRequest, EvaluationResponse, EvaluationResult, Question,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// `[supervisor.evaluate]`. The model is a plain `provider:model` string, not
/// a profile: Jev takes state and questions only and accepts no sampling keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluateConfig {
	pub model: String,
	pub recall: bool,
	pub skills: bool,
	pub capabilities: bool,
	pub authorizer: bool,
	pub condense: bool,
	pub compression: bool,
	pub distill: bool,
	pub plan: bool,
	pub gate: bool,
}

/// The unit of switching, stats, and fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seam {
	Recall,
	Skills,
	Authorizer,
	Condense,
	Compression,
	Distill,
	Plan,
	Gate,
	Capabilities,
}

pub const SEAM_COUNT: usize = 9;

impl Seam {
	pub const ALL: [Seam; SEAM_COUNT] = [
		Seam::Recall,
		Seam::Skills,
		Seam::Authorizer,
		Seam::Condense,
		Seam::Compression,
		Seam::Distill,
		Seam::Plan,
		Seam::Gate,
		Seam::Capabilities,
	];

	pub fn name(self) -> &'static str {
		match self {
			Seam::Recall => "recall",
			Seam::Skills => "skills",
			Seam::Authorizer => "authorizer",
			Seam::Condense => "condense",
			Seam::Compression => "compression",
			Seam::Distill => "distill",
			Seam::Plan => "plan",
			Seam::Gate => "gate",
			Seam::Capabilities => "capabilities",
		}
	}

	pub fn index(self) -> usize {
		match self {
			Seam::Recall => 0,
			Seam::Skills => 1,
			Seam::Authorizer => 2,
			Seam::Condense => 3,
			Seam::Compression => 4,
			Seam::Distill => 5,
			Seam::Plan => 6,
			Seam::Gate => 7,
			Seam::Capabilities => 8,
		}
	}

	fn switched_on(self, config: &EvaluateConfig) -> bool {
		match self {
			Seam::Recall => config.recall,
			Seam::Skills => config.skills,
			Seam::Authorizer => config.authorizer,
			Seam::Condense => config.condense,
			Seam::Compression => config.compression,
			Seam::Distill => config.distill,
			Seam::Plan => config.plan,
			Seam::Gate => config.gate,
			Seam::Capabilities => config.capabilities,
		}
	}
}

// Thresholds are constants, not config keys — same reasoning as the detector
// thresholds: good defaults, and questions and thresholds are reviewed
// together in this one file.

/// Recall: a scoped candidate whose "bears on the request" probability is
/// below this is excluded from pack admission.
pub const RECALL_KEEP_AT: f64 = 0.5;
/// Skills: the chosen skill auto-activates only when its own probability in
/// the Choice distribution is at or above this. The distribution sums to 1
/// over the roster plus `none`, so it is the same kind of number every other
/// seam thresholds; the answer's derived `confidence` is not. A roster of a
/// hundred skills spreads the mass: on the tap's eval set the right skill is
/// the top choice 92% of the time, but 0.8 abstained on 13% of those, 0.6 on
/// 5%, with one more chitchat false activation in thirty.
pub const SKILL_ACTIVATE_AT: f64 = 0.6;
/// Authorizer: any Noul at or above this flags the batch for the supervisor.
pub const AUTHORIZER_FLAG_AT: f64 = 0.5;
/// Condense: a chunk at or above this is kept, together with its neighbours.
pub const CONDENSE_KEEP_AT: f64 = 0.5;
/// Condense: line-aligned chunk size of a candidate's original text. A single
/// line above this forms its own chunk.
pub const CONDENSE_CHUNK_TOKENS: usize = 256;
/// Condense: head-and-tail sample of a chunk that is one oversized line, so
/// no single line can push a window over the state cap.
pub const CONDENSE_CHUNK_SAMPLE_TOKENS: usize = 512;
/// Compression: a summarize-lane tool packet below this is demoted to a
/// recall pointer before the fold model reads the evidence set.
pub const COMPRESSION_KEEP_AT: f64 = 0.5;
/// Compression: head-and-tail sample of a packet's rendered content.
pub const COMPRESSION_SAMPLE_TOKENS: usize = 512;
/// Distill: a candidate lesson at or above this is kept — the same yes/no the
/// chat verifier answers on the same evidence.
pub const DISTILL_KEEP_AT: f64 = 0.5;
/// Plan: a `request` or `phase_complete` signal below this is consumed
/// without a planner call. Low on purpose: the pre-screen may only remove a
/// call when the evaluation is confident nothing is needed, since the planner
/// already leans toward `no_plan` and `hold`.
pub const PLAN_SKIP_BELOW: f64 = 0.2;
/// Gate: a charged finding at or above this is refuted. High on purpose:
/// "doubt is not refutation" is the pre-change rule.
pub const GATE_REFUTE_AT: f64 = 0.8;
/// Capabilities: a cosine winner stays activated only when the Choice gives it
/// at least this. Near zero on purpose: cosine already cleared its threshold
/// and margin, so only a flat rejection says the phrasing fooled it. On the
/// tap's eval set this removes most false activations on chitchat at a cost of
/// one point of recall.
pub const CAPABILITY_CONFIRM_AT: f64 = 0.05;
/// Capabilities: with no cosine winner, the chosen capability activates only
/// when its own probability in the Choice is at or above this. Lower than the
/// skill floor because the roster is short: over five options plus `none` the
/// mass is not spread thin, and 0.8 abstained on half of the recoverable
/// margin-abstains.
pub const CAPABILITY_ACTIVATE_AT: f64 = 0.6;
/// Capabilities: the Choice lists this many best-scored inactive capabilities
/// plus `none`. The cosine top-5 holds the right capability 97% of the time on
/// the tap's eval set; listing the whole roster spreads the distribution and
/// costs nine times the tokens.
pub const CAPABILITY_TOP_K: usize = 5;
/// Below the model's 32k state limit with headroom for question text.
pub const MAX_STATE_TOKENS: usize = 24_000;
/// Questions per window. The provider's per-call limit is undocumented; this
/// stays well under the 255-option bound a Choice carries.
pub const MAX_WINDOW_QUESTIONS: usize = 96;
/// Windows of one round in flight at once. Bounded so a large round cannot
/// fan out into a burst the provider rate-limits, which with no retries
/// would fail the whole round.
const MAX_WINDOWS_IN_FLIGHT: usize = 4;
/// Windows are packed from per-item estimates; the slack keeps the serialized
/// state under the runner's cap even when JSON framing adds a little.
const WINDOW_SLACK_TOKENS: usize = 256;
/// Per-candidate content budget inside the recall state.
pub const RECALL_CANDIDATE_TOKENS: usize = 320;
/// A Choice needs room for `none`; the API caps options at 255.
pub const MAX_SKILL_ROSTER: usize = 254;
/// One attempt only: the runner sits on the agent's critical path per turn,
/// so a provider outage costs at most one timeout per seam per turn.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Choice option meaning "no skill applies".
pub const SKILL_NONE: &str = "none";
/// Trigger label reported when the evaluation choice activates a skill.
pub const SKILL_TRIGGER: &str = "evaluate";
/// Question id of the single skill Choice.
pub const SKILL_QUESTION_ID: &str = "skill";
/// Choice option meaning "no capability applies"; the same key as the skills'.
pub const CAPABILITY_NONE: &str = SKILL_NONE;
/// Question id of the single capability Choice.
pub const CAPABILITY_QUESTION_ID: &str = "capability";

const RECALL_QUESTION: &str = "Does this lesson bear on the current request? Yes when the lesson states a convention, constraint, workflow step, or preference the work on this request must follow, or a fact it must take into account; no when it concerns a different tool, language, file, or task and following it would change nothing about this request.";
const CONDENSE_QUESTION: &str = "Must the agent read this chunk to finish the current task? Yes only when the chunk holds an error message or stack trace, the specific data the task or the tool arguments ask for, an explicit negative result, a count, total or exit code, or a path, line number or signature the agent must act on. No when the chunk is more of a listing that other chunks already answer, boilerplate, progress noise, separators, or content the task never touches; dropped chunks stay readable in a file.";
const COMPRESSION_QUESTION: &str = "Does the work that continues after this fold still need the content of this tool interaction? Yes when it holds an unresolved error, a user-facing correction, or facts the pinned task, constraints or plan still depend on; no when it is a completed step whose outcome is already established or a lookup the task has moved past.";
const DISTILL_QUESTION: &str = "Does this lesson's rule follow from its cited evidence quote as that quote appears in the transcript? Yes only when the rule states what the quote says without generalizing beyond it, adding requirements the quote does not state, or inventing scope the quote does not establish; no when the quote is absent from the transcript, misread, or overreached.";
const PLAN_REQUEST_QUESTION: &str = "Does the work remaining on the current request need an external plan? Yes only when the remaining work has at least three meaningful dependent phases, material context-loss risk, or a real branch that must be tracked; no for an answer, a review with one deliverable, a focused fix, or a routine read, change and check sequence, and no when the runtime evidence shows the work is already mostly done.";
const PLAN_PHASE_QUESTION: &str = "Do the runtime-recorded actions or tool observations show that the active phase's done_when condition is met? Yes only when a recorded action or tool output evidences the stated outcome; no when only the assistant's narration claims it or the evidence shows unrelated or unfinished work.";
const GATE_QUESTION: &str = "Is this finding refuted by a citable observation in the evidence: a recorded action that performed the check the finding calls missing, a diff hunk containing the change it calls absent, a successful recorded check whose output exercised the condition it calls violated, or request text showing the demand was never made? No when the evidence merely makes the finding doubtful; doubt is not refutation.";
const SKILL_QUESTION: &str = "Which skill applies to the request? Choose the one whose description matches what the request asks for; choose none when no listed skill fits.";
const SKILL_NONE_DESCRIPTION: &str = "No skill in this list applies to the request";
const CAPABILITY_QUESTION: &str = "Which capability applies to the request? Choose the one whose description matches what the request asks for; choose none when no listed capability fits.";
const CAPABILITY_NONE_DESCRIPTION: &str = "No capability in this list applies to the request";
/// Authorizer Nouls, keyed by the suffix appended to each pending call id.
pub const AUTHORIZER_NOULS: [(&str, &str); 3] = [
	(
		"prohibited",
		"The call conflicts with an explicit prohibition in the user or role instructions.",
	),
	(
		"destructive",
		"The call deletes, overwrites, or discards files, uncommitted changes, data, or git history that are not regenerable from a build.",
	),
	(
		"external",
		"The call sends data to, or modifies, something outside the working directory.",
	),
];

/// Master switch and seam switch together: false means no call, ever.
pub fn enabled(config: &super::SupervisorConfig, seam: Seam) -> bool {
	config.enabled && seam.switched_on(&config.evaluate)
}

pub fn recall_question_id(index: usize) -> String {
	format!("c{index}")
}

/// One Noul per scoped candidate, keyed by its index in the state. Each
/// question names its candidate by index so the answer is tied to that
/// candidate's text, not to an opaque id.
pub fn recall_questions(count: usize) -> BTreeMap<String, Question> {
	(0..count)
		.map(|index| {
			(
				recall_question_id(index),
				Question::noul(format!("Candidate {index}: {RECALL_QUESTION}")),
			)
		})
		.collect()
}

/// One Choice over every inactive pool entry plus `none`.
pub fn skill_question<'a>(roster: impl IntoIterator<Item = (&'a str, &'a str)>) -> Question {
	roster_question(SKILL_QUESTION, SKILL_NONE_DESCRIPTION, roster)
}

/// One Choice over the best-scored inactive capabilities plus `none`.
pub fn capability_question<'a>(roster: impl IntoIterator<Item = (&'a str, &'a str)>) -> Question {
	roster_question(CAPABILITY_QUESTION, CAPABILITY_NONE_DESCRIPTION, roster)
}

fn roster_question<'a>(
	instructions: &str,
	none_description: &'a str,
	roster: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Question {
	Question::choice(
		instructions,
		roster
			.into_iter()
			.chain(std::iter::once((SKILL_NONE, none_description))),
	)
}

pub fn authorizer_question_id(call_id: &str, noul: &str) -> String {
	format!("{call_id}.{noul}")
}

/// Three Nouls per pending call, each naming the call it judges by id and
/// tool so a batch of several calls is not scored as one.
pub fn authorizer_questions<'a>(
	calls: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> BTreeMap<String, Question> {
	calls
		.into_iter()
		.flat_map(|(id, tool)| {
			AUTHORIZER_NOULS.iter().map(move |(noul, instructions)| {
				(
					authorizer_question_id(id, noul),
					Question::noul(format!("Call {id} ({tool}): {instructions}")),
				)
			})
		})
		.collect()
}

pub fn condense_question_id(chunk: usize) -> String {
	format!("k{chunk}")
}

/// One Noul per chunk of one window, keyed by the chunk's slot in that
/// window. Each question names its chunk by index and line range so the
/// answer is tied to that chunk's text, not to an opaque id.
pub fn condense_questions(
	chunks: impl IntoIterator<Item = (usize, usize, usize)>,
) -> BTreeMap<String, Question> {
	chunks
		.into_iter()
		.enumerate()
		.map(|(slot, (index, first_line, last_line))| {
			(
				condense_question_id(slot),
				Question::noul(format!(
					"Chunk {index} (lines {first_line}-{last_line}): {CONDENSE_QUESTION}"
				)),
			)
		})
		.collect()
}

pub fn compression_question_id(unit: usize) -> String {
	format!("u{unit}")
}

/// One Noul per scoring unit of one window, keyed by the unit's slot. Each
/// question names its packet id so the answer is tied to that unit.
pub fn compression_questions<'a>(
	ids: impl IntoIterator<Item = &'a str>,
) -> BTreeMap<String, Question> {
	ids.into_iter()
		.enumerate()
		.map(|(slot, id)| {
			(
				compression_question_id(slot),
				Question::noul(format!("Packet {id}: {COMPRESSION_QUESTION}")),
			)
		})
		.collect()
}

pub fn distill_question_id(lesson: usize) -> String {
	format!("l{lesson}")
}

/// One Noul per candidate lesson, keyed by slot and named by its 1-based
/// number in the state.
pub fn distill_questions(count: usize) -> BTreeMap<String, Question> {
	(0..count)
		.map(|slot| {
			(
				distill_question_id(slot),
				Question::noul(format!("Lesson {}: {DISTILL_QUESTION}", slot + 1)),
			)
		})
		.collect()
}

/// Question id of the single plan pre-screen Noul.
pub const PLAN_QUESTION_ID: &str = "plan";

/// The `request` pre-screen: does the remaining work need a plan at all?
pub fn plan_request_question() -> BTreeMap<String, Question> {
	BTreeMap::from([(
		PLAN_QUESTION_ID.to_string(),
		Question::noul(PLAN_REQUEST_QUESTION),
	)])
}

/// The `phase_complete` pre-screen names the phase outcome it judges.
pub fn plan_phase_question(done_when: &str) -> BTreeMap<String, Question> {
	BTreeMap::from([(
		PLAN_QUESTION_ID.to_string(),
		Question::noul(format!(
			"{PLAN_PHASE_QUESTION} The phase's done_when: {done_when}"
		)),
	)])
}

pub fn gate_question_id(finding: usize) -> String {
	format!("f{finding}")
}

/// One Noul per charged finding, keyed by slot and named by its 1-based
/// number in the state.
pub fn gate_questions(count: usize) -> BTreeMap<String, Question> {
	(0..count)
		.map(|slot| {
			(
				gate_question_id(slot),
				Question::noul(format!("Finding {}: {GATE_QUESTION}", slot + 1)),
			)
		})
		.collect()
}

/// The Noul probability answered under `id`. The runner only returns answer
/// sets that carry a Noul for every Noul asked, so a caller reading its own
/// question ids always gets a number.
pub fn probability(answers: &BTreeMap<String, Answer>, id: &str) -> f64 {
	match answers.get(id) {
		Some(Answer::Noul { noul }) => *noul,
		_ => 0.0,
	}
}

/// Split `items` into consecutive windows whose state stays under the runner's
/// cap and whose question count stays under the per-call bound. `header_tokens`
/// is the cost of the state with no item in it. An item larger than a whole
/// window gets one of its own and fails the state cap at call time.
pub fn windows<T>(
	items: Vec<T>,
	header_tokens: usize,
	item_tokens: impl Fn(&T) -> usize,
) -> Vec<Vec<T>> {
	let budget = MAX_STATE_TOKENS.saturating_sub(WINDOW_SLACK_TOKENS + header_tokens);
	let mut out: Vec<Vec<T>> = Vec::new();
	let mut current: Vec<T> = Vec::new();
	let mut used = 0usize;
	for item in items {
		let cost = item_tokens(&item);
		if !current.is_empty() && (used + cost > budget || current.len() >= MAX_WINDOW_QUESTIONS) {
			out.push(std::mem::take(&mut current));
			used = 0;
		}
		used += cost;
		current.push(item);
	}
	if !current.is_empty() {
		out.push(current);
	}
	out
}

/// The seam falls through to its pre-change result: one debug line and one
/// counter, whether a call was attempted or skipped before it could be.
pub fn unavailable(seam: Seam, reason: &str) {
	crate::log_debug!("evaluate {} unavailable: {}", seam.name(), reason);
	super::stats::evaluate_unavailable(seam);
}

/// One chat call this seam stood in for, measured by the payload that call
/// would have carried: `input_tokens` estimated from it, priced at the
/// supervisor model's reference input rate when the table knows the model.
/// Output tokens and wall time are not counted, so the figure is a floor.
pub fn avoided(config: &crate::config::Config, seam: Seam, input_tokens: usize) {
	let model = config.get_supervisor_model_profile().model;
	let name = model
		.split_once(':')
		.map_or(model.as_str(), |(_, name)| name);
	let cost =
		octolib::llm::reference_pricing::calculate_reference_cost(name, input_tokens as u64, 0, 0)
			.unwrap_or(0.0);
	super::stats::evaluate_avoided(seam, input_tokens as u64, cost);
}

/// Evaluate `questions` over `state` for `seam`. `None` means the caller keeps
/// its pre-change behavior: the seam is off, the state is too large, the
/// provider failed or timed out, or the answers did not match the questions.
pub async fn run(
	config: &super::SupervisorConfig,
	seam: Seam,
	state: serde_json::Value,
	questions: BTreeMap<String, Question>,
) -> Option<BTreeMap<String, Answer>> {
	if !enabled(config, seam) {
		return None;
	}
	match attempt(config, seam, state, questions).await {
		Ok(answers) => Some(answers),
		Err(reason) => {
			unavailable(seam, &reason);
			None
		}
	}
}

/// Evaluate the windows of one round concurrently (at most
/// `MAX_WINDOWS_IN_FLIGHT` at a time), all or nothing: one failed window
/// discards every answer, counts one fallback for the round, and the caller
/// keeps its pre-change behavior. Answers come back in window order. An empty
/// round makes no call and yields no answers.
pub async fn run_windows(
	config: &super::SupervisorConfig,
	seam: Seam,
	windows: Vec<(serde_json::Value, BTreeMap<String, Question>)>,
) -> Option<Vec<BTreeMap<String, Answer>>> {
	if !enabled(config, seam) {
		return None;
	}
	use futures::StreamExt;
	let results: Vec<_> = futures::stream::iter(
		windows
			.into_iter()
			.map(|(state, questions)| attempt(config, seam, state, questions)),
	)
	.buffered(MAX_WINDOWS_IN_FLIGHT)
	.collect()
	.await;
	let mut answers = Vec::with_capacity(results.len());
	for result in results {
		match result {
			Ok(window) => answers.push(window),
			Err(reason) => {
				unavailable(seam, &reason);
				return None;
			}
		}
	}
	Some(answers)
}

/// One bounded attempt. `Err` carries the fallback reason and leaves the
/// counting to the caller so a multi-window round counts one fallback.
async fn attempt(
	config: &super::SupervisorConfig,
	seam: Seam,
	state: serde_json::Value,
	questions: BTreeMap<String, Question>,
) -> Result<BTreeMap<String, Answer>, String> {
	if crate::session::estimate_tokens(&state.to_string()) > MAX_STATE_TOKENS {
		return Err("state too large".to_string());
	}
	let mut request = EvaluationRequest::new(state);
	request.questions = questions.clone();
	request.max_retries = 0;
	request.timeout = TIMEOUT;

	super::stats::evaluate_call(seam);
	let started = std::time::Instant::now();
	let response = match tokio::time::timeout(TIMEOUT, call(&config.evaluate.model, request)).await
	{
		Ok(Ok(response)) => response,
		Ok(Err(error)) => return Err(error.to_string()),
		Err(_) => return Err("timeout".to_string()),
	};
	super::stats::record_call(
		super::stats::CallKind::Evaluate,
		response.usage.input_tokens,
		response.usage.output_tokens,
		0,
		started.elapsed().as_millis() as u64,
		response.usage.cost.unwrap_or(0.0),
	);
	// Every question must come back under its own id with the matching answer
	// type; anything else is an unparseable response for the whole seam.
	let well_formed = questions.iter().all(|(id, question)| {
		matches!(
			(question, response.answers.get(id)),
			(Question::Noul { .. }, Some(Answer::Noul { .. }))
				| (Question::Choice { .. }, Some(Answer::Choice { .. }))
				| (Question::Score { .. }, Some(Answer::Score { .. }))
		)
	});
	if !well_formed {
		return Err("invalid response".to_string());
	}
	Ok(response.answers)
}

async fn call(model: &str, request: EvaluationRequest) -> EvaluationResult<EvaluationResponse> {
	#[cfg(test)]
	if let Some(fake) = fake_provider() {
		let (_, model) = octolib::evaluation::EvaluationProviderFactory::parse_model(model)?;
		return fake.evaluate(request.with_model(model)).await;
	}
	octolib::evaluation::evaluate(model, request).await
}

// In-process provider override so unit tests never touch the network. Tests
// that install one must serialize (it is process-global, like the stats sink).
#[cfg(test)]
type FakeProvider = std::sync::Arc<dyn octolib::evaluation::EvaluationProvider>;
#[cfg(test)]
static FAKE_PROVIDER: std::sync::RwLock<Option<FakeProvider>> = std::sync::RwLock::new(None);

#[cfg(test)]
pub(crate) fn set_fake_provider(provider: Option<FakeProvider>) {
	*FAKE_PROVIDER.write().unwrap() = provider;
}

#[cfg(test)]
fn fake_provider() -> Option<FakeProvider> {
	FAKE_PROVIDER.read().unwrap().clone()
}

#[cfg(test)]
#[path = "evaluate_tests.rs"]
mod tests;
