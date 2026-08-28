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

//! Compact paid retrieval benchmark. Ignored by default: it uses the configured
//! learning model only for production query rewriting. Embeddings are local and
//! rewrite responses are cached, so reruns after the first are free.

use super::*;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy)]
struct Domain {
	id: &'static str,
	tags: &'static [&'static str],
	current: &'static str,
	stale: &'static str,
	exact: &'static str,
	paraphrase: &'static str,
	noisy: &'static str,
	indirect: &'static str,
	unrelated: &'static str,
}

const DOMAINS: &[Domain] = &[
	Domain {
		id: "continuation",
		tags: &["invalid_continuation", "provider", "model identity"],
		current: "Current verified rule: when a continuation is rejected, clear only that continuation and retry the exact resolved provider and model. Never silently switch models.",
		stale: "Obsolete rule: after invalid_continuation, switch to any available fallback model so the request succeeds.",
		exact: "How must invalid_continuation recovery preserve the resolved provider and model?",
		paraphrase: "The previous response cursor is bad. Is it safe to route the retry through a different model?",
		noisy: "Continue the provider repair; billing and UI are unrelated. I need the rule for a rejected cursor and stable model identity.",
		indirect: "Recovery works, but the retry unexpectedly changed which model answered. What constraint did we violate?",
		unrelated: "What is tomorrow's rainfall forecast for Chiang Mai?",
	},
	Domain {
		id: "worktree",
		tags: &["git", "worktree", "checkout"],
		current: "Current user rule: use a Git worktree means create and work inside a separate checkout directory, leaving the current checkout untouched.",
		stale: "Obsolete interpretation: worktree means keep edits uncommitted on the current branch and avoid merging them.",
		exact: "When I say use a worktree, what must remain untouched?",
		paraphrase: "Do this in another checkout directory instead of modifying the folder I am currently using.",
		noisy: "The feature is unrelated to rebasing or release tags; isolate the implementation the way I previously defined worktree.",
		indirect: "You left local staged edits in my active checkout. Why does that fail my isolation instruction?",
		unrelated: "Give me a recipe for sourdough pancakes with blueberries.",
	},
	Domain {
		id: "stripe",
		tags: &["stripe", "webhook", "signature", "raw body"],
		current: "Current verified rule: verify the Stripe webhook signature against the exact raw request bytes before parsing JSON.",
		stale: "Obsolete rule: decode and normalize the Stripe JSON first, then verify the signature against the re-encoded object.",
		exact: "Which payload must Stripe webhook signature verification use?",
		paraphrase: "Can middleware deserialize the event before authenticity checking?",
		noisy: "Ignore checkout CSS and invoices; the endpoint receives a signed event and a body parser currently runs first.",
		indirect: "Valid webhook signatures fail only when whitespace in the incoming JSON changes. What ordering constraint matters?",
		unrelated: "How many moons does Saturn currently have?",
	},
	Domain {
		id: "money",
		tags: &["money", "minor units", "currency", "integer"],
		current: "Current project rule: persist and calculate money in integer minor units with an explicit currency; format decimals only at display boundaries.",
		stale: "Obsolete rule: store monetary amounts as floating point major units for convenient arithmetic.",
		exact: "How are monetary values represented in persistence and calculations?",
		paraphrase: "Should a payout of ten dollars be stored as 10.0 or as currency plus the smallest units?",
		noisy: "The table has timestamps and partner IDs, but this review is about avoiding rounding drift in amounts.",
		indirect: "Repeated percentage operations changed a payout by one cent. Which established representation prevents this?",
		unrelated: "Explain how photosynthesis converts light into chemical energy.",
	},
	Domain {
		id: "pkce",
		tags: &["oauth", "pkce", "verifier", "state"],
		current: "Current security rule: generate the PKCE verifier client-side, send only its challenge during authorization, retain the verifier for token exchange, and validate state on callback.",
		stale: "Obsolete rule: send the plain PKCE verifier in the authorization URL and skip callback state validation.",
		exact: "What is sent during OAuth authorization and what is retained for PKCE token exchange?",
		paraphrase: "Where should the secret proof string live while the browser handles login?",
		noisy: "The provider discovery document is valid; focus on the callback correlation and proof challenge lifecycle.",
		indirect: "The callback succeeded but could be bound to another browser session. Which two retained values prevent that?",
		unrelated: "Recommend a quiet mechanical keyboard for an open office.",
	},
	Domain {
		id: "cancellation",
		tags: &["tokio", "cancellation", "child process", "JoinHandle"],
		current: "Current runtime rule: cancellation must signal or abort owned work and await its JoinHandle or child process cleanup; dropping a handle alone is not cleanup.",
		stale: "Obsolete rule: dropping a Tokio JoinHandle automatically cancels and cleans up the spawned operation.",
		exact: "What must happen to owned Tokio work when a session is cancelled?",
		paraphrase: "Is forgetting the task handle enough to stop its subprocess?",
		noisy: "Logging format is irrelevant; shutdown still leaves a worker alive after its owner disappears.",
		indirect: "The session closed but its command continues consuming CPU. Which lifecycle obligation was missed?",
		unrelated: "Translate the phrase good morning into Icelandic.",
	},
	Domain {
		id: "cache_markers",
		tags: &["prompt cache", "compression", "marker", "reinjection"],
		current: "Current compression rule: clear stale non-system markers and align the rolling cache markers only after summary insertion and every skill, fidelity, and continuation reinjection.",
		stale: "Obsolete rule: align prompt-cache markers immediately after inserting the summary, before reinjected context is appended.",
		exact: "When are cache markers aligned during conversation compression?",
		paraphrase: "Should the second cache boundary be chosen before or after restored skills and continuation context?",
		noisy: "Token pricing is not the issue; the compressed request keeps missing the newly restored context from its cache boundary.",
		indirect: "The summary is cached but the reinserted constraints are repeatedly billed. Which ordering rule is wrong?",
		unrelated: "What exercises improve ankle mobility for runners?",
	},
	Domain {
		id: "mcp_timeout",
		tags: &["mcp", "timeout", "progress", "side effects"],
		current: "Current MCP rule: timeout is an idle timeout reset by meaningful progress, with an absolute cap. After timeout, cancellation side effects remain uncertain and must be checked before retry.",
		stale: "Obsolete rule: MCP timeout is a fixed wall-clock deadline and a timed-out call is guaranteed to have produced no side effects.",
		exact: "How does MCP progress affect timeout and what must be checked after one fires?",
		paraphrase: "A long tool keeps reporting useful progress. Should its ordinary deadline still expire, and is retry automatically safe?",
		noisy: "The command text is valid; distinguish inactivity from total runtime and account for uncertain remote mutation.",
		indirect: "The retry duplicated an external action after the first call timed out. Which recovery assumption was invalid?",
		unrelated: "Summarize the plot of The Importance of Being Earnest.",
	},
];

// Final challenge slice: added only after calibration/holdout fixed the fusion,
// phrase-validation, and sparse-rescue defects. Do not tune constants on it.
const CHALLENGE_DOMAINS: &[Domain] = &[
	Domain {
		id: "idempotency",
		tags: &["payment", "idempotency", "retry", "request key"],
		current: "Current payment rule: derive a stable idempotency key from the logical operation and reuse it across uncertain retries; never mint a new key until prior side effects are resolved.",
		stale: "Obsolete rule: create a fresh idempotency key for every HTTP attempt so retries are independently traceable.",
		exact: "How should payment retry idempotency keys behave?",
		paraphrase: "The first charge request timed out and its side effect is unknown. Should the next attempt use a new identity?",
		noisy: "Logging and receipt emails are unrelated; prevent a duplicated charge after an ambiguous network failure.",
		indirect: "A customer was charged twice because recovery treated the retry as a new logical operation. Which retained constraint was broken?",
		unrelated: "How do I prune a mature lemon tree?",
	},
	Domain {
		id: "attachment_queue",
		tags: &["attachments", "queue", "transcript", "atomic"],
		current: "Current UI rule: queue text and attachments atomically as one payload, never auto-send a transcript, and preserve attachments when a prompt waits behind another turn.",
		stale: "Obsolete rule: enqueue text first, upload attachments separately, and automatically submit speech transcripts when recording ends.",
		exact: "How are queued prompts and attachments kept together?",
		paraphrase: "A voice transcript appeared as a sent message before its image finished uploading. What queue boundary should prevent that?",
		noisy: "Panel colors are irrelevant; focus on preserving media while another prompt is already running.",
		indirect: "The delayed prompt arrived, but its image belonged to the following turn. Which atomicity rule was lost?",
		unrelated: "Compare the nutritional value of lentils and chickpeas.",
	},
	Domain {
		id: "migration_integrity",
		tags: &["migration", "constraint", "database", "fresh install"],
		current: "Current database rule: a consolidated fresh-install schema must preserve every application-required column and integrity constraint, then be verified against a clean database.",
		stale: "Obsolete rule: once historical migration files are consolidated, omitted columns can be inferred by the application at runtime without clean-database verification.",
		exact: "What must a consolidated database baseline preserve and how is it verified?",
		paraphrase: "The new install schema parses but a field formerly added by migration disappeared. Is source inspection alone sufficient?",
		noisy: "Ignore seed styling and old filenames; confirm the baseline owns all runtime-required integrity.",
		indirect: "Upgraded databases work while fresh installs fail because an application field is absent. Which consolidation invariant catches this?",
		unrelated: "Why do migratory birds fly in a V formation?",
	},
	Domain {
		id: "destructive_scope",
		tags: &["delete", "sandbox", "path", "destructive"],
		current: "Current safety rule: resolve and validate exact destructive targets first; never recursively delete a broad root, home directory, unresolved variable, or ambiguous glob.",
		stale: "Obsolete rule: recursive cleanup may use the workspace root or an unresolved environment variable when the command is expected to run in a sandbox.",
		exact: "Which paths are forbidden as recursive deletion targets?",
		paraphrase: "Is a sandbox enough protection for cleanup through an environment variable that might be empty?",
		noisy: "Disk usage reporting is unrelated; the cleanup target expands from a variable and has not been validated.",
		indirect: "A cleanup command resolved its target to the checkout root. Which precondition should have stopped execution?",
		unrelated: "Describe the harmonic structure of a twelve-bar blues progression.",
	},
];

#[derive(Clone)]
struct Case {
	id: String,
	category: &'static str,
	split: &'static str,
	query: &'static str,
	expected: Option<String>,
	stale: String,
}

#[derive(Default, Serialize)]
struct Metrics {
	cases: usize,
	positives: usize,
	hit_at_1: usize,
	hit_at_5: usize,
	mrr: f64,
	abstentions: usize,
	correct_abstentions: usize,
	stale_at_1: usize,
}

impl Metrics {
	fn observe(&mut self, case: &Case, ranked: &[usize], lessons: &[Lesson]) {
		self.cases += 1;
		let ids: Vec<&str> = ranked
			.iter()
			.filter_map(|index| lessons.get(*index))
			.map(|lesson| lesson.source.as_str())
			.collect();
		if let Some(expected) = case.expected.as_deref() {
			self.positives += 1;
			if ids.first().copied() == Some(expected) {
				self.hit_at_1 += 1;
			}
			if ids.iter().take(5).any(|id| *id == expected) {
				self.hit_at_5 += 1;
			}
			if let Some(rank) = ids.iter().position(|id| *id == expected) {
				self.mrr += 1.0 / (rank as f64 + 1.0);
			}
			if ids.first().copied() == Some(case.stale.as_str()) {
				self.stale_at_1 += 1;
			}
		} else {
			self.abstentions += 1;
			if ids.is_empty() {
				self.correct_abstentions += 1;
			}
		}
	}

	fn rates(&self) -> serde_json::Value {
		serde_json::json!({
			"cases": self.cases,
			"positives": self.positives,
			"hit_at_1": ratio(self.hit_at_1, self.positives),
			"recall_at_5": ratio(self.hit_at_5, self.positives),
			"mrr": if self.positives == 0 { 0.0 } else { self.mrr / self.positives as f64 },
			"abstention_accuracy": ratio(self.correct_abstentions, self.abstentions),
			"stale_at_1": self.stale_at_1,
		})
	}
}

fn ratio(value: usize, total: usize) -> f64 {
	if total == 0 {
		0.0
	} else {
		value as f64 / total as f64
	}
}

fn lessons() -> Vec<Lesson> {
	let mut out = Vec::new();
	for domain in DOMAINS.iter().chain(CHALLENGE_DOMAINS) {
		out.push(Lesson {
			content: domain.current.to_string(),
			title: format!("{} current rule", domain.id),
			tags: domain.tags.iter().map(|tag| tag.to_string()).collect(),
			source: format!("{}:current", domain.id),
			importance: 0.9,
			confidence: "high".to_string(),
			created: "2026-08-01T00:00:00Z".to_string(),
			..Default::default()
		});
		out.push(Lesson {
			content: domain.stale.to_string(),
			title: format!("{} obsolete rule", domain.id),
			tags: domain.tags.iter().map(|tag| tag.to_string()).collect(),
			source: format!("{}:stale", domain.id),
			importance: 0.2,
			confidence: "medium".to_string(),
			created: "2025-01-01T00:00:00Z".to_string(),
			..Default::default()
		});
	}
	out.extend([
		Lesson {
			content: "Use CSS container queries to adapt card layouts to component width.".into(),
			title: "container query layout".into(),
			tags: vec!["css".into()],
			source: "distractor:css".into(),
			..Default::default()
		},
		Lesson {
			content: "JPEG orientation metadata must be applied before generating thumbnails."
				.into(),
			title: "image orientation".into(),
			tags: vec!["jpeg".into()],
			source: "distractor:image".into(),
			..Default::default()
		},
		Lesson {
			content: "DNS negative answers can be cached according to the zone SOA minimum.".into(),
			title: "negative DNS cache".into(),
			tags: vec!["dns".into()],
			source: "distractor:dns".into(),
			..Default::default()
		},
		Lesson {
			content: "Accessibility focus order should follow the visual and DOM reading order."
				.into(),
			title: "focus order".into(),
			tags: vec!["a11y".into()],
			source: "distractor:a11y".into(),
			..Default::default()
		},
	]);
	out
}

fn cases(split: &str) -> Vec<Case> {
	let mut out = Vec::new();
	for domain in DOMAINS {
		for (category, case_split, query) in [
			("exact", "calibration", domain.exact),
			("paraphrase", "calibration", domain.paraphrase),
			("noisy", "calibration", domain.noisy),
			("indirect", "holdout", domain.indirect),
		] {
			out.push(Case {
				id: format!("{}:{category}", domain.id),
				category,
				split: case_split,
				query,
				expected: Some(format!("{}:current", domain.id)),
				stale: format!("{}:stale", domain.id),
			});
		}
		out.push(Case {
			id: format!("{}:abstain", domain.id),
			category: "abstain",
			split: "holdout",
			query: domain.unrelated,
			expected: None,
			stale: format!("{}:stale", domain.id),
		});
	}
	for domain in CHALLENGE_DOMAINS {
		for (category, query) in [
			("paraphrase", domain.paraphrase),
			("indirect", domain.indirect),
		] {
			out.push(Case {
				id: format!("{}:{category}", domain.id),
				category,
				split: "challenge",
				query,
				expected: Some(format!("{}:current", domain.id)),
				stale: format!("{}:stale", domain.id),
			});
		}
		out.push(Case {
			id: format!("{}:abstain", domain.id),
			category: "abstain",
			split: "challenge",
			query: domain.unrelated,
			expected: None,
			stale: format!("{}:stale", domain.id),
		});
	}
	if split == "all" {
		out
	} else {
		out.into_iter().filter(|case| case.split == split).collect()
	}
}

fn raw_patterns(query: &str) -> Vec<String> {
	const STOP: &[&str] = &[
		"about", "after", "before", "could", "from", "have", "into", "must", "should", "their",
		"there", "these", "this", "what", "when", "where", "which", "with", "would", "your",
	];
	let mut out: Vec<String> = query
		.split(|character: char| !character.is_alphanumeric() && character != '_')
		.map(str::to_ascii_lowercase)
		.filter(|term| term.len() >= 4 && !STOP.contains(&term.as_str()))
		.take(8)
		.collect();
	out.sort();
	out.dedup();
	out
}

fn rerank_single(ranking: &[usize], lessons: &[Lesson]) -> Vec<usize> {
	let mut scored: Vec<(f32, usize)> = ranking
		.iter()
		.enumerate()
		.map(|(rank, index)| {
			let base = 1.0 / (RRF_K + rank as f32 + 1.0);
			(base * importance_factor(lessons[*index].importance), *index)
		})
		.collect();
	scored.sort_by(|left, right| {
		right
			.0
			.partial_cmp(&left.0)
			.unwrap_or(std::cmp::Ordering::Equal)
	});
	scored.into_iter().map(|(_, index)| index).collect()
}

fn rerank_hybrid(keyword: &[usize], dense: &[usize], lessons: &[Lesson]) -> Vec<usize> {
	let mut fused = reciprocal_rank_fusion(lessons.len(), &[keyword, dense]);
	for (score, index) in &mut fused {
		*score *= importance_factor(lessons[*index].importance);
	}
	fused.sort_by(|left, right| {
		right
			.0
			.partial_cmp(&left.0)
			.unwrap_or(std::cmp::Ordering::Equal)
	});
	fused.into_iter().map(|(_, index)| index).collect()
}

fn rerank_weighted_hybrid(
	keyword: &[usize],
	dense: &[usize],
	lessons: &[Lesson],
	keyword_weight: f32,
) -> Vec<usize> {
	let mut scores = vec![0.0_f32; lessons.len()];
	for (rank, index) in keyword.iter().enumerate() {
		scores[*index] += keyword_weight / (RRF_K + rank as f32 + 1.0);
	}
	for (rank, index) in dense.iter().enumerate() {
		scores[*index] += 1.0 / (RRF_K + rank as f32 + 1.0);
	}
	let mut ranked: Vec<(f32, usize)> = scores
		.into_iter()
		.enumerate()
		.filter_map(|(index, score)| {
			(score > 0.0).then_some((score * importance_factor(lessons[index].importance), index))
		})
		.collect();
	ranked.sort_by(|left, right| {
		right
			.0
			.partial_cmp(&left.0)
			.unwrap_or(std::cmp::Ordering::Equal)
	});
	ranked.into_iter().map(|(_, index)| index).collect()
}

fn dense_at_floor(scores: &[(f32, usize)], floor: f32) -> Vec<usize> {
	scores
		.iter()
		.filter_map(|(score, index)| (*score > floor).then_some(*index))
		.collect()
}

fn sparse_rescue(mut ranked: Vec<usize>, sparse: &[usize], lessons: &[Lesson]) -> Vec<usize> {
	let candidate = sparse.iter().take(3).copied().max_by(|left, right| {
		lessons[*left]
			.importance
			.partial_cmp(&lessons[*right].importance)
			.unwrap_or(std::cmp::Ordering::Equal)
	});
	let Some(candidate) = candidate else {
		return ranked;
	};
	if ranked.iter().take(5).any(|index| *index == candidate) {
		return ranked;
	}
	ranked.retain(|index| *index != candidate);
	let position = ranked.len().min(4);
	ranked.insert(position, candidate);
	ranked
}

fn source_ids(ranked: &[usize], lessons: &[Lesson], limit: usize) -> Vec<String> {
	ranked
		.iter()
		.take(limit)
		.filter_map(|index| lessons.get(*index))
		.map(|lesson| lesson.source.clone())
		.collect()
}

fn cache_path() -> PathBuf {
	std::env::var_os("LEARNING_BENCH_REWRITE_CACHE")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from("target/learning-benchmark/rewrite-cache.json"))
}

fn load_cache(path: &Path) -> HashMap<String, Vec<String>> {
	std::fs::read_to_string(path)
		.ok()
		.and_then(|content| serde_json::from_str(&content).ok())
		.unwrap_or_default()
}

fn save_cache(path: &Path, cache: &HashMap<String, Vec<String>>) {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).expect("create benchmark cache dir");
	}
	std::fs::write(
		path,
		serde_json::to_vec_pretty(cache).expect("serialize rewrite cache"),
	)
	.expect("write rewrite cache");
}

#[tokio::test]
#[ignore = "live compact benchmark: local embeddings plus configured query-rewrite model"]
async fn compact_learning_retrieval_frontier() {
	assert_eq!(
		std::env::var("LEARNING_BENCH_LIVE").as_deref(),
		Ok("1"),
		"set LEARNING_BENCH_LIVE=1 to authorize configured rewrite-model calls"
	);
	crate::config::get_env_tracker()
		.lock()
		.unwrap()
		.load_dotenv_override()
		.expect("load user-scope benchmark credentials");
	let split = std::env::var("LEARNING_BENCH_SPLIT").unwrap_or_else(|_| "calibration".into());
	assert!(matches!(
		split.as_str(),
		"calibration" | "holdout" | "challenge" | "all"
	));
	let config = crate::config::Config::load().expect("real config loads");
	let rewrite_model = std::env::var("LEARNING_BENCH_MODEL")
		.unwrap_or_else(|_| config.supervisor.learning.model.clone());
	let lessons = lessons();
	let cases = cases(&split);

	// Warm and persist stable lesson vectors outside measured query latency.
	let lesson_texts: Vec<String> = lessons
		.iter()
		.map(|lesson| {
			format!(
				"{} {} {}",
				lesson.title,
				lesson.content,
				lesson.tags.join(" ")
			)
		})
		.collect();
	crate::embeddings::embed_many(&lesson_texts)
		.await
		.expect("local embedding model is available");

	let path = cache_path();
	let mut cache = load_cache(&path);
	let mut cache_hits = 0usize;
	let mut rewrite_calls = 0usize;
	let mut rewrite_failures = 0usize;
	let mut rewrite_rejections = 0usize;
	let mut rewrite_errors = Vec::new();
	let mut rewrite_ms = 0u128;
	let mut embedding_ms = 0u128;
	let mut keyword_raw = Metrics::default();
	let mut dense = Metrics::default();
	let mut hybrid_raw = Metrics::default();
	let mut hybrid_rewrite = Metrics::default();
	let mut fixed_weight = Metrics::default();
	let mut production = Metrics::default();
	let mut case_details = Vec::new();

	for case in &cases {
		let raw = raw_patterns(case.query);
		let cache_key = format!("{}\n{}", rewrite_model, case.query);
		let rewritten = if let Some(patterns) = cache.get(&cache_key) {
			match crate::supervisor::learning::inject::validate_retrieval_patterns(
				&patterns.join("\n"),
			) {
				Ok(validated) => {
					cache_hits += 1;
					validated
				}
				Err(_) => {
					rewrite_rejections += 1;
					raw.clone()
				}
			}
		} else {
			let started = Instant::now();
			let (_tx, rx) = tokio::sync::watch::channel(false);
			let result = crate::supervisor::learning::inject::prepare_retrieval_query(
				&config,
				case.query,
				"file",
				&rewrite_model,
				rx,
			)
			.await;
			rewrite_ms += started.elapsed().as_millis();
			rewrite_calls += 1;
			match result {
				Ok(patterns) if !patterns.is_empty() => {
					cache.insert(cache_key, patterns.clone());
					save_cache(&path, &cache);
					patterns
				}
				Ok(_) => {
					rewrite_failures += 1;
					rewrite_errors.push(format!("{}: empty rewrite", case.id));
					raw.clone()
				}
				Err(error) => {
					if error.to_string().starts_with("invalid retrieval rewrite:") {
						rewrite_rejections += 1;
						cache.insert(cache_key, Vec::new());
						save_cache(&path, &cache);
					} else {
						rewrite_failures += 1;
						rewrite_errors.push(format!("{}: {error:#}", case.id));
					}
					raw.clone()
				}
			}
		};

		let keyword_raw_rank = rerank_single(&rank_by_keywords(&lessons, &raw), &lessons);
		let started = Instant::now();
		let scores = cosine_scores(&lessons, case.query, PRODUCTION_DENSE_SCORING)
			.await
			.expect("embedding scoring succeeds");
		embedding_ms += started.elapsed().as_millis();
		let dense_indices = dense_at_floor(&scores, COSINE_FLOOR);
		let dense_rank = rerank_single(&dense_indices, &lessons);
		let hybrid_raw_rank =
			rerank_hybrid(&rank_by_keywords(&lessons, &raw), &dense_indices, &lessons);
		let hybrid_rewrite_rank = rerank_hybrid(
			&rank_by_keywords(&lessons, &rewritten),
			&dense_indices,
			&lessons,
		);
		let rewrite_keywords = rank_by_keywords(&lessons, &rewritten);
		let fixed_weight_rank = sparse_rescue(
			rerank_weighted_hybrid(&rewrite_keywords, &dense_indices, &lessons, 0.25),
			&rewrite_keywords,
			&lessons,
		);
		let production_rank = sparse_rescue(
			rerank_weighted_hybrid(
				&rewrite_keywords,
				&dense_indices,
				&lessons,
				adaptive_keyword_weight(&rewrite_keywords, &dense_indices, &lessons),
			),
			&rewrite_keywords,
			&lessons,
		);
		case_details.push(serde_json::json!({
			"id": case.id,
			"category": case.category,
			"split": case.split,
			"query": case.query,
			"expected": case.expected,
			"raw_patterns": raw,
			"rewritten_patterns": rewritten,
			"keyword_raw_top5": source_ids(&keyword_raw_rank, &lessons, 5),
			"dense_top5": source_ids(&dense_rank, &lessons, 5),
			"hybrid_raw_top5": source_ids(&hybrid_raw_rank, &lessons, 5),
			"hybrid_rewrite_top5": source_ids(&hybrid_rewrite_rank, &lessons, 5),
			"fixed_weight_top5": source_ids(&fixed_weight_rank, &lessons, 5),
			"production_top5": source_ids(&production_rank, &lessons, 5),
		}));

		keyword_raw.observe(case, &keyword_raw_rank, &lessons);
		dense.observe(case, &dense_rank, &lessons);
		hybrid_raw.observe(case, &hybrid_raw_rank, &lessons);
		hybrid_rewrite.observe(case, &hybrid_rewrite_rank, &lessons);
		fixed_weight.observe(case, &fixed_weight_rank, &lessons);
		production.observe(case, &production_rank, &lessons);
	}

	let report = serde_json::json!({
		"benchmark": "octomind-memory-contract-v1",
		"split": split,
		"case_count": cases.len(),
		"learning_model": rewrite_model,
		"rewrite": {
			"calls": rewrite_calls,
			"cache_hits": cache_hits,
			"failures": rewrite_failures,
			"rejections": rewrite_rejections,
			"errors": rewrite_errors,
			"latency_ms": rewrite_ms,
		},
		"embedding_query_latency_ms": embedding_ms,
		"supervisor_usage": crate::supervisor::stats::snapshot(),
		"modes": {
			"keyword_raw": keyword_raw.rates(),
			"dense": dense.rates(),
			"hybrid_raw": hybrid_raw.rates(),
			"hybrid_rewrite": hybrid_rewrite.rates(),
			"fixed_weight_025": fixed_weight.rates(),
			"production_adaptive_hybrid": production.rates(),
		},
		"cases": case_details,
	});
	let report_path = std::env::var_os("LEARNING_BENCH_REPORT")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from(format!("target/learning-benchmark/{split}.json")));
	if let Some(parent) = report_path.parent() {
		std::fs::create_dir_all(parent).unwrap();
	}
	std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
	println!("LEARNING_BENCH_REPORT={}\n{report}", report_path.display());

	let full = &report["modes"]["production_adaptive_hybrid"];
	assert_eq!(rewrite_failures, 0, "query rewriting must not fail");
	assert!(
		full["recall_at_5"].as_f64().unwrap_or_default() >= 0.90,
		"full retrieval recall@5 below contract: {report}"
	);
	assert_eq!(
		full["stale_at_1"].as_u64().unwrap_or(u64::MAX),
		0,
		"stale memory outranked its correction: {report}"
	);
	if full["cases"].as_u64().unwrap_or_default() > full["positives"].as_u64().unwrap_or_default() {
		assert!(
			full["abstention_accuracy"].as_f64().unwrap_or_default() >= 0.75,
			"full retrieval abstention below contract: {report}"
		);
	}
}
