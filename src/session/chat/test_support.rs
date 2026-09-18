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

//! Shared fake-provider test harness: a scripted OpenAI-compatible HTTP stub
//! plus session/config builders wired to octolib's ollama provider via
//! `OLLAMA_API_URL`. Used by every in-crate e2e-style test that drives a
//! real LLM round trip (api executor, conversation compression, …).

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `OLLAMA_API_URL` is process-global env — tests touching it must not
/// overlap, across ALL test modules in this binary. An async mutex because
/// the guard is deliberately held across the awaited LLM round trip, and it
/// cannot poison — a failed test must not cascade.
pub(crate) static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Spawn a one-shot-per-connection HTTP stub returning scripted
/// chat-completion bodies in order. Returns the chat-completions URL.
pub(crate) async fn spawn_stub(responses: Vec<serde_json::Value>) -> String {
	spawn_stub_with_status(responses.into_iter().map(|r| (200, r)).collect()).await
}

/// Like [`spawn_stub`] but each scripted entry carries its HTTP status,
/// so provider-level error handling can be exercised.
pub(crate) async fn spawn_stub_with_status(responses: Vec<(u16, serde_json::Value)>) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind stub listener");
	let addr = listener.local_addr().expect("stub addr");
	let queue = std::sync::Arc::new(StdMutex::new(VecDeque::from(responses)));

	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			let queue = queue.clone();
			tokio::spawn(async move {
				// Read headers + Content-Length body of the POST request.
				let mut buf = Vec::new();
				let mut tmp = [0u8; 8192];
				let header_end = loop {
					let n = sock.read(&mut tmp).await.unwrap_or(0);
					if n == 0 {
						return;
					}
					buf.extend_from_slice(&tmp[..n]);
					if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
						break pos + 4;
					}
				};
				let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
				let content_length: usize = headers
					.lines()
					.find_map(|l| l.strip_prefix("content-length:"))
					.and_then(|v| v.trim().parse().ok())
					.unwrap_or(0);
				while buf.len() < header_end + content_length {
					let n = sock.read(&mut tmp).await.unwrap_or(0);
					if n == 0 {
						break;
					}
					buf.extend_from_slice(&tmp[..n]);
				}

				let (status, body) = queue
					.lock()
					.expect("stub queue")
					.pop_front()
					.unwrap_or_else(|| {
						(
							200,
							serde_json::json!({
								"choices": [{
									"message": {"role": "assistant", "content": "SCRIPT EXHAUSTED"},
									"finish_reason": "stop"
								}],
								"usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
							}),
						)
					});
				let body = body.to_string();
				let reason = if status == 200 { "OK" } else { "Error" };
				let response = format!(
					"HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
					body.len(),
					body
				);
				let _ = sock.write_all(response.as_bytes()).await;
				let _ = sock.shutdown().await;
			});
		}
	});

	format!("http://{}/v1/chat/completions", addr)
}

pub(crate) fn final_response(text: &str) -> serde_json::Value {
	serde_json::json!({
		"choices": [{
			"message": {"role": "assistant", "content": text},
			"finish_reason": "stop"
		}],
		"usage": {"prompt_tokens": 20, "completion_tokens": 10, "total_tokens": 30, "cost": 0.001}
	})
}

pub(crate) fn tool_calls_response(calls: &[(&str, &str, serde_json::Value)]) -> serde_json::Value {
	let tool_calls: Vec<serde_json::Value> = calls
		.iter()
		.map(|(id, name, arguments)| {
			serde_json::json!({
				"id": id,
				"type": "function",
				"function": {"name": name, "arguments": arguments.to_string()}
			})
		})
		.collect();
	serde_json::json!({
		"choices": [{
			"message": {"role": "assistant", "content": "", "tool_calls": tool_calls},
			"finish_reason": "tool_calls"
		}],
		"usage": {"prompt_tokens": 25, "completion_tokens": 15, "total_tokens": 40, "cost": 0.002}
	})
}

pub(crate) fn tool_call_response(
	tool_name: &str,
	arguments: serde_json::Value,
) -> serde_json::Value {
	tool_calls_response(&[("call_1", tool_name, arguments)])
}

/// Merged config wired for the fake provider: real template + assistant
/// role, supervisor off (its gates would issue their own scripted-queue
/// desyncing LLM calls).
pub(crate) fn fake_provider_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config.model = "ollama:fake-model".to_string();
	config.supervisor.enabled = false;
	// Scripted queues describe exact call sequences. Tests that exercise retry
	// behavior opt back in explicitly instead of consuming an unrelated entry.
	config.max_retries = 0;
	let mut merged = config.get_merged_config_for_role("assistant");
	merged.model = "ollama:fake-model".to_string();
	merged
}

pub(crate) fn fake_session(user_input: &str) -> ChatSession {
	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();
	session
		.add_user_message(user_input)
		.expect("add user message");
	session
}

// ---------------------------------------------------------------------------
// In-process evaluation provider for the supervisor's evaluation gates. No
// network: the seam runner routes to it through `evaluate::set_fake_provider`.
// Process-global, so tests installing it must hold `ENV_LOCK` or be `#[serial]`.
// ---------------------------------------------------------------------------

pub(crate) enum FakeEvaluationStep {
	/// Answer the request with exactly these answers.
	Answers(std::collections::BTreeMap<String, octolib::evaluation::Answer>),
	/// Fail as if the named key env var were unset.
	MissingKey(&'static str),
	/// Hang for this long before failing — exercises the runner timeout.
	Sleep(std::time::Duration),
}

pub(crate) struct FakeEvaluation {
	steps: StdMutex<VecDeque<FakeEvaluationStep>>,
	pub usage: octolib::evaluation::EvaluationUsage,
	/// Every request the runner sent, in order — assert on captured state.
	pub requests: StdMutex<Vec<octolib::evaluation::EvaluationRequest>>,
}

impl FakeEvaluation {
	pub(crate) fn requests(&self) -> Vec<octolib::evaluation::EvaluationRequest> {
		self.requests.lock().unwrap().clone()
	}
}

#[async_trait::async_trait]
impl octolib::evaluation::EvaluationProvider for FakeEvaluation {
	fn name(&self) -> &str {
		"fake"
	}
	fn supports_model(&self, _model: &str) -> bool {
		true
	}
	fn get_model_pricing(&self, _model: &str) -> Option<octolib::evaluation::EvaluationPricing> {
		None
	}
	async fn evaluate(
		&self,
		request: octolib::evaluation::EvaluationRequest,
	) -> octolib::evaluation::EvaluationResult<octolib::evaluation::EvaluationResponse> {
		self.requests.lock().unwrap().push(request.clone());
		let step = self.steps.lock().unwrap().pop_front();
		match step {
			Some(FakeEvaluationStep::Answers(answers)) => {
				Ok(octolib::evaluation::EvaluationResponse {
					model: request.model,
					answers,
					usage: self.usage.clone(),
				})
			}
			Some(FakeEvaluationStep::MissingKey(var)) => Err(
				octolib::evaluation::EvaluationError::MissingApiKey(var.to_string()),
			),
			Some(FakeEvaluationStep::Sleep(duration)) => {
				tokio::time::sleep(duration).await;
				Err(octolib::evaluation::EvaluationError::MissingApiKey(
					"slept".to_string(),
				))
			}
			None => Err(octolib::evaluation::EvaluationError::InvalidRequest(
				"fake evaluation: no scripted step left".to_string(),
			)),
		}
	}
}

/// Holds `ENV_LOCK` for its lifetime (the fake is process-global) and
/// uninstalls the fake on drop so a failed test cannot leak it into the next.
pub(crate) struct FakeEvaluationGuard(
	pub std::sync::Arc<FakeEvaluation>,
	#[allow(dead_code)] tokio::sync::MutexGuard<'static, ()>,
);

impl std::ops::Deref for FakeEvaluationGuard {
	type Target = FakeEvaluation;
	fn deref(&self) -> &FakeEvaluation {
		&self.0
	}
}

impl Drop for FakeEvaluationGuard {
	fn drop(&mut self) {
		crate::supervisor::evaluate::set_fake_provider(None);
	}
}

/// Takes `ENV_LOCK` itself — callers must not hold it already.
pub(crate) async fn install_fake_evaluation(steps: Vec<FakeEvaluationStep>) -> FakeEvaluationGuard {
	let lock = ENV_LOCK.lock().await;
	let fake = std::sync::Arc::new(FakeEvaluation {
		steps: StdMutex::new(steps.into()),
		usage: octolib::evaluation::EvaluationUsage {
			input_tokens: 100,
			output_tokens: 0,
			cost: Some(0.000_004_2),
		},
		requests: StdMutex::new(Vec::new()),
	});
	crate::supervisor::evaluate::set_fake_provider(Some(fake.clone()));
	FakeEvaluationGuard(fake, lock)
}

/// Per-seam evaluation counter from the `/info` snapshot; 0 when absent. The
/// stats sink is process-global, so assert on deltas around the call.
pub(crate) fn evaluate_counter(seam: crate::supervisor::evaluate::Seam, key: &str) -> u64 {
	crate::supervisor::stats::snapshot()
		.and_then(|s| s.get("evaluate")?.get(seam.name())?.get(key)?.as_u64())
		.unwrap_or(0)
}

/// Shorthand for a Noul answer map keyed by question id.
pub(crate) fn nouls(
	pairs: &[(&str, f64)],
) -> std::collections::BTreeMap<String, octolib::evaluation::Answer> {
	pairs
		.iter()
		.map(|(id, noul)| {
			(
				id.to_string(),
				octolib::evaluation::Answer::Noul { noul: *noul },
			)
		})
		.collect()
}

/// Shorthand for a single Choice answer under `id`.
pub(crate) fn choice(
	id: &str,
	chosen: &str,
	confidence: f64,
) -> std::collections::BTreeMap<String, octolib::evaluation::Answer> {
	std::collections::BTreeMap::from([(
		id.to_string(),
		octolib::evaluation::Answer::Choice {
			choice: chosen.to_string(),
			probabilities: std::collections::BTreeMap::from([(chosen.to_string(), confidence)]),
			confidence,
		},
	)])
}
