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
//! Wraps octolib's HuggingFace provider (candle backend, gated behind octolib's
//! `huggingface` feature) with a process-global model singleton and an in-memory
//! cache. Used by capability discovery and tool gating to score natural-language
//! intent against tool/capability descriptions.
//!
//! The model identity is an implementation detail. Users do not configure it
//! and cannot change it: `muvon/octomind-embed`, an all-MiniLM-L6-v2 fine-tune
//! (22M params, 384-dim, CPU-only). Weights are downloaded on first use to the
//! HuggingFace cache directory and reused across runs.
//!
//! No behavior change in this commit — this is the substrate. Capability
//! discovery and tool gating wire it up in subsequent commits.

use anyhow::Result;
use octolib::{EmbeddingProvider, EmbeddingProviderType, InputType};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::{Mutex, OnceLock, RwLock};
use tokenizers::Tokenizer;
use tokio::sync::Mutex as TokioMutex;

/// Hardcoded internal embedding model.
///
/// `muvon/octomind-embed` is an all-MiniLM-L6-v2 fine-tune trained on the
/// octomind-tap capability triggers with paraphrase + hard-negative
/// augmentation (see `octomind-tap/model/`). 22M params, 384-dim, same
/// size/latency as base MiniLM-L6 but sharpened on the capability-routing
/// task: confusable clusters (shell vs programming-rust, etc.) clear the
/// margin gate where the base model abstains.
///
/// MiniLM-L6 is a symmetric sentence-transformer: trained WITHOUT query/document
/// instruction prefixes and capped at 256 tokens. Embed both sides bare
/// (`InputType::None`) and keep inputs under the cap.
///
/// Loaded via octolib's HuggingFace (candle) provider — downloads weights from
/// `https://huggingface.co/<MODEL_NAME>` to the standard HF cache on first
/// use and reuses them thereafter.
const MODEL_NAME: &str = "muvon/octomind-embed";

/// Embedding dimension. MiniLM-L6 is 384.
pub const EMBED_DIM: usize = 384;

/// MiniLM-L6's input window in tokens — its sentence-transformers training cap.
/// The model-exact budget: the candle backend errors past the 512-position
/// ceiling and quality degrades past the 256 trained window. Enforced precisely
/// via the model's own tokenizer (`chunk_to_token_limit`). A model fact, not
/// config: the model is fixed, so its cap is too.
pub const EMBED_MAX_INPUT_TOKENS: usize = 256;

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

/// On-disk cache **file-format** version. Bump this only when the cache
/// *layout* below changes in code (fields added/reordered, encoding changed)
/// so old files are rejected instead of misparsed. This is orthogonal to the
/// *model*: a weights change is caught separately by the HF commit SHA stored
/// in the header (`model_revision`). OEC2 = the layout that carries that SHA.
const CACHE_MAGIC: &[u8; 4] = b"OEC2";

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

/// Content fingerprint of the currently-loaded weights: the HF commit SHA
/// that hf_hub resolved for `MODEL_NAME`, read from its ref file
/// (`<hf_home>/models--<org>--<name>/refs/main`).
///
/// This is what makes the trigger cache self-invalidate on a *same-name*
/// model swap. `disk_cache_path()` keys the file by MODEL_NAME, so it only
/// notices a *renamed* retrain; when we re-publish new weights under the SAME
/// repo, hf_hub fetches the new commit and updates `refs/main`, so the SHA
/// here changes and `load_disk_cache` drops the now-stale vectors.
///
/// Returns "" when unresolvable (offline, ref file absent, layout change); in
/// that case the cache falls back to name + dim validation only.
fn model_revision() -> String {
	for dir in model_repo_dirs() {
		if let Ok(s) = std::fs::read_to_string(dir.join("refs").join("main")) {
			let s = s.trim().to_string();
			if !s.is_empty() {
				return s;
			}
		}
	}
	String::new()
}

/// Candidate `models--<org>--<name>` cache directories, ordered by likelihood.
/// hf_hub (which octolib wraps) resolves its cache from different roots
/// depending on env, and octolib's own downloads use a different on-disk layout
/// than a model pre-fetched into the standard hub by an external tool. Probing
/// all of them makes the SHA and snapshot-file lookups robust instead of
/// guessing a single path that silently returns "" — and thereby disables cache
/// invalidation — whenever the weights happen to live elsewhere.
fn model_repo_dirs() -> Vec<std::path::PathBuf> {
	let repo = format!("models--{}", MODEL_NAME.replace('/', "--"));
	let mut roots: Vec<std::path::PathBuf> = Vec::new();
	if let Ok(octo) = octolib::storage::get_huggingface_cache_dir() {
		// octolib's own downloads land directly here: <octo>/models--...
		roots.push(octo.clone());
		// hf_hub nests repos under <HF_HOME>/hub when HF_HOME=<octo>.
		roots.push(octo.join("hub"));
		// Standard hub (<cache>/huggingface/hub) — octo is <cache>/octolib/huggingface.
		if let Some(cache) = octo.parent().and_then(|p| p.parent()) {
			roots.push(cache.join("huggingface").join("hub"));
		}
	}
	if let Ok(c) = std::env::var("HF_HUB_CACHE") {
		roots.push(std::path::PathBuf::from(c));
	}
	if let Ok(h) = std::env::var("HF_HOME") {
		roots.push(std::path::PathBuf::from(h).join("hub"));
	}
	roots.into_iter().map(|r| r.join(&repo)).collect()
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

	// Model content fingerprint. If we can resolve the current weights' commit
	// SHA and it differs from the one that produced these vectors, the model
	// was swapped under the same name — drop the stale cache and re-embed.
	let rev_len = read_u32(&mut r)? as usize;
	let mut rev_bytes = vec![0u8; rev_len];
	r.read_exact(&mut rev_bytes)?;
	let cached_rev = std::str::from_utf8(&rev_bytes)?;
	let current_rev = model_revision();
	if !current_rev.is_empty() && cached_rev != current_rev {
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
		// Model content fingerprint (HF commit SHA) — lets the cache
		// self-invalidate when new weights are published under the same name.
		let rev_bytes = model_revision();
		let rev_bytes = rev_bytes.as_bytes();
		w.write_all(&(rev_bytes.len() as u32).to_le_bytes())?;
		w.write_all(rev_bytes)?;
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
	let (v, _usage) = p.generate_embedding(text).await?;
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
		// Dedup identical inputs: an overlapping/repeated text is embedded ONCE
		// and fanned out to every position that needs it. Embedding is a pure
		// function of text — two equal texts must map to the same vector — so
		// computing each occurrence separately only wastes inference.
		let mut unique: Vec<String> = Vec::new();
		let mut seen = std::collections::HashSet::new();
		for (_, t) in &to_compute {
			if seen.insert(cache_key(t)) {
				unique.push(t.clone());
			}
		}

		let p = provider().await?;
		// MiniLM-L6 is symmetric — embed bare, no query/document prefix. The
		// query side (`embed`) is already prefix-free; keep both consistent.
		let (computed, _usage) = p
			.generate_embeddings_batch(unique.clone(), InputType::None)
			.await?;
		{
			let mut cache_w = cache().write().unwrap();
			for (text, vec) in unique.into_iter().zip(computed) {
				cache_w.insert(cache_key(&text), vec);
			}
			// Fill every slot from the now-populated cache — repeated texts
			// resolve to the same shared vector by key, so the output stays
			// 1-to-1 with the input by position.
			for (idx, text) in to_compute {
				result[idx] = cache_w.get(&cache_key(&text)).cloned();
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

/// The model's own WordPiece tokenizer, lazily loaded from the SAME HF cache
/// files octolib's candle provider uses — so our token counts match the model
/// exactly. `None` (logged) if the cache files aren't resolvable (offline,
/// layout change); callers then fall back to a char estimate.
fn tokenizer() -> Option<&'static Tokenizer> {
	static TOKENIZER: OnceLock<Option<Tokenizer>> = OnceLock::new();
	TOKENIZER
		.get_or_init(|| {
			let path = model_file_path("tokenizer.json")?;
			match Tokenizer::from_file(&path) {
				Ok(t) => Some(t),
				Err(e) => {
					crate::log_debug!(
						"embeddings: tokenizer load failed ({}); estimating tokens",
						e
					);
					None
				}
			}
		})
		.as_ref()
}

/// Path to a file inside the loaded model's HF snapshot
/// (`<hf_home>/models--<org>--<name>/snapshots/<sha>/<filename>`), or `None`
/// if the cache layout can't be resolved.
fn model_file_path(filename: &str) -> Option<std::path::PathBuf> {
	for dir in model_repo_dirs() {
		let Ok(sha) = std::fs::read_to_string(dir.join("refs").join("main")) else {
			continue;
		};
		let sha = sha.trim();
		if sha.is_empty() {
			continue;
		}
		let path = dir.join("snapshots").join(sha).join(filename);
		if path.exists() {
			return Some(path);
		}
	}
	None
}

/// Split `text` into chunks that each fit MiniLM-L6's token window, cutting at
/// exact token boundaries via the model's own tokenizer (reserving 2 tokens for
/// the [CLS]/[SEP] the model adds at embed time). Text within the window returns
/// as one chunk; nothing is dropped. Falls back to a char window only if the
/// tokenizer can't be loaded.
pub fn chunk_to_token_limit(text: &str, max_tokens: usize) -> Vec<String> {
	let trimmed = text.trim();
	if trimmed.is_empty() {
		return Vec::new();
	}
	let content_cap = max_tokens.saturating_sub(2).max(1);
	let fallback_chars = content_cap.saturating_mul(4).max(1);
	let Some(tok) = tokenizer() else {
		return chunk_by_chars(trimmed, fallback_chars);
	};
	let Ok(enc) = tok.encode(trimmed, false) else {
		return chunk_by_chars(trimmed, fallback_chars);
	};
	let n = enc.len();
	if n <= content_cap {
		return vec![trimmed.to_string()];
	}
	// `offsets[i]` is the byte span of token i in `trimmed`; cut at the start
	// byte of each window's first token so chunks tile the text with no gap.
	let offsets = enc.get_offsets();
	let mut chunks = Vec::new();
	let mut start = 0usize;
	while start < n {
		let end = (start + content_cap).min(n);
		let start_byte = offsets[start].0;
		let end_byte = if end < n {
			offsets[end].0
		} else {
			trimmed.len()
		};
		let piece = trimmed
			.get(start_byte..end_byte)
			.map(str::trim)
			.unwrap_or("");
		if !piece.is_empty() {
			chunks.push(piece.to_string());
		}
		start = end;
	}
	if chunks.is_empty() {
		return chunk_by_chars(trimmed, fallback_chars);
	}
	chunks
}

/// Simple char-window splitter — the tokenizer-unavailable fallback. A text
/// within budget returns as one chunk; nothing is dropped.
fn chunk_by_chars(text: &str, max_chars: usize) -> Vec<String> {
	let trimmed = text.trim();
	let chars: Vec<char> = trimmed.chars().collect();
	if chars.is_empty() {
		return Vec::new();
	}
	if chars.len() <= max_chars {
		return vec![trimmed.to_string()];
	}
	chars
		.chunks(max_chars)
		.map(|c| c.iter().collect())
		.collect()
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
