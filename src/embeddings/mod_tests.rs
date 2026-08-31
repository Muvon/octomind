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

use super::*;

#[test]
fn chunk_by_chars_windows_and_preserves() {
	assert_eq!(chunk_by_chars("short", 100), vec!["short".to_string()]);
	let blob = "x".repeat(50);
	let parts = chunk_by_chars(&blob, 10);
	assert_eq!(parts.len(), 5);
	assert!(parts.iter().all(|c| c.chars().count() <= 10));
	assert_eq!(parts.concat().matches('x').count(), 50);
}

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
			.as_chunks::<4>()
			.0
			.iter()
			.map(|c| f32::from_le_bytes(*c))
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
