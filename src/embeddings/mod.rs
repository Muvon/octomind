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

//! Embedding infrastructure — internal model, no user config.
//!
//! Wraps octolib's `FastEmbedProviderImpl` (gated behind octolib's `fastembed`
//! feature) with a process-global model singleton and an in-memory cache. Used
//! by capability discovery and tool gating to score natural-language intent
//! against tool/capability descriptions.
//!
//! The model identity is an implementation detail. Users do not configure it
//! and cannot change it. Default: `BAAI/bge-small-en-v1.5` (33M params,
//! 384-dim, CPU-only). Weights are downloaded on first use to fastembed's
//! cache directory and reused across runs.
//!
//! No behavior change in this commit — this is the substrate. Capability
//! discovery and tool gating wire it up in subsequent commits.

use anyhow::Result;
use octolib::{EmbeddingProvider, EmbeddingProviderType, InputType};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{OnceLock, RwLock};
use tokio::sync::Mutex as TokioMutex;

/// Hardcoded internal embedding model.
///
/// `muvon/octomind-embed` is a BGE-small-en-v1.5 fine-tune trained on the
/// octomind-tap capability triggers with paraphrase + hard-negative
/// augmentation (see `octomind-tap/model/`). 33M params, 384-dim, same
/// size/latency as base BGE-small but sharpened on the capability-routing
/// task: confusable clusters (shell vs programming-rust, etc.) clear the
/// margin gate where the base model abstains.
///
/// Loaded via octolib's HuggingFace provider — downloads ONNX weights from
/// `https://huggingface.co/<MODEL_NAME>` to the standard HF cache on first
/// use and reuses them thereafter.
const MODEL_NAME: &str = "muvon/octomind-embed";

/// Embedding dimension. BGE-small family is 384.
pub const EMBED_DIM: usize = 384;

static PROVIDER: OnceLock<Box<dyn EmbeddingProvider>> = OnceLock::new();
// Serialize provider init across all callers — `#[tokio::test]` creates
// a separate runtime per test, and `tokio::sync::OnceCell` does not
// reliably gate concurrent init across runtimes (multiple tests can race
// the same hf_hub cache file, corrupting the partial download and yielding
// "Could not find model weights" for late-comers). std `OnceLock` is
// process-global, and the tokio `Mutex` lets the slow async init run
// inside `.await`. After init, callers take only the lock-free fast path.
static INIT_LOCK: TokioMutex<()> = TokioMutex::const_new(());
static CACHE: OnceLock<RwLock<HashMap<u64, Vec<f32>>>> = OnceLock::new();

fn cache() -> &'static RwLock<HashMap<u64, Vec<f32>>> {
	CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn cache_key(text: &str) -> u64 {
	let mut h = std::collections::hash_map::DefaultHasher::new();
	text.hash(&mut h);
	h.finish()
}

async fn provider() -> Result<&'static (dyn EmbeddingProvider + 'static)> {
	// Fast path: already initialized, lock-free atomic read.
	if let Some(p) = PROVIDER.get() {
		return Ok(p.as_ref());
	}
	// Slow path: serialize the actual download/load so concurrent tasks
	// don't race the hf_hub cache. Re-check after acquiring the lock — a
	// peer task may have completed init while we were waiting.
	let _guard = INIT_LOCK.lock().await;
	if let Some(p) = PROVIDER.get() {
		return Ok(p.as_ref());
	}
	let provider_type = EmbeddingProviderType::HuggingFace;
	let new_p = octolib::create_embedding_provider_from_parts(&provider_type, MODEL_NAME).await?;
	// `set` returns Err only if some other task slipped in between our
	// check and set — in that case use whichever pointer won.
	let _ = PROVIDER.set(new_p);
	Ok(PROVIDER.get().expect("PROVIDER set above").as_ref())
}

/// Kick off model initialization in the background so the first real
/// `embed()` / `embed_many()` call doesn't pay the download/load cost.
///
/// Spawns a tokio task that calls `provider()` once. If weights need to be
/// downloaded (~50MB on first ever run), that happens off the hot path.
/// If init fails (no network, restricted env), the failure is logged and
/// callers fall back to whatever path they implement (e.g. capability
/// discover falls back to keyword scoring).
///
/// Idempotent: subsequent calls observe the already-initialized singleton
/// and return immediately. Safe to call from multiple places — only the
/// first one actually triggers init.
pub fn warmup() {
	tokio::spawn(async move {
		match provider().await {
			Ok(_) => {
				crate::log_debug!("embeddings: model warmup complete");
			}
			Err(e) => {
				crate::log_debug!(
					"embeddings: warmup failed ({}) — features that need embeddings will fall back",
					e
				);
			}
		}
	});
}

/// Whether the embedding model is initialized and ready (no further
/// download/load cost). Useful for status UI; not required for correctness.
pub fn is_ready() -> bool {
	PROVIDER.get().is_some()
}

/// Embed a single text. Returns a cached vector if the same text was
/// embedded earlier in the same process.
pub async fn embed(text: &str) -> Result<Vec<f32>> {
	let key = cache_key(text);
	if let Some(v) = cache().read().unwrap().get(&key) {
		return Ok(v.clone());
	}
	let p = provider().await?;
	let v = p.generate_embedding(text).await?;
	cache().write().unwrap().insert(key, v.clone());
	Ok(v)
}

/// Embed many texts in one batch. Cached entries are returned without
/// re-running inference; uncached entries are batched together.
pub async fn embed_many(texts: &[String]) -> Result<Vec<Vec<f32>>> {
	let mut result: Vec<Option<Vec<f32>>> = Vec::with_capacity(texts.len());
	let mut to_compute: Vec<(usize, String)> = Vec::new();
	{
		let cache_r = cache().read().unwrap();
		for (i, t) in texts.iter().enumerate() {
			if let Some(v) = cache_r.get(&cache_key(t)) {
				result.push(Some(v.clone()));
			} else {
				result.push(None);
				to_compute.push((i, t.clone()));
			}
		}
	}

	if !to_compute.is_empty() {
		let p = provider().await?;
		let raw: Vec<String> = to_compute.iter().map(|(_, t)| t.clone()).collect();
		let computed = p
			.generate_embeddings_batch(raw, InputType::Document)
			.await?;
		let mut cache_w = cache().write().unwrap();
		for ((idx, text), vec) in to_compute.into_iter().zip(computed) {
			cache_w.insert(cache_key(&text), vec.clone());
			result[idx] = Some(vec);
		}
	}

	Ok(result.into_iter().flatten().collect())
}

/// Cosine similarity between two equal-length vectors.
/// Returns 0.0 if lengths differ or either vector is zero.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
	if a.len() != b.len() || a.is_empty() {
		return 0.0;
	}
	let mut dot = 0.0_f32;
	let mut na = 0.0_f32;
	let mut nb = 0.0_f32;
	for (x, y) in a.iter().zip(b.iter()) {
		dot += x * y;
		na += x * x;
		nb += y * y;
	}
	let denom = na.sqrt() * nb.sqrt();
	if denom == 0.0 {
		0.0
	} else {
		dot / denom
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn cosine_identical_vectors_one() {
		let v = vec![0.1_f32, 0.2, 0.3, 0.4];
		assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
	}

	#[test]
	fn cosine_orthogonal_zero() {
		let a = vec![1.0_f32, 0.0];
		let b = vec![0.0_f32, 1.0];
		assert!(cosine(&a, &b).abs() < 1e-6);
	}

	#[test]
	fn cosine_mismatched_lengths_zero() {
		let a = vec![1.0_f32, 2.0];
		let b = vec![1.0_f32];
		assert_eq!(cosine(&a, &b), 0.0);
	}

	#[test]
	fn cosine_empty_zero() {
		let a: Vec<f32> = vec![];
		let b: Vec<f32> = vec![];
		assert_eq!(cosine(&a, &b), 0.0);
	}

	#[test]
	fn cache_keys_deterministic() {
		let k1 = cache_key("hello");
		let k2 = cache_key("hello");
		let k3 = cache_key("world");
		assert_eq!(k1, k2);
		assert_ne!(k1, k3);
	}

	/// End-to-end smoke test: actually loads `muvon/octomind-embed`
	/// (downloads safetensors from HuggingFace on first run, fast on
	/// subsequent runs) and verifies that `embed()` returns the expected
	/// dimension and that the cache returns the same vector on a repeat call.
	#[tokio::test]
	#[serial_test::serial(embed_model)]
	async fn embed_smoke() {
		let v = embed("hello world").await.expect("embed should succeed");
		assert_eq!(v.len(), EMBED_DIM);
		// Cache hit on second call — must return the exact same vector.
		let v2 = embed("hello world").await.unwrap();
		assert_eq!(v, v2);
		// Different text should produce a different vector.
		let v3 = embed("entirely different content").await.unwrap();
		assert_ne!(v, v3);
	}

	#[tokio::test]
	#[serial_test::serial(embed_model)]
	async fn embed_many_smoke() {
		let texts = vec![
			"query a postgres database for slow queries".to_string(),
			"search the web for recent news".to_string(),
			"read the contents of a local file".to_string(),
		];
		let vecs = embed_many(&texts).await.expect("embed_many should succeed");
		assert_eq!(vecs.len(), texts.len());
		for v in &vecs {
			assert_eq!(v.len(), EMBED_DIM);
		}
		// Different prompts should produce different embeddings.
		assert_ne!(vecs[0], vecs[1]);
		assert_ne!(vecs[1], vecs[2]);
		// Cosine should rank: same > different.
		let same_q = embed("query a postgres database for slow queries")
			.await
			.unwrap();
		let same_score = cosine(&same_q, &vecs[0]);
		let diff_score = cosine(&same_q, &vecs[1]);
		assert!(
			same_score > diff_score,
			"cosine should rank identical text higher than unrelated text (same={same_score:.3}, diff={diff_score:.3})"
		);
	}
}
