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

//! Tiny live precision check for the consolidation proposer + independent
//! verifier. False accepts are forbidden; rejecting a safe merge costs storage
//! efficiency but does not corrupt durable knowledge.

use super::*;
use std::path::PathBuf;
use std::time::Instant;

struct Case {
	id: &'static str,
	expect_merge: bool,
	sources: [Lesson; 2],
}

fn orientation(id: &str, content: String, tags: &[&str]) -> Lesson {
	Lesson {
		content,
		title: id.to_string(),
		memory_type: "orientation".to_string(),
		importance: 0.7,
		confidence: "medium".to_string(),
		tags: tags.iter().map(|tag| tag.to_string()).collect(),
		source: format!("benchmark:{id}"),
		role: "developer".to_string(),
		project: "benchmark".to_string(),
		created: "2026-08-01T00:00:00Z".to_string(),
		..Default::default()
	}
}

fn cases() -> Vec<Case> {
	let continuation_context = "The resolved provider and model form the continuation identity. An invalid cursor is local to that identity. Recovery must not reinterpret provider resolution, silently cross a model boundary, or claim success when same-identity retry fails. Tool side effects must be checked before retry because a rejected response cursor does not prove that earlier remote work was absent. ".repeat(3);
	let cache_context = "Conversation compression rebuilds the request in a fixed order. Stale non-system cache markers are cleared. The summary, active skills, fidelity constraints, continuation wrapper, and final state are all inserted before rolling cache boundaries are aligned. Aligning early omits reinjected constraints from the cached prefix and repeatedly bills or loses load-bearing context. ".repeat(3);
	let retry_context = "Payment retries occur after an ambiguous transport failure. The server may already have committed the logical operation, so retry identity determines whether the action is deduplicated or repeated. The chosen rule must remain explicit and must not hide uncertainty about prior side effects. ".repeat(3);
	let webhook_context = "Webhook authenticity depends on the byte representation covered by the sender's signature. Middleware ordering and parsing can change whitespace or serialization. The reusable rule concerns the exact verification boundary and must not be generalized into unrelated authentication protocols. ".repeat(3);
	let oauth_context = "OAuth authorization with PKCE retains a verifier for token exchange and validates callback state. The challenge, verifier, and browser correlation have a different lifecycle from signed webhook bodies. Combining them would create a misleading procedure with unrelated applicability conditions. ".repeat(3);
	vec![
		Case {
			id: "merge-continuation",
			expect_merge: true,
			sources: [
				orientation(
					"continuation identity",
					format!("{continuation_context} Durable rule: clear only the rejected continuation and retain the exact resolved provider/model tuple."),
					&["provider", "continuation"],
				),
				orientation(
					"continuation recovery",
					format!("{continuation_context} Durable rule: retry only with the same resolved identity and fail closed if recovery for that identity fails."),
					&["provider", "continuation"],
				),
			],
		},
		Case {
			id: "merge-cache-order",
			expect_merge: true,
			sources: [
				orientation(
					"cache reinjection order",
					format!("{cache_context} Durable rule: marker alignment happens after summary insertion and every reinjection."),
					&["compression", "cache"],
				),
				orientation(
					"cache rolling boundary",
					format!("{cache_context} Durable rule: clear stale markers first, then choose the final rolling boundaries from the fully rebuilt request."),
					&["compression", "cache"],
				),
			],
		},
		Case {
			id: "reject-contradictory-retry",
			expect_merge: false,
			sources: [
				orientation(
					"stable idempotency",
					format!("{retry_context} Required rule: reuse the same stable idempotency key for retries of one logical payment."),
					&["payment", "idempotency"],
				),
				orientation(
					"fresh idempotency",
					format!("{retry_context} Conflicting rule: every retry must use a newly generated idempotency key, even for the same logical payment."),
					&["payment", "idempotency"],
				),
			],
		},
		Case {
			id: "reject-distinct-security",
			expect_merge: false,
			sources: [
				orientation(
					"webhook bytes",
					format!("{webhook_context} Verify the signature against exact raw request bytes before parsing JSON."),
					&["stripe", "webhook"],
				),
				orientation(
					"oauth pkce",
					format!("{oauth_context} Retain the PKCE verifier client-side and validate state when processing the callback."),
					&["oauth", "pkce"],
				),
			],
		},
	]
}

#[tokio::test]
#[ignore = "live consolidation proposer/verifier benchmark"]
async fn compact_consolidation_precision() {
	assert_eq!(
		std::env::var("LEARNING_BENCH_LIVE").as_deref(),
		Ok("1"),
		"set LEARNING_BENCH_LIVE=1"
	);
	crate::config::get_env_tracker()
		.lock()
		.unwrap()
		.load_dotenv_override()
		.expect("load server credentials");
	let mut config = crate::config::Config::load().expect("real config loads");
	let model = std::env::var("LEARNING_BENCH_MODEL")
		.unwrap_or_else(|_| config.get_supervisor_model_profile().model);
	config.supervisor.model.model = Some(model.clone());
	config.supervisor.model.model = Some(model.clone());

	let mut results = Vec::new();
	let mut false_accepts = 0usize;
	let mut true_accepts = 0usize;
	let mut safe_cases = 0usize;
	let started = Instant::now();
	for case in cases() {
		let merged = propose_and_verify(&config, &case.sources).await;
		let accepted = merged.is_some();
		if case.expect_merge {
			safe_cases += 1;
			if accepted {
				true_accepts += 1;
			}
		} else if accepted {
			false_accepts += 1;
		}
		results.push(serde_json::json!({
			"id": case.id,
			"expected_merge": case.expect_merge,
			"accepted": accepted,
			"source_tokens": storage_tokens(&case.sources),
			"merged_tokens": merged.as_ref().map(memory_tokens),
		}));
	}
	let report = serde_json::json!({
		"benchmark": "octomind-consolidation-contract-v1",
		"model": model,
		"cases": results,
		"false_accepts": false_accepts,
		"safe_merge_acceptance": true_accepts as f64 / safe_cases as f64,
		"latency_ms": started.elapsed().as_millis(),
		"supervisor_usage": crate::supervisor::stats::snapshot(),
	});
	let path = PathBuf::from("target/learning-benchmark/consolidation.json");
	std::fs::create_dir_all(path.parent().unwrap()).unwrap();
	std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
	println!("CONSOLIDATION_REPORT={}\n{report}", path.display());
	assert_eq!(false_accepts, 0, "unsafe consolidation accepted: {report}");
	assert!(
		true_accepts >= 1,
		"consolidator rejected every safe merge: {report}"
	);
}
