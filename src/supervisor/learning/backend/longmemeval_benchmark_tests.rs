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

//! Small public retrieval check over the official cleaned LongMemEval oracle
//! data. Five questions from each of six task types are selected in source
//! order; their relevant sessions form one shared distractor pool. This is not
//! the full benchmark and must be reported as a 30-question retrieval subset.

use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

const PER_TYPE: usize = 5;
const REFERENCE_DATASET_REVISION: &str = "98d7416c24c778c2fee6e6f3006e7a073259d48f";
const REFERENCE_DATASET_SHA256: &str =
	"821a2034d219ab45846873dd14c14f12cfe7776e73527a483f9dac095d38620c";
const TYPES: &[&str] = &[
	"temporal-reasoning",
	"multi-session",
	"knowledge-update",
	"single-session-user",
	"single-session-assistant",
	"single-session-preference",
];

#[derive(Deserialize)]
struct PublicItem {
	question_id: String,
	question_type: String,
	question: String,
	haystack_session_ids: Vec<String>,
	haystack_sessions: Vec<Vec<PublicMessage>>,
	answer_session_ids: Vec<String>,
}

#[derive(Deserialize)]
struct PublicMessage {
	role: String,
	content: String,
}

#[derive(Default, Serialize)]
struct PublicMetrics {
	cases: usize,
	hit_at_1: usize,
	hit_at_5: usize,
	reciprocal_rank_sum: f64,
}

impl PublicMetrics {
	fn observe(&mut self, expected: &HashSet<&str>, ranked: &[usize], lessons: &[Lesson]) {
		self.cases += 1;
		let ids: Vec<&str> = ranked
			.iter()
			.filter_map(|index| lessons.get(*index))
			.map(|lesson| lesson.source.as_str())
			.collect();
		if ids.first().is_some_and(|id| expected.contains(id)) {
			self.hit_at_1 += 1;
		}
		if ids.iter().take(5).any(|id| expected.contains(id)) {
			self.hit_at_5 += 1;
		}
		if let Some(rank) = ids.iter().position(|id| expected.contains(id)) {
			self.reciprocal_rank_sum += 1.0 / (rank as f64 + 1.0);
		}
	}

	fn report(&self) -> serde_json::Value {
		let denominator = self.cases.max(1) as f64;
		serde_json::json!({
			"cases": self.cases,
			"hit_at_1": self.hit_at_1 as f64 / denominator,
			"recall_at_5": self.hit_at_5 as f64 / denominator,
			"mrr": self.reciprocal_rank_sum / denominator,
		})
	}
}

#[derive(Default, Serialize)]
struct RewriteStats {
	calls: usize,
	cache_hits: usize,
	failures: usize,
	rejections: usize,
	latency_ms: u128,
	errors: Vec<String>,
}

fn load_cache(path: &Path) -> HashMap<String, Vec<String>> {
	std::fs::read_to_string(path)
		.ok()
		.and_then(|content| serde_json::from_str(&content).ok())
		.unwrap_or_default()
}

fn save_cache(path: &Path, cache: &HashMap<String, Vec<String>>) {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).unwrap();
	}
	std::fs::write(path, serde_json::to_vec_pretty(cache).unwrap()).unwrap();
}

async fn rewrite(
	config: &crate::config::Config,
	model: &str,
	question_id: &str,
	query: &str,
	cache: &mut HashMap<String, Vec<String>>,
	cache_path: &Path,
	stats: &mut RewriteStats,
) -> Vec<String> {
	let key = format!("longmemeval\n{model}\n{question_id}\n{query}");
	if let Some(patterns) = cache.get(&key) {
		match crate::supervisor::learning::inject::validate_retrieval_patterns(&patterns.join("\n"))
		{
			Ok(patterns) => {
				stats.cache_hits += 1;
				return patterns;
			}
			Err(_) => {
				stats.rejections += 1;
				return Vec::new();
			}
		}
	}

	let started = Instant::now();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let result =
		crate::supervisor::learning::inject::prepare_retrieval_query(config, query, model, rx)
			.await;
	stats.latency_ms += started.elapsed().as_millis();
	stats.calls += 1;
	match result {
		Ok(patterns) => {
			cache.insert(key, patterns.clone());
			save_cache(cache_path, cache);
			patterns
		}
		Err(error) if error.to_string().starts_with("invalid retrieval rewrite:") => {
			stats.rejections += 1;
			cache.insert(key, Vec::new());
			save_cache(cache_path, cache);
			Vec::new()
		}
		Err(error) => {
			stats.failures += 1;
			stats.errors.push(format!("{question_id}: {error:#}"));
			Vec::new()
		}
	}
}

fn rank_production(keyword: &[usize], dense: &[usize], lessons: &[Lesson]) -> Vec<usize> {
	let keyword_weight = if dense.is_empty() {
		1.0
	} else {
		KEYWORD_RRF_WEIGHT
	};
	let mut fused =
		weighted_reciprocal_rank_fusion(lessons.len(), &[(keyword, keyword_weight), (dense, 1.0)]);
	for (score, index) in &mut fused {
		*score *= importance_factor(lessons[*index].importance);
	}
	fused.sort_by(|left, right| {
		right
			.0
			.partial_cmp(&left.0)
			.unwrap_or(std::cmp::Ordering::Equal)
	});
	promote_sparse_rescue(&mut fused, keyword, lessons);
	fused.into_iter().map(|(_, index)| index).collect()
}

fn rank_adaptive(keyword: &[usize], dense: &[usize], lessons: &[Lesson]) -> Vec<usize> {
	let keyword_weight = if keyword
		.iter()
		.take(3)
		.any(|index| lessons[*index].importance < 0.4)
	{
		KEYWORD_RRF_WEIGHT
	} else {
		1.0
	};
	let mut fused =
		weighted_reciprocal_rank_fusion(lessons.len(), &[(keyword, keyword_weight), (dense, 1.0)]);
	for (score, index) in &mut fused {
		*score *= importance_factor(lessons[*index].importance);
	}
	fused.sort_by(|left, right| {
		right
			.0
			.partial_cmp(&left.0)
			.unwrap_or(std::cmp::Ordering::Equal)
	});
	promote_sparse_rescue(&mut fused, keyword, lessons);
	fused.into_iter().map(|(_, index)| index).collect()
}

fn rank_equal(keyword: &[usize], dense: &[usize], lessons: &[Lesson]) -> Vec<usize> {
	let mut fused = reciprocal_rank_fusion(lessons.len(), &[keyword, dense]);
	fused.sort_by(|left, right| {
		right
			.0
			.partial_cmp(&left.0)
			.unwrap_or(std::cmp::Ordering::Equal)
	});
	fused.into_iter().map(|(_, index)| index).collect()
}

fn top_ids(ranked: &[usize], lessons: &[Lesson]) -> Vec<String> {
	ranked
		.iter()
		.take(5)
		.filter_map(|index| lessons.get(*index))
		.map(|lesson| lesson.source.clone())
		.collect()
}

#[tokio::test]
#[ignore = "live public subset: set LONGMEMEVAL_ORACLE_JSON and benchmark model"]
async fn compact_longmemeval_oracle_retrieval() {
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
	let data_path = PathBuf::from(
		std::env::var_os("LONGMEMEVAL_ORACLE_JSON").expect("set LONGMEMEVAL_ORACLE_JSON"),
	);
	let data_bytes = std::fs::read(&data_path).expect("read LongMemEval oracle JSON");
	let dataset_sha256 = hex::encode(Sha256::digest(&data_bytes));
	let expected_sha256 = std::env::var("LONGMEMEVAL_EXPECTED_SHA256")
		.unwrap_or_else(|_| REFERENCE_DATASET_SHA256.to_string());
	assert_eq!(
		dataset_sha256, expected_sha256,
		"LongMemEval dataset drift: use the pinned revision or explicitly set LONGMEMEVAL_EXPECTED_SHA256"
	);
	let data: Vec<PublicItem> =
		serde_json::from_slice(&data_bytes).expect("parse LongMemEval oracle JSON");
	let mut counts: HashMap<&str, usize> = HashMap::new();
	let mut selected = Vec::new();
	for item in data {
		let Some(kind) = TYPES
			.iter()
			.copied()
			.find(|kind| *kind == item.question_type)
		else {
			continue;
		};
		let count = counts.entry(kind).or_default();
		if *count < PER_TYPE {
			*count += 1;
			selected.push(item);
		}
	}
	assert_eq!(selected.len(), TYPES.len() * PER_TYPE);

	let mut sessions = BTreeMap::new();
	for item in &selected {
		for (id, messages) in item
			.haystack_session_ids
			.iter()
			.zip(&item.haystack_sessions)
		{
			let content = messages
				.iter()
				.map(|message| format!("{}: {}", message.role, message.content))
				.collect::<Vec<_>>()
				.join("\n");
			sessions.entry(id.clone()).or_insert_with(|| Lesson {
				content,
				title: format!("session {id}"),
				source: id.clone(),
				importance: 0.5,
				..Default::default()
			});
		}
	}
	let lessons: Vec<Lesson> = sessions.into_values().collect();
	let config = crate::config::Config::load().expect("real config loads");
	let model = std::env::var("LEARNING_BENCH_MODEL")
		.unwrap_or_else(|_| config.supervisor.learning.model.clone());
	let cache_path = PathBuf::from("target/learning-benchmark/longmemeval-rewrite-cache.json");
	let mut cache = load_cache(&cache_path);
	let mut rewrite_stats = RewriteStats::default();
	let mut dense_metrics = PublicMetrics::default();
	let mut equal_metrics = PublicMetrics::default();
	let mut fixed_metrics = PublicMetrics::default();
	let mut production_metrics = PublicMetrics::default();
	let mut production_dense_latency_ms = 0u128;
	let mut details = Vec::new();

	for item in &selected {
		let patterns = rewrite(
			&config,
			&model,
			&item.question_id,
			&item.question,
			&mut cache,
			&cache_path,
			&mut rewrite_stats,
		)
		.await;
		let keyword = rank_by_keywords(&lessons, &patterns);
		let baseline_dense = cosine_scores(
			&lessons,
			&item.question,
			DenseScoring {
				chunk_tokens: crate::embeddings::EMBED_MAX_INPUT_TOKENS,
				max_chunk_weight: 0.0,
			},
		)
		.await
		.expect("baseline embedding scoring")
		.into_iter()
		.filter_map(|(score, index)| (score > COSINE_FLOOR).then_some(index))
		.collect::<Vec<_>>();
		let dense_started = Instant::now();
		let production_dense = cosine_scores(&lessons, &item.question, PRODUCTION_DENSE_SCORING)
			.await
			.expect("local embedding scoring")
			.into_iter()
			.filter_map(|(score, index)| (score > COSINE_FLOOR).then_some(index))
			.collect::<Vec<_>>();
		production_dense_latency_ms += dense_started.elapsed().as_millis();
		let equal = rank_equal(&keyword, &baseline_dense, &lessons);
		let fixed = rank_production(&keyword, &baseline_dense, &lessons);
		let production = rank_adaptive(&keyword, &production_dense, &lessons);
		let expected: HashSet<&str> = item.answer_session_ids.iter().map(String::as_str).collect();
		dense_metrics.observe(&expected, &baseline_dense, &lessons);
		equal_metrics.observe(&expected, &equal, &lessons);
		fixed_metrics.observe(&expected, &fixed, &lessons);
		production_metrics.observe(&expected, &production, &lessons);
		details.push(serde_json::json!({
			"question_id": item.question_id,
			"question_type": item.question_type,
			"question": item.question,
			"answer_session_ids": item.answer_session_ids,
			"patterns": patterns,
			"dense_top5": top_ids(&baseline_dense, &lessons),
			"equal_hybrid_top5": top_ids(&equal, &lessons),
			"fixed_weight_top5": top_ids(&fixed, &lessons),
			"production_top5": top_ids(&production, &lessons),
		}));
	}

	let report = serde_json::json!({
		"benchmark": "longmemeval-cleaned-oracle-stratified-30-retrieval",
		"source": "xiaowu0162/longmemeval-cleaned longmemeval_oracle.json",
		"dataset_revision": REFERENCE_DATASET_REVISION,
		"dataset_sha256": dataset_sha256,
		"model": model,
		"questions": selected.len(),
		"memory_sessions": lessons.len(),
		"rewrite": rewrite_stats,
		"supervisor_usage": crate::supervisor::stats::snapshot(),
		"production_dense_latency_ms": production_dense_latency_ms,
		"modes": {
			"dense": dense_metrics.report(),
			"equal_hybrid": equal_metrics.report(),
			"fixed_weight_025": fixed_metrics.report(),
			"production": production_metrics.report(),
		},
		"cases": details,
	});
	let report_path = PathBuf::from("target/learning-benchmark/longmemeval-oracle-30.json");
	std::fs::create_dir_all(report_path.parent().unwrap()).unwrap();
	std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
	println!("LONGMEMEVAL_REPORT={}\n{report}", report_path.display());

	assert_eq!(
		report["rewrite"]["failures"].as_u64().unwrap_or(u64::MAX),
		0
	);
	assert!(
		report["modes"]["production"]["recall_at_5"]
			.as_f64()
			.unwrap_or_default()
			>= 0.95,
		"public retrieval recall below contract: {report}"
	);
}
