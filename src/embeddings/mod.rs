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
use tokio::sync::OnceCell;

/// Hardcoded internal model. Small + fast + good enough for description matching.
const MODEL_NAME: &str = "BAAI/bge-small-en-v1.5";

/// Embedding dimension for the default model.
pub const EMBED_DIM: usize = 384;

static PROVIDER: OnceCell<Box<dyn EmbeddingProvider>> = OnceCell::const_new();
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
	let p = PROVIDER
		.get_or_try_init(|| async {
			let provider_type = EmbeddingProviderType::FastEmbed;
			octolib::create_embedding_provider_from_parts(&provider_type, MODEL_NAME).await
		})
		.await?;
	Ok(p.as_ref())
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

	/// Smoke test that actually loads the model and downloads weights on
	/// first use. Run manually with:
	/// `cargo test embeddings::tests::embed_smoke -- --ignored`
	#[tokio::test]
	#[ignore]
	async fn embed_smoke() {
		let v = embed("hello world").await.expect("embed should succeed");
		assert_eq!(v.len(), EMBED_DIM);
		// Cache hit on second call.
		let v2 = embed("hello world").await.unwrap();
		assert_eq!(v, v2);
	}
}
