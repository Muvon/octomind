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

//! Cross-encoder reranker — second-stage precision pass after bi-encoder retrieval.
//!
//! The bi-encoder (see `super::embed`) scores every capability fast but
//! coarsely. Confusable clusters (shell vs programming-rust, generic
//! "run"/"execute" overlap) frequently land within the margin gate.
//! This reranker reads `(intent, trigger)` as a single sequence with full
//! self-attention — it can directly weight token overlap and produce a
//! sharper score distribution.
//!
//! Model: `muvon/octomind-rerank` — Jina-reranker-v1-turbo-en fine-tune
//! trained on the same hard-negative triplets the bi-encoder saw, with
//! `BinaryCrossEntropyLoss` over (anchor, positive) and (anchor, negative)
//! pairs. 33M params, English-only, ONNX, fastembed-compatible.
//!
//! Provider: octolib's HuggingFace reranker (uses `hf_hub` to download).

use anyhow::Result;
use octolib::reranker::{create_rerank_provider_from_parts, RerankProvider, RerankProviderType};
use tokio::sync::OnceCell;

/// Hardcoded internal reranker model. Fine-tuned for the capability-
/// routing task and uploaded to `muvon/octomind-rerank` on HuggingFace.
/// Same lineage as the embedding model — both ship together.
const MODEL_NAME: &str = "muvon/octomind-rerank";

static PROVIDER: OnceCell<Box<dyn RerankProvider>> = OnceCell::const_new();

async fn provider() -> Result<&'static (dyn RerankProvider + 'static)> {
	let p = PROVIDER
		.get_or_try_init(|| async {
			create_rerank_provider_from_parts(&RerankProviderType::HuggingFace, MODEL_NAME).await
		})
		.await?;
	Ok(p.as_ref())
}

/// Whether the reranker is initialized and ready. Mirrors `embeddings::is_ready`.
/// Used by callers to fall back to the bi-encoder gate when the reranker
/// is still warming up or failed to initialize.
pub fn is_ready() -> bool {
	PROVIDER.initialized()
}

/// Warm up the reranker model in the background so the first real
/// `rerank()` call doesn't pay the download/load cost. Called from
/// startup alongside `embeddings::warmup`. Idempotent; failures are
/// logged and capability auto-activation falls back to cosine-only.
pub fn warmup() {
	tokio::spawn(async move {
		match provider().await {
			Ok(_) => {
				crate::log_debug!("reranker: model warmup complete");
			}
			Err(e) => {
				crate::log_debug!(
					"reranker: warmup failed ({}) — capability gate will fall back to bi-encoder only",
					e
				);
			}
		}
	});
}

/// Score (`query`, `document`) pairs with the cross-encoder. Returns one
/// f32 score per document in input order. Higher = more relevant.
///
/// The model returns scores in roughly [0, 1] after sigmoid (Jina-style
/// rerankers normalize to relevance probability). Callers compare these
/// scores against `RERANK_THRESHOLD`/`RERANK_MARGIN` in the capability gate.
pub async fn rerank(query: &str, documents: Vec<String>) -> Result<Vec<f32>> {
	if documents.is_empty() {
		return Ok(Vec::new());
	}
	let p = provider().await?;
	let n = documents.len();
	let response = p.rerank(query, documents, None, true).await?;

	// `RerankResponse.results` contains `{index, score}`. We need to return
	// scores in INPUT order, not sorted order, so the caller can zip them
	// with their candidate list.
	let mut scores = vec![0.0_f32; n];
	for r in response.results {
		if r.index < n {
			scores[r.index] = r.relevance_score as f32;
		}
	}
	Ok(scores)
}
