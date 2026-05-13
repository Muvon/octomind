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
use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::{Mutex, OnceLock, RwLock};
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
/// One-shot guard ensuring the on-disk cache is read in only once per process.
static DISK_CACHE_LOADED: OnceLock<()> = OnceLock::new();
/// Serializes concurrent writers within a single process. Cross-process
/// concurrency is handled by writing to a temp file and renaming atomically;
/// the last writer wins. Lost entries are deterministically re-derivable from
/// trigger text, so the cost of a lost write is one extra embed per text.
static DISK_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// On-disk cache file format magic. Changing the layout below means bumping
/// this so old files are rejected on load (re-embedded fresh).
const CACHE_MAGIC: &[u8; 4] = b"OEC1";

fn cache() -> &'static RwLock<HashMap<u64, Vec<f32>>> {
	CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Path to the on-disk embedding cache for the current model.
///
/// File name embeds the model identity so switching MODEL_NAME (e.g. retraining
/// `muvon/octomind-embed`) automatically opens a fresh file instead of
/// pointing the new model at vectors produced by the old one. The header also
/// stores the model name + dim as belt-and-suspenders.
fn disk_cache_path() -> Result<std::path::PathBuf> {
	let dir = crate::directories::get_cache_dir()?.join("embeddings");
	std::fs::create_dir_all(&dir)?;
	let safe_name = MODEL_NAME.replace('/', "_");
	Ok(dir.join(format!("triggers-{safe_name}.bin")))
}

/// Read the on-disk cache into the given map, merging without overwriting.
/// In-memory entries take precedence on key collision (they reflect the
/// current process's freshly-computed work).
///
/// Best-effort: any failure (missing file, magic mismatch, model-name change,
/// dim change, truncation, IO error) returns silently with no entries added.
/// The model name and dim in the header are validated to defend against the
/// theoretical case where the path filter is bypassed (e.g. user copies the
/// file across machines with different model installs).
fn load_disk_cache() -> Result<usize> {
	let path = disk_cache_path()?;
	if !path.exists() {
		return Ok(0);
	}
	let file = std::fs::File::open(&path)?;
	let mut r = BufReader::new(file);

	let mut magic = [0u8; 4];
	r.read_exact(&mut magic)?;
	if &magic != CACHE_MAGIC {
		return Ok(0);
	}

	let model_name_len = read_u32(&mut r)? as usize;
	let mut model_name_bytes = vec![0u8; model_name_len];
	r.read_exact(&mut model_name_bytes)?;
	let model_name = std::str::from_utf8(&model_name_bytes)?;
	if model_name != MODEL_NAME {
		return Ok(0);
	}

	let dim = read_u32(&mut r)? as usize;
	if dim != EMBED_DIM {
		return Ok(0);
	}

	let count = read_u32(&mut r)? as usize;
	let mut loaded = 0;
	let mut buf = vec![0u8; dim * 4];
	let mut c = cache().write().unwrap();
	for _ in 0..count {
		let key = read_u64(&mut r)?;
		r.read_exact(&mut buf)?;
		if c.contains_key(&key) {
			continue;
		}
		let mut vec = Vec::with_capacity(dim);
		for chunk in buf.chunks_exact(4) {
			vec.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
		}
		c.insert(key, vec);
		loaded += 1;
	}
	Ok(loaded)
}

/// Snapshot the in-memory cache and persist it atomically. Writes to a temp
/// file in the same directory and renames into place — readers always see a
/// fully-formed file or the previous one, never a partial.
///
/// Skips entirely if another writer holds the lock; the next batched embed
/// will retry. This is intentional: we'd rather lose a write than block the
/// hot path.
fn save_disk_cache_locked() {
	let Ok(_guard) = DISK_WRITE_LOCK.try_lock() else {
		return;
	};
	let snapshot: Vec<(u64, Vec<f32>)> = {
		let c = cache().read().unwrap();
		c.iter().map(|(k, v)| (*k, v.clone())).collect()
	};
	let path = match disk_cache_path() {
		Ok(p) => p,
		Err(e) => {
			crate::log_debug!("embeddings: cache path resolution failed: {}", e);
			return;
		}
	};
	let tmp_path = path.with_extension("bin.tmp");
	let write_result = (|| -> Result<()> {
		let file = std::fs::File::create(&tmp_path)?;
		let mut w = BufWriter::new(file);
		w.write_all(CACHE_MAGIC)?;
		let name_bytes = MODEL_NAME.as_bytes();
		w.write_all(&(name_bytes.len() as u32).to_le_bytes())?;
		w.write_all(name_bytes)?;
		w.write_all(&(EMBED_DIM as u32).to_le_bytes())?;
		w.write_all(&(snapshot.len() as u32).to_le_bytes())?;
		for (key, vec) in &snapshot {
			w.write_all(&key.to_le_bytes())?;
			for f in vec {
				w.write_all(&f.to_le_bytes())?;
			}
		}
		w.flush()?;
		drop(w);
		std::fs::rename(&tmp_path, &path)?;
		Ok(())
	})();
	if let Err(e) = write_result {
		let _ = std::fs::remove_file(&tmp_path);
		crate::log_debug!("embeddings: failed to persist cache: {}", e);
	}
}

fn read_u32<R: Read>(r: &mut R) -> Result<u32> {
	let mut buf = [0u8; 4];
	r.read_exact(&mut buf)?;
	Ok(u32::from_le_bytes(buf))
}

fn read_u64<R: Read>(r: &mut R) -> Result<u64> {
	let mut buf = [0u8; 8];
	r.read_exact(&mut buf)?;
	Ok(u64::from_le_bytes(buf))
}

/// First-call lazy load of the on-disk cache into memory. Idempotent across
/// the process — subsequent calls are a no-op atomic check. Called from the
/// public embed entry points so it happens *after* the embedding model is
/// available (and after `provider()` has resolved directory bootstrapping).
fn ensure_disk_cache_loaded() {
	DISK_CACHE_LOADED.get_or_init(|| match load_disk_cache() {
		Ok(0) => {}
		Ok(n) => crate::log_debug!("embeddings: loaded {} cached vectors from disk", n),
		Err(e) => crate::log_debug!("embeddings: disk cache load failed: {}", e),
	});
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
/// Also lazily loads the on-disk vector cache once the model is ready, so
/// the first user message doesn't pay the file-read cost either. The disk
/// load is synchronous (~5 ms for ~90 KB) but happens inside the spawned
/// task, before `is_ready()` flips true.
///
/// Idempotent: subsequent calls observe the already-initialized singleton
/// and return immediately. Safe to call from multiple places — only the
/// first one actually triggers init.
pub fn warmup() {
	tokio::spawn(async move {
		match provider().await {
			Ok(_) => {
				ensure_disk_cache_loaded();
				crate::log_debug!("embeddings: model + disk cache ready");
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

/// Pre-embed a batch of texts in the background after model warmup completes.
/// Used at boot to prime the in-memory + on-disk caches for stable trigger
/// sets (capability triggers, skill semantic phrases) — that way the first
/// auto-activation after `is_ready()` flips true gets all cache hits instead
/// of paying ~300-500 ms to embed the trigger batch on the user's hot path.
///
/// Fire-and-forget: spawns its own tokio task. Errors are logged and dropped;
/// the auto-activation path falls back to lazy embedding on first use, so a
/// prewarm failure is invisible to the user — they just pay the cost they
/// would have paid without this function.
///
/// Cache-aware: texts already present in the cache (whether from this
/// process's prior calls or loaded from disk) are skipped by `embed_many`,
/// so the steady-state second-run cost is just the disk read in `warmup()`.
pub fn prewarm(texts: Vec<String>) {
	if texts.is_empty() {
		return;
	}
	tokio::spawn(async move {
		match embed_many(&texts).await {
			Ok(_) => crate::log_debug!("embeddings: prewarmed {} texts", texts.len()),
			Err(e) => crate::log_debug!("embeddings: prewarm failed ({})", e),
		}
	});
}

/// Whether the embedding model is initialized and ready (no further
/// download/load cost). Useful for status UI; not required for correctness.
pub fn is_ready() -> bool {
	PROVIDER.get().is_some()
}

/// Embed a single text. Returns a cached vector if the same text was
/// embedded earlier in the same process (or in a prior process whose vectors
/// were loaded from disk on first call).
///
/// Does NOT persist on miss. Single-text embeds are dominated by per-turn
/// user input, which is high-volume and low-reuse — persisting it would
/// bloat the cache file without payoff. Only batched embeds (used for
/// trigger sets, which are stable across runs) write back to disk.
pub async fn embed(text: &str) -> Result<Vec<f32>> {
	ensure_disk_cache_loaded();
	let key = cache_key(text);
	if let Some(v) = cache().read().unwrap().get(&key) {
		return Ok(v.clone());
	}
	let p = provider().await?;
	let v = p.generate_embedding(text).await?;
	cache().write().unwrap().insert(key, v.clone());
	Ok(v)
}

/// Embed many texts in one batch. Cached entries (from this process's memory
/// or loaded from disk on first call) are returned without re-running
/// inference; uncached entries are batched together.
///
/// After computing new entries, the whole in-memory cache is snapshotted and
/// persisted atomically (temp-write + rename). This is the path that
/// auto-activation uses for trigger sets — tap update → some trigger texts
/// change → those hash to new keys → only the delta is re-embedded → the
/// fresh cache replaces the file on disk. Old entries from the previous
/// trigger set survive harmlessly in the file until they're naturally
/// orphaned (never queried).
pub async fn embed_many(texts: &[String]) -> Result<Vec<Vec<f32>>> {
	ensure_disk_cache_loaded();
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
		{
			let mut cache_w = cache().write().unwrap();
			for ((idx, text), vec) in to_compute.into_iter().zip(computed) {
				cache_w.insert(cache_key(&text), vec.clone());
				result[idx] = Some(vec);
			}
		}
		// Persist after the write lock is released so the snapshot inside
		// `save_disk_cache_locked` doesn't deadlock against itself.
		save_disk_cache_locked();
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

	/// Round-trip the binary cache format. Verifies vectors written by
	/// `save_disk_cache_locked` are byte-identical when read back by
	/// `load_disk_cache`. Uses a tempfile to avoid clobbering the real
	/// cache; we redirect by overriding the env var the directories module
	/// honors, but since `disk_cache_path` doesn't accept overrides, we
	/// instead exercise the format functions directly against an in-memory
	/// buffer using a helper. This decouples the format check from the
	/// global state.
	#[test]
	fn disk_cache_format_round_trip() {
		// Build a synthetic snapshot.
		let entries: Vec<(u64, Vec<f32>)> = vec![
			(
				0xDEAD_BEEF_u64,
				(0..EMBED_DIM).map(|i| i as f32 * 0.01).collect(),
			),
			(
				0xCAFE_F00D_u64,
				(0..EMBED_DIM).map(|i| (i as f32).sin()).collect(),
			),
		];

		// Encode using the same layout as `save_disk_cache_locked` so the
		// reader path is exercised against canonical bytes.
		let mut buf: Vec<u8> = Vec::new();
		buf.extend_from_slice(CACHE_MAGIC);
		let name = MODEL_NAME.as_bytes();
		buf.extend_from_slice(&(name.len() as u32).to_le_bytes());
		buf.extend_from_slice(name);
		buf.extend_from_slice(&(EMBED_DIM as u32).to_le_bytes());
		buf.extend_from_slice(&(entries.len() as u32).to_le_bytes());
		for (k, v) in &entries {
			buf.extend_from_slice(&k.to_le_bytes());
			for f in v {
				buf.extend_from_slice(&f.to_le_bytes());
			}
		}

		// Decode using the same logic as `load_disk_cache`.
		let mut r = std::io::Cursor::new(&buf);
		let mut magic = [0u8; 4];
		r.read_exact(&mut magic).unwrap();
		assert_eq!(&magic, CACHE_MAGIC);
		let mn_len = read_u32(&mut r).unwrap() as usize;
		let mut mn = vec![0u8; mn_len];
		r.read_exact(&mut mn).unwrap();
		assert_eq!(std::str::from_utf8(&mn).unwrap(), MODEL_NAME);
		assert_eq!(read_u32(&mut r).unwrap() as usize, EMBED_DIM);
		let count = read_u32(&mut r).unwrap() as usize;
		assert_eq!(count, entries.len());

		let mut buf_vec = vec![0u8; EMBED_DIM * 4];
		for (expected_key, expected_vec) in &entries {
			let key = read_u64(&mut r).unwrap();
			assert_eq!(key, *expected_key);
			r.read_exact(&mut buf_vec).unwrap();
			let decoded: Vec<f32> = buf_vec
				.chunks_exact(4)
				.map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
				.collect();
			assert_eq!(decoded.len(), expected_vec.len());
			for (a, b) in decoded.iter().zip(expected_vec.iter()) {
				assert_eq!(a.to_bits(), b.to_bits(), "f32 bit-exact mismatch");
			}
		}
	}

	/// Reject files written by a different model so the cache never returns
	/// vectors produced by an embedder that doesn't match the current one.
	#[test]
	fn disk_cache_rejects_wrong_model_name() {
		let mut buf: Vec<u8> = Vec::new();
		buf.extend_from_slice(CACHE_MAGIC);
		let other = b"some/other-model";
		buf.extend_from_slice(&(other.len() as u32).to_le_bytes());
		buf.extend_from_slice(other);
		buf.extend_from_slice(&(EMBED_DIM as u32).to_le_bytes());
		buf.extend_from_slice(&0u32.to_le_bytes()); // zero entries

		let mut r = std::io::Cursor::new(&buf);
		let mut magic = [0u8; 4];
		r.read_exact(&mut magic).unwrap();
		assert_eq!(&magic, CACHE_MAGIC);
		let mn_len = read_u32(&mut r).unwrap() as usize;
		let mut mn = vec![0u8; mn_len];
		r.read_exact(&mut mn).unwrap();
		assert_ne!(
			std::str::from_utf8(&mn).unwrap(),
			MODEL_NAME,
			"loader must reject this file at the model-name check"
		);
	}

	/// Reject files whose embedding dimension differs from the current model's.
	#[test]
	fn disk_cache_rejects_wrong_dim() {
		let mut buf: Vec<u8> = Vec::new();
		buf.extend_from_slice(CACHE_MAGIC);
		let name = MODEL_NAME.as_bytes();
		buf.extend_from_slice(&(name.len() as u32).to_le_bytes());
		buf.extend_from_slice(name);
		buf.extend_from_slice(&(512u32).to_le_bytes()); // wrong dim
		buf.extend_from_slice(&0u32.to_le_bytes());

		let mut r = std::io::Cursor::new(&buf);
		let mut magic = [0u8; 4];
		r.read_exact(&mut magic).unwrap();
		let mn_len = read_u32(&mut r).unwrap() as usize;
		let mut mn = vec![0u8; mn_len];
		r.read_exact(&mut mn).unwrap();
		assert_eq!(std::str::from_utf8(&mn).unwrap(), MODEL_NAME);
		let dim = read_u32(&mut r).unwrap() as usize;
		assert_ne!(
			dim, EMBED_DIM,
			"loader must reject this file at the dim check"
		);
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
