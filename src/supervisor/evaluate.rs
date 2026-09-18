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
//! Jev through octolib's `evaluation` module) at five supervisor seams —
//! a relevance filter after recall ranking, a roster choice when skill rules
//! abstain, a pre-screen that decides whether the authorizer needs to wake
//! the supervisor model at all, chunk scoring of oversized tool results in
//! place of the supervisor-model condenser, and pre-fold demotion of dead
//! tool packets in the PACT evidence set.
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
//! answers can only drop or demote text, never author it.

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
	pub authorizer: bool,
	pub condense: bool,
	pub compression: bool,
}

/// The unit of switching, stats, and fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seam {
	Recall,
	Skills,
	Authorizer,
	Condense,
	Compression,
}

pub const SEAM_COUNT: usize = 5;

impl Seam {
	pub const ALL: [Seam; SEAM_COUNT] = [
		Seam::Recall,
		Seam::Skills,
		Seam::Authorizer,
		Seam::Condense,
		Seam::Compression,
	];

	pub fn name(self) -> &'static str {
		match self {
			Seam::Recall => "recall",
			Seam::Skills => "skills",
			Seam::Authorizer => "authorizer",
			Seam::Condense => "condense",
			Seam::Compression => "compression",
		}
	}

	pub fn index(self) -> usize {
		match self {
			Seam::Recall => 0,
			Seam::Skills => 1,
			Seam::Authorizer => 2,
			Seam::Condense => 3,
			Seam::Compression => 4,
		}
	}

	fn switched_on(self, config: &EvaluateConfig) -> bool {
		match self {
			Seam::Recall => config.recall,
			Seam::Skills => config.skills,
			Seam::Authorizer => config.authorizer,
			Seam::Condense => config.condense,
			Seam::Compression => config.compression,
		}
	}
}

// Thresholds are constants, not config keys — same reasoning as the detector
// thresholds: good defaults, and questions and thresholds are reviewed
// together in this one file.

/// Recall: a scoped candidate whose "bears on the request" probability is
/// below this is excluded from pack admission.
pub const RECALL_KEEP_AT: f64 = 0.5;
/// Skills: the chosen skill auto-activates only at or above this confidence.
pub const SKILL_CONFIDENCE_FLOOR: f64 = 0.8;
/// Authorizer: any Noul at or above this flags the batch for the supervisor.
pub const AUTHORIZER_FLAG_AT: f64 = 0.5;
/// Condense: a chunk at or above this is kept, together with its neighbours.
pub const CONDENSE_KEEP_AT: f64 = 0.5;
/// Condense: line-aligned chunk size of a candidate's original text. A single
/// line above this forms its own chunk.
pub const CONDENSE_CHUNK_TOKENS: usize = 256;
/// Compression: a summarize-lane tool packet below this is demoted to a
/// recall pointer before the fold model reads the evidence set.
pub const COMPRESSION_KEEP_AT: f64 = 0.5;
/// Compression: head-and-tail sample of a packet's rendered content.
pub const COMPRESSION_SAMPLE_TOKENS: usize = 512;
/// Below the model's 32k state limit with headroom for question text.
pub const MAX_STATE_TOKENS: usize = 24_000;
/// Questions per window. The provider's per-call limit is undocumented; this
/// stays well under the 255-option bound a Choice carries.
pub const MAX_WINDOW_QUESTIONS: usize = 96;
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

const RECALL_QUESTION: &str = "Does this lesson bear on the current request? Yes only when applying the lesson would change how the request is carried out; no when it concerns a different tool, language, file, or task.";
const CONDENSE_QUESTION: &str = "Does the agent need this chunk of the tool output to advance the current task? Yes for error messages and stack traces, the data the tool call's arguments were querying for, explicit negative results, counts, totals and exit codes, and the paths, line numbers or signatures the task points at; no for boilerplate, progress noise, decorative separators, unrelated matches, and stretches the task never touches.";
const COMPRESSION_QUESTION: &str = "Does the work that continues after this fold still need the content of this tool interaction? Yes when it holds an unresolved error, a user-facing correction, or facts the pinned task, constraints or plan still depend on; no when it is a completed step whose outcome is already established or a lookup the task has moved past.";
const SKILL_QUESTION: &str = "Which skill applies to the request? Choose the one whose description matches what the request asks for; choose none when no listed skill fits.";
const SKILL_NONE_DESCRIPTION: &str = "No skill in this list applies to the request";
/// Authorizer Nouls, keyed by the suffix appended to each pending call id.
pub const AUTHORIZER_NOULS: [(&str, &str); 3] = [
	(
		"prohibited",
		"The call conflicts with an explicit prohibition in the user or role instructions.",
	),
	(
		"destructive",
		"The call deletes or overwrites files or git history that are not regenerable from a build.",
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

/// One Noul per scoped candidate, keyed by its index in the state.
pub fn recall_questions(count: usize) -> BTreeMap<String, Question> {
	(0..count)
		.map(|index| (recall_question_id(index), Question::noul(RECALL_QUESTION)))
		.collect()
}

/// One Choice over every inactive pool entry plus `none`.
pub fn skill_question<'a>(roster: impl IntoIterator<Item = (&'a str, &'a str)>) -> Question {
	Question::choice(
		SKILL_QUESTION,
		roster
			.into_iter()
			.chain(std::iter::once((SKILL_NONE, SKILL_NONE_DESCRIPTION))),
	)
}

pub fn authorizer_question_id(call_id: &str, noul: &str) -> String {
	format!("{call_id}.{noul}")
}

/// Three Nouls per pending call.
pub fn authorizer_questions(call_ids: &[String]) -> BTreeMap<String, Question> {
	call_ids
		.iter()
		.flat_map(|id| {
			AUTHORIZER_NOULS.iter().map(move |(noul, instructions)| {
				(
					authorizer_question_id(id, noul),
					Question::noul(*instructions),
				)
			})
		})
		.collect()
}

pub fn condense_question_id(chunk: usize) -> String {
	format!("k{chunk}")
}

/// One Noul per chunk of one window, keyed by the chunk's slot in that window.
pub fn condense_questions(count: usize) -> BTreeMap<String, Question> {
	(0..count)
		.map(|slot| {
			(
				condense_question_id(slot),
				Question::noul(CONDENSE_QUESTION),
			)
		})
		.collect()
}

pub fn compression_question_id(unit: usize) -> String {
	format!("u{unit}")
}

/// One Noul per scoring unit of one window, keyed by the unit's slot.
pub fn compression_questions(count: usize) -> BTreeMap<String, Question> {
	(0..count)
		.map(|slot| {
			(
				compression_question_id(slot),
				Question::noul(COMPRESSION_QUESTION),
			)
		})
		.collect()
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

/// Evaluate the windows of one round concurrently, all or nothing: one failed
/// window discards every answer, counts one fallback for the round, and the
/// caller keeps its pre-change behavior. Answers come back in window order.
/// An empty round makes no call and yields no answers.
pub async fn run_windows(
	config: &super::SupervisorConfig,
	seam: Seam,
	windows: Vec<(serde_json::Value, BTreeMap<String, Question>)>,
) -> Option<Vec<BTreeMap<String, Answer>>> {
	if !enabled(config, seam) {
		return None;
	}
	let results = futures::future::join_all(
		windows
			.into_iter()
			.map(|(state, questions)| attempt(config, seam, state, questions)),
	)
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
