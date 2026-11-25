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

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

use super::{project, Grant, Origin, Passage, Scope, Source, MAX_CHUNK_BYTES, MAX_EXCERPT_BYTES};
use crate::embeddings::{cosine, embed_transient, EMBED_DIM, MODEL_REVISION};
use crate::session::{
	persistence::{
		entry_kind, legacy_metadata, withdrawal_fingerprint, withdrawn_messages, SessionLog, WithdrawnMessages,
		TRUNCATION_POINT,
	},
	Message,
};

const CACHE_SCHEMA: u8 = 6;
const MAX_ARCHIVE_REFS: usize = 64;
const HOT_CHUNKS: usize = 512;
const OLD_INSTRUCTIONS: usize = 128;
const MAX_CACHE_FILES: usize = 16;
const MAX_CACHE_FILE_BYTES: u64 = 5 * 1024 * 1024;
const QUERY_SCAN_BYTES: u64 = 4 * 1024 * 1024;
const WORKER_SCAN_BYTES: u64 = 16 * 1024 * 1024;
const EMBED_BATCH: usize = 32;
const MIN_SIMILARITY: f32 = 0.25;
const RRF_K: f32 = 60.0;
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const WORKER_BATCH_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PARALLEL_SEARCHES: usize = 4;
const MAX_LEXICAL_WORKERS: usize = 2;
const MAX_SEMANTIC_WORKERS: usize = 1;
const MAX_QUEUE: usize = 8;

static SEARCH_SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(MAX_PARALLEL_SEARCHES)));
static WITHDRAWALS: LazyLock<Mutex<VecDeque<WithdrawalRecord>>> = LazyLock::new(Default::default);
static JOBS: LazyLock<Mutex<HashMap<String, Job>>> = LazyLock::new(Default::default);
static LEXICAL_WORKERS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(MAX_LEXICAL_WORKERS));
static SEMANTIC_WORKERS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(MAX_SEMANTIC_WORKERS));

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
struct Span {
	start: u64,
	end: u64,
	line: u32,
	part: u32,
}

#[derive(Debug, Deserialize, Serialize)]
struct Entry {
	span: Span,
	source: Origin,
	digest: u64,
	fingerprint: u64,
	terms: Vec<u64>,
	#[serde(default, skip_serializing_if = "Option::is_none", with = "vector_encoding")]
	vector: Option<Vec<f32>>,
}

mod vector_encoding {
	use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
	use serde::{de::Error, Deserialize, Deserializer, Serializer};

	pub(super) fn serialize<S: Serializer>(value: &Option<Vec<f32>>, serializer: S) -> Result<S::Ok, S::Error> {
		let Some(vector) = value else {
			return serializer.serialize_none();
		};
		let bytes: Vec<_> = vector.iter().flat_map(|value| value.to_le_bytes()).collect();
		serializer.serialize_str(&STANDARD_NO_PAD.encode(bytes))
	}

	pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<f32>>, D::Error> {
		let encoded = String::deserialize(deserializer)?;
		let bytes = STANDARD_NO_PAD.decode(encoded).map_err(D::Error::custom)?;
		if bytes.len() != super::EMBED_DIM * size_of::<f32>() {
			return Err(D::Error::custom("invalid conversation vector length"));
		}
		let vector: Vec<_> = bytes
			.as_chunks::<4>()
			.0
			.iter()
			.map(|bytes| f32::from_le_bytes(*bytes))
			.collect();
		if vector.iter().any(|value| !value.is_finite()) {
			return Err(D::Error::custom("non-finite conversation vector"));
		}
		Ok(Some(vector))
	}
}

#[derive(Debug, Deserialize, Serialize)]
struct Index {
	schema: u8,
	revision: String,
	through: u64,
	evicted: bool,
	archives: VecDeque<ArchiveRef>,
	entries: VecDeque<Entry>,
}

impl Default for Index {
	fn default() -> Self {
		Self {
			schema: CACHE_SCHEMA,
			revision: MODEL_REVISION.to_owned(),
			through: 0,
			evicted: false,
			archives: VecDeque::new(),
			entries: VecDeque::new(),
		}
	}
}

struct WithdrawalRecord {
	path: PathBuf,
	through: u64,
	withdrawn: Arc<WithdrawnMessages>,
}

fn withdrawals(source: &Source) -> Result<Arc<WithdrawnMessages>> {
	let path = source.path()?;
	let file = File::open(&path).context("opening delegation withdrawal log")?;
	file.lock_shared()?;
	let end = file.metadata()?.len();
	drop(file);
	let cached = WITHDRAWALS
		.lock()
		.expect("delegation withdrawals poisoned")
		.iter()
		.find(|record| record.path == path)
		.map(|record| (record.through, Arc::clone(&record.withdrawn)));
	if let Some((through, result)) = &cached {
		if *through == end {
			return Ok(Arc::clone(result));
		}
	}
	let (from, previous) = cached
		.filter(|(through, _)| *through < end)
		.unwrap_or_else(|| (0, Arc::new(WithdrawnMessages::default())));
	let mut log = SessionLog::open(&path)?.through(end).at_frame(from)?;
	let mut result = previous;
	while let Some(line) = log.next().transpose()? {
		if entry_kind(&line) == Some(TRUNCATION_POINT) {
			result = Arc::new(withdrawn_messages(&path, end)?);
			break;
		}
	}
	anyhow::ensure!(log.torn_at().is_none(), "delegation withdrawal log is incomplete");
	let mut cache = WITHDRAWALS.lock().expect("delegation withdrawals poisoned");
	if let Some(position) = cache.iter().position(|record| record.path == path) {
		if cache[position].through > end {
			return Ok(Arc::clone(&cache[position].withdrawn));
		}
		cache.remove(position);
	}
	if cache.len() == MAX_CACHE_FILES {
		cache.pop_front();
	}
	cache.push_back(WithdrawalRecord {
		path,
		through: end,
		withdrawn: Arc::clone(&result),
	});
	Ok(result)
}

fn cache_path(source: &Source) -> Result<PathBuf> {
	source.path()?;
	Ok(crate::directories::get_sessions_dir()?
		.join("delegation")
		.join("index")
		.join(format!("{CACHE_SCHEMA}-{MODEL_REVISION}-{}.json", source.session)))
}

fn load(path: &Path) -> Result<Option<Index>> {
	let file = match File::open(path) {
		Ok(file) => file,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
		Err(error) => return Err(error).context("opening delegation index"),
	};
	anyhow::ensure!(
		file.metadata()?.len() <= MAX_CACHE_FILE_BYTES,
		"delegation index exceeds the byte limit"
	);
	let index: Index = serde_json::from_reader(BufReader::new(file).take(MAX_CACHE_FILE_BYTES))?;
	if index.schema != CACHE_SCHEMA || index.revision != MODEL_REVISION {
		return Ok(None);
	}
	Ok(Some(index))
}

async fn save(path: PathBuf, mut index: Index) -> Result<()> {
	let dir = path
		.parent()
		.context("delegation index directory is missing")?
		.to_owned();
	tokio::fs::create_dir_all(&dir).await?;
	let lock = crate::local_inference::election_lock(&dir.join("index")).await?;
	tokio::task::spawn_blocking(move || {
		let mut files = Vec::new();
		for entry in std::fs::read_dir(&dir)? {
			let entry = entry?;
			if entry.path() != path && entry.path().extension().is_some_and(|ext| ext == "json") {
				files.push((entry.metadata()?.modified()?, entry.path()));
			}
		}
		files.sort_unstable();
		let remove = files.len().saturating_sub(MAX_CACHE_FILES - 1);
		for (_, file) in files.into_iter().take(remove) {
			std::fs::remove_file(file)?;
		}
		// Lexical progress and completed vectors can arrive in either order.
		if let Some(mut saved) = load(&path)? {
			if saved.through > index.through {
				std::mem::swap(&mut saved, &mut index);
			}
			let mut vectors: HashMap<_, _> = saved
				.entries
				.into_iter()
				.filter_map(|entry| entry.vector.map(|vector| (entry.digest, vector)))
				.collect();
			for entry in &mut index.entries {
				if entry.vector.is_none() {
					entry.vector = vectors.remove(&entry.digest);
				}
			}
		}
		crate::local_inference::publish(&path, &index)?;
		drop(lock);
		Ok(())
	})
	.await?
}

fn hash(value: impl Hash) -> u64 {
	let mut hasher = DefaultHasher::new();
	value.hash(&mut hasher);
	hasher.finish()
}

fn terms(text: &str) -> Vec<u64> {
	let lower = text.to_lowercase();
	let plain = lower.split(|ch: char| !ch.is_alphanumeric() && ch != '_');
	let identifiers = lower
		.split(|ch: char| !ch.is_alphanumeric() && !matches!(ch, '_' | '/' | '.' | '-' | ':'))
		.map(|word| word.trim_matches(['.', ':', '-']));
	let mut terms: Vec<_> = plain
		.chain(identifiers)
		.filter(|word| !word.is_empty())
		.map(hash)
		.collect();
	terms.sort_unstable();
	terms.dedup();
	terms
}

fn chunks(mut text: &str) -> impl Iterator<Item = &str> {
	std::iter::from_fn(move || {
		if text.is_empty() {
			return None;
		}
		let cap = text.floor_char_boundary(MAX_CHUNK_BYTES);
		let split = if cap == text.len() {
			cap
		} else {
			let tail = &text[..cap];
			tail.rfind(['\n', ' '])
				.filter(|&at| at > cap / 2)
				.map_or(cap, |at| at + 1)
		};
		let (chunk, rest) = text.split_at(split);
		text = rest;
		Some(chunk)
	})
}

#[derive(Deserialize)]
struct TranscriptMessage {
	role: String,
	content: String,
	timestamp: u64,
	name: Option<String>,
}

impl From<TranscriptMessage> for Message {
	fn from(row: TranscriptMessage) -> Self {
		Self {
			role: row.role,
			content: row.content,
			timestamp: row.timestamp,
			name: row.name,
			..Self::default()
		}
	}
}

fn read_message(raw: &str, start: u64) -> Result<Option<Message>> {
	if entry_kind(raw).is_some() || legacy_metadata(raw).is_some() || raw.is_empty() {
		return Ok(None);
	}
	let message: TranscriptMessage =
		serde_json::from_str(raw).with_context(|| format!("invalid transcript message at byte {start}"))?;
	Ok(Some(message.into()))
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct ArchiveRef {
	end: u64,
	name: String,
}

fn archive_name(message: &Message) -> Option<String> {
	use crate::session::chat::conversation_compression::COMPRESSION_MESSAGE_NAME;
	if message.name.as_deref() != Some(COMPRESSION_MESSAGE_NAME) {
		return None;
	}
	let (_, tail) = message.content.split_once("<archive path=\"")?;
	let (path, _) = tail.split_once("\">")?;
	let filename = Path::new(path).file_name()?.to_str()?;
	let id = filename.strip_prefix("comp_")?.strip_suffix(".jsonl")?;
	(!id.is_empty() && id.bytes().all(|ch| ch.is_ascii_digit())).then(|| filename.to_owned())
}

fn visit_archive(
	source: &Source,
	name: &str,
	bytes: u64,
	withdrawn: &WithdrawnMessages,
	mut visitor: impl FnMut(usize, u32, Origin, u64, &str),
) -> Result<(u64, bool)> {
	let id = name.strip_prefix("comp_").and_then(|name| name.strip_suffix(".jsonl"));
	anyhow::ensure!(
		id.is_some_and(|id| !id.is_empty() && id.bytes().all(|ch| ch.is_ascii_digit())),
		"invalid delegation archive name"
	);
	let path = crate::directories::get_sessions_dir()?
		.join("archive")
		.join(&source.session)
		.join(name);
	let file = match File::open(&path) {
		Ok(file) => file,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, false)),
		Err(error) => return Err(error).context("opening a delegated compression archive"),
	};
	let len = file.metadata()?.len();
	let mut reader = BufReader::new(file).take(bytes);
	let mut raw = String::new();
	let mut consumed = 0;
	let mut line = 0;
	while reader.read_line(&mut raw)? != 0 {
		consumed += raw.len() as u64;
		if !raw.ends_with('\n') {
			break;
		}
		line += 1;
		let message: TranscriptMessage =
			serde_json::from_str(&raw).context("invalid message in delegated compression archive")?;
		let message = message.into();
		if !withdrawn.contains_archived(&message) {
			if let Some((origin, content)) = project(&message, source.delegated) {
				for (part, excerpt) in (0..).zip(chunks(&content)) {
					if !excerpt.trim().is_empty() {
						visitor(
							line,
							part,
							origin,
							hash((message.timestamp, origin, part, excerpt)),
							excerpt,
						);
					}
				}
			}
		}
		raw.clear();
	}
	Ok((consumed, consumed >= len && raw.is_empty()))
}

fn visit(
	source: &Source,
	from: u64,
	bytes: u64,
	withdrawn: &WithdrawnMessages,
	mut visitor: impl FnMut(Span, Origin, u64, u64, &str),
) -> Result<(u64, VecDeque<ArchiveRef>, bool)> {
	let mut log = source.open(from, source.end)?;
	let mut archives = VecDeque::<ArchiveRef>::new();
	let mut archives_complete = true;
	let mut through = from;
	while through.saturating_sub(from) < bytes || log.has_buffered_rows() {
		let Some(raw) = log.next().transpose()? else { break };
		let (start, end) = log.frame_bounds();
		let line = log.frame_line();
		through = end;
		let Some(message) = read_message(&raw, start)? else {
			continue;
		};
		let fingerprint = withdrawal_fingerprint(&message);
		if withdrawn.contains_position(start, line, fingerprint) {
			continue;
		}
		if let Some(name) = archive_name(&message) {
			if !archives.iter().any(|archive| archive.name == name) {
				if archives.len() == MAX_ARCHIVE_REFS {
					archives.pop_front();
					archives_complete = false;
				}
				archives.push_back(ArchiveRef { end, name });
			}
		}
		let Some((origin, content)) = project(&message, source.delegated) else {
			continue;
		};
		for (part, excerpt) in (0..).zip(chunks(&content)) {
			if !excerpt.trim().is_empty() {
				let span = Span { start, end, line, part };
				visitor(
					span,
					origin,
					hash((message.timestamp, origin, part, excerpt)),
					fingerprint,
					excerpt,
				);
			}
		}
	}
	anyhow::ensure!(
		log.torn_at().is_none(),
		"delegation source contains an incomplete log frame"
	);
	Ok((through, archives, archives_complete))
}

fn read_excerpt(source: &Source, entry: &Entry) -> Result<String> {
	let span = entry.span;
	let raw = source
		.open(span.start, span.end)?
		.nth((span.line - 1) as usize)
		.transpose()?
		.context("delegation source message is missing")?;
	let message = read_message(&raw, span.start)?.context("delegation source message changed")?;
	let (origin, content) = project(&message, source.delegated).context("delegation source projection changed")?;
	let excerpt = chunks(&content)
		.nth(span.part as usize)
		.context("delegation source chunk is missing")?;
	anyhow::ensure!(
		hash((message.timestamp, origin, span.part, excerpt)) == entry.digest,
		"delegation source chunk changed"
	);
	Ok(excerpt.to_owned())
}

fn update(mut index: Index, source: &Source) -> Result<Index> {
	let withdrawn = withdrawals(source)?;
	index
		.entries
		.retain(|entry| !withdrawn.contains_position(entry.span.start, entry.span.line, entry.fingerprint));
	let mut seen: HashSet<_> = index.entries.iter().map(|entry| entry.digest).collect();
	let mut older_instructions = index.entries.len().saturating_sub(HOT_CHUNKS);
	let (through, archives, archives_complete) = visit(
		source,
		index.through,
		WORKER_SCAN_BYTES,
		&withdrawn,
		|span, origin, digest, fingerprint, excerpt| {
			if !seen.insert(digest) {
				return;
			}
			index.entries.push_back(Entry {
				span,
				source: origin,
				digest,
				fingerprint,
				terms: terms(excerpt),
				vector: None,
			});
			if index.entries.len() <= HOT_CHUNKS {
				return;
			}
			let oldest_hot = index.entries.len() - HOT_CHUNKS - 1;
			if index.entries[oldest_hot].source == Origin::UserInstruction && older_instructions < OLD_INSTRUCTIONS {
				older_instructions += 1;
				return;
			}
			let remove = if index.entries[oldest_hot].source == Origin::UserInstruction {
				0
			} else {
				oldest_hot
			};
			let removed = index
				.entries
				.remove(remove)
				.expect("the index contains an eviction candidate");
			seen.remove(&removed.digest);
			index.evicted = true;
		},
	)?;
	index.through = through;
	index.evicted |= !archives_complete;
	for archive in archives {
		if index.archives.iter().any(|old| old.name == archive.name) {
			continue;
		}
		if index.archives.len() == MAX_ARCHIVE_REFS {
			index.archives.pop_front();
			index.evicted = true;
		}
		index.archives.push_back(archive);
	}
	Ok(index)
}

async fn index_lexical(source: Source) -> Result<()> {
	let path = cache_path(&source)?;
	loop {
		let load_path = path.clone();
		let read_source = source.clone();
		let index = tokio::task::spawn_blocking(move || {
			read_source.verify()?;
			let index = load(&load_path)?.unwrap_or_default();
			if index.through >= read_source.end {
				return Ok(None);
			}
			Ok::<_, anyhow::Error>(Some(update(index, &read_source)?))
		})
		.await??;
		let Some(index) = index else { return Ok(()) };
		let complete = index.through == source.end;
		save(path.clone(), index).await?;
		if complete {
			return Ok(());
		}
	}
}

async fn index_semantic(source: Source) -> Result<()> {
	let path = cache_path(&source)?;
	let load_path = path.clone();
	let Some(mut index) = tokio::task::spawn_blocking(move || load(&load_path)).await?? else {
		return Ok(());
	};
	let pending: Vec<_> = index
		.entries
		.iter()
		.enumerate()
		.filter_map(|(position, entry)| (entry.span.end <= source.end && entry.vector.is_none()).then_some(position))
		.rev()
		.collect();
	for batch in pending.chunks(EMBED_BATCH) {
		let mut texts = Vec::with_capacity(batch.len());
		for &position in batch {
			texts.push(read_excerpt(&source, &index.entries[position])?);
		}
		let result = tokio::time::timeout(WORKER_BATCH_TIMEOUT, embed_transient(&texts))
			.await
			.context("conversation embedding batch timed out")
			.flatten();
		let vectors = match result {
			Ok(vectors) => vectors,
			Err(error) => {
				save(path, index).await?;
				return Err(error);
			}
		};
		for (&position, vector) in batch.iter().zip(vectors) {
			index.entries[position].vector = Some(vector);
		}
	}
	if !pending.is_empty() {
		save(path, index).await?;
	}
	Ok(())
}

struct Job {
	generation: uuid::Uuid,
	source: Source,
	consumers: usize,
	lexical: Option<tokio::task::AbortHandle>,
	semantic: Option<tokio::task::AbortHandle>,
}

pub(super) struct Lease(String);

impl Drop for Lease {
	fn drop(&mut self) {
		let mut jobs = JOBS.lock().expect("delegation index queue poisoned");
		let job = jobs
			.get_mut(&self.0)
			.expect("active delegation index lease has an owner");
		job.consumers -= 1;
		if job.consumers == 0 {
			let job = jobs.remove(&self.0).expect("final consumer owns the job");
			for worker in [job.lexical, job.semantic].into_iter().flatten() {
				worker.abort();
			}
		}
	}
}

fn start_semantic(job: &mut Job) {
	if job.semantic.is_some() {
		return;
	}
	let generation = job.generation;
	let mut source = job.source.clone();
	let session = source.session.clone();
	job.semantic = Some(
		tokio::spawn(async move {
			let _permit = SEMANTIC_WORKERS.acquire().await.expect("delegation workers stay open");
			loop {
				let attempted = source.end;
				if let Err(error) = index_semantic(source).await {
					crate::log_error!("Delegation embeddings {}: {:#}", session, error);
				}
				let mut jobs = JOBS.lock().expect("delegation index queue poisoned");
				let Some(job) = jobs.get_mut(&session).filter(|job| job.generation == generation) else {
					break;
				};
				if job.source.end <= attempted || job.lexical.is_some() {
					job.semantic = None;
					break;
				}
				source = job.source.clone();
			}
		})
		.abort_handle(),
	);
}

fn start_lexical(job: &mut Job) {
	if job.lexical.is_some() {
		return;
	}
	let generation = job.generation;
	let mut source = job.source.clone();
	let session = source.session.clone();
	job.lexical = Some(
		tokio::spawn(async move {
			let _permit = LEXICAL_WORKERS.acquire().await.expect("delegation workers stay open");
			loop {
				let attempted = source.end;
				let result = index_lexical(source).await;
				if let Err(error) = &result {
					crate::log_error!("Delegation index {}: {:#}", session, error);
				}
				let mut jobs = JOBS.lock().expect("delegation index queue poisoned");
				let Some(job) = jobs.get_mut(&session).filter(|job| job.generation == generation) else {
					break;
				};
				if job.source.end <= attempted {
					job.lexical = None;
					if result.is_ok() {
						start_semantic(job);
					}
					break;
				}
				source = job.source.clone();
			}
		})
		.abort_handle(),
	);
}

pub(super) fn queue(source: Source) -> Option<Lease> {
	let mut jobs = JOBS.lock().expect("delegation index queue poisoned");
	if jobs.len() == MAX_QUEUE && !jobs.contains_key(&source.session) {
		crate::log_debug!("Delegation index queue full for {}", source.session);
		return None;
	}
	let session = source.session.clone();
	let job = jobs.entry(session.clone()).or_insert_with(|| Job {
		generation: uuid::Uuid::new_v4(),
		source: source.clone(),
		consumers: 0,
		lexical: None,
		semantic: None,
	});
	if job.source.end < source.end {
		job.source = source;
	}
	job.consumers += 1;
	start_lexical(job);
	Some(Lease(session))
}

pub(super) fn advance(session: &str, session_file: &Path) -> Result<()> {
	let mut jobs = JOBS.lock().expect("delegation index queue poisoned");
	let Some(job) = jobs.get_mut(session) else {
		return Ok(());
	};
	let path = job.source.path()?;
	if path != session_file {
		return Ok(());
	}
	let end = path.metadata()?.len();
	if end > job.source.end {
		job.source = Source::capture(session, &path, job.source.delegated)?;
		start_lexical(job);
	}
	Ok(())
}

#[derive(Default, Serialize)]
pub(crate) struct Coverage {
	lexical_complete: bool,
	semantic_complete: bool,
}

#[derive(Serialize)]
pub(crate) struct SearchResult {
	coverage: Coverage,
	results: Vec<Passage>,
}

fn exact_match(text: &str, query: &str, quoted: &[&str]) -> bool {
	let lower = text.to_lowercase();
	lower.contains(query) || quoted.iter().any(|phrase| lower.contains(phrase))
}

fn lexical_score(text: &str, query: &str, quoted: &[&str], query_terms: &[u64]) -> usize {
	if exact_match(text, query, quoted) {
		return query_terms.len() + 1;
	}
	terms(text)
		.iter()
		.filter(|term| query_terms.binary_search(term).is_ok())
		.count()
}

struct Hit {
	passage: Passage,
	exact: bool,
	rank: f32,
	order: u64,
}

fn push_hit(hits: &mut Vec<Hit>, hit: Hit, limit: usize) {
	let key = |hit: &Hit| {
		let instruction = hit.passage.source == Origin::UserInstruction;
		(hit.exact, hit.exact && instruction, hit.rank, instruction, hit.order)
	};
	let position = hits.partition_point(|old| key(old) >= key(&hit));
	if position < limit {
		hits.insert(position, hit);
		hits.truncate(limit);
	}
}

fn passage(depth: usize, span: Span, source: Origin, excerpt: String) -> Passage {
	Passage {
		depth,
		source,
		reference: format!("message:{}:{}:{}", span.start, span.line, span.part + 1),
		excerpt,
	}
}

fn search_index(
	grant: &Grant,
	indexes: Vec<(usize, Index)>,
	query: &str,
	query_vector: Option<Vec<f32>>,
	limit: usize,
) -> Result<SearchResult> {
	let query = query.to_lowercase();
	let query_terms = terms(&query);
	let quoted: Vec<_> = query
		.split('"')
		.enumerate()
		.filter_map(|(i, phrase)| (i % 2 == 1 && !phrase.trim().is_empty()).then_some(phrase))
		.collect();
	let mut hits = Vec::new();
	let mut coverage = Coverage {
		lexical_complete: true,
		semantic_complete: query_vector.is_some(),
	};
	let mut scan_budget = QUERY_SCAN_BYTES;
	for (i, index) in indexes {
		let source = &grant.sources[i];
		let withdrawn = withdrawals(source)?;
		let mut scored = Vec::new();
		let mut archives = index.archives;
		let mut seen = HashSet::new();
		let mut vectorized = HashSet::new();
		for entry in index.entries.iter().filter(|entry| entry.span.end <= source.end) {
			if withdrawn.contains_position(entry.span.start, entry.span.line, entry.fingerprint) {
				continue;
			}
			if entry.vector.is_some() {
				vectorized.insert(entry.digest);
			}
			let lexical = entry
				.terms
				.iter()
				.filter(|term| query_terms.binary_search(term).is_ok())
				.count();
			let semantic = match (&entry.vector, &query_vector) {
				(Some(vector), Some(query)) if vector.len() == EMBED_DIM => cosine(vector, query),
				_ => {
					coverage.semantic_complete = false;
					0.0
				}
			};
			if lexical > 0 || semantic >= MIN_SIMILARITY {
				scored.push((lexical, semantic, entry));
			}
		}
		// Keep the lexical and semantic candidate lists separate so a good
		// embedding cannot crowd out a literal filename or symbol match.
		let candidate_limit = limit * 4;
		scored.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| b.2.span.start.cmp(&a.2.span.start)));
		let lexical: HashMap<_, _> = scored
			.iter()
			.filter(|(score, ..)| *score > 0)
			.take(candidate_limit)
			.enumerate()
			.map(|(rank, (_, _, entry))| (entry.span, rank))
			.collect();
		scored.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
		let semantic: HashMap<_, _> = scored
			.iter()
			.filter(|(_, score, _)| *score >= MIN_SIMILARITY)
			.take(candidate_limit)
			.enumerate()
			.map(|(rank, (_, _, entry))| (entry.span, rank))
			.collect();
		let rrf = |rank: Option<&usize>| rank.map_or(0.0, |rank| 1.0 / (RRF_K + *rank as f32));
		for (_, _, entry) in scored {
			let rank = rrf(lexical.get(&entry.span)) + rrf(semantic.get(&entry.span));
			if rank == 0.0 || !seen.insert(entry.digest) {
				continue;
			}
			let excerpt = read_excerpt(source, entry)?;
			let exact = exact_match(&excerpt, &query, &quoted);
			push_hit(
				&mut hits,
				Hit {
					passage: passage(i + 1, entry.span, entry.source, excerpt),
					exact,
					rank,
					order: entry.span.start,
				},
				limit,
			);
		}

		if index.evicted || index.through < source.end {
			let from = if index.evicted { 0 } else { index.through };
			let (through, scanned_archives, archives_complete) = visit(
				source,
				from,
				scan_budget,
				&withdrawn,
				|span, origin, digest, _, excerpt| {
					let score = lexical_score(excerpt, &query, &quoted, &query_terms);
					if score > 0 && seen.insert(digest) {
						push_hit(
							&mut hits,
							Hit {
								passage: passage(i + 1, span, origin, excerpt.to_owned()),
								exact: score > query_terms.len(),
								rank: score as f32 / (RRF_K * (query_terms.len() + 1) as f32),
								order: span.start,
							},
							limit,
						);
					}
				},
			)?;
			archives.extend(scanned_archives);
			scan_budget = scan_budget.saturating_sub(through.saturating_sub(from));
			coverage.lexical_complete &= through >= source.end && archives_complete;
			coverage.semantic_complete = false;
		}
		let mut visited_archives = HashSet::new();
		for archive in archives.iter().rev().filter(|archive| archive.end <= source.end) {
			if !visited_archives.insert(&archive.name) {
				continue;
			}
			if scan_budget == 0 {
				coverage.lexical_complete = false;
				coverage.semantic_complete = false;
				break;
			}
			let (consumed, complete) = visit_archive(
				source,
				&archive.name,
				scan_budget,
				&withdrawn,
				|line, part, origin, digest, excerpt| {
					coverage.semantic_complete &= vectorized.contains(&digest);
					let score = lexical_score(excerpt, &query, &quoted, &query_terms);
					if score > 0 && seen.insert(digest) {
						push_hit(
							&mut hits,
							Hit {
								passage: Passage {
									depth: i + 1,
									source: origin,
									reference: format!("archive:{}:{}:{}", archive.name, line, part + 1),
									excerpt: excerpt.to_owned(),
								},
								exact: score > query_terms.len(),
								rank: score as f32 / (RRF_K * (query_terms.len() + 1) as f32),
								order: archive.end,
							},
							limit,
						);
						coverage.semantic_complete = false;
					}
				},
			)?;
			scan_budget = scan_budget.saturating_sub(consumed);
			coverage.lexical_complete &= complete;
		}
	}

	let mut remaining = MAX_EXCERPT_BYTES;
	let results = hits
		.into_iter()
		.map_while(|mut hit| {
			if remaining == 0 {
				return None;
			}
			hit.passage
				.excerpt
				.truncate(hit.passage.excerpt.floor_char_boundary(remaining));
			remaining -= hit.passage.excerpt.len();
			Some(hit.passage)
		})
		.collect();
	Ok(SearchResult { coverage, results })
}

pub(crate) async fn search(grant: Arc<Grant>, query: String, scope: Scope, limit: usize) -> Result<SearchResult> {
	let slot = SEARCH_SLOTS
		.clone()
		.try_acquire_owned()
		.context("delegation search is busy")?;
	let sources = Arc::clone(&grant);
	let indexes = tokio::task::spawn_blocking(move || {
		sources
			.sources
			.iter()
			.enumerate()
			.filter(|(i, _)| scope.includes(*i, sources.sources.len()))
			.map(|(i, source)| {
				source.verify()?;
				Ok((i, load(&cache_path(source)?)?.unwrap_or_default()))
			})
			.collect::<Result<Vec<_>>>()
	})
	.await??;
	let mut vector = None;
	if indexes.iter().any(|(i, index)| {
		index
			.entries
			.iter()
			.any(|entry| entry.span.end <= grant.sources[*i].end && entry.vector.is_some())
	}) && crate::embeddings::is_ready()
	{
		match tokio::time::timeout(QUERY_TIMEOUT, embed_transient(std::slice::from_ref(&query))).await {
			Ok(Ok(mut vectors)) => vector = vectors.pop(),
			Ok(Err(error)) => crate::log_debug!("Delegation query embedding unavailable: {:#}", error),
			Err(_) => crate::log_debug!("Delegation query embedding timed out"),
		}
	}
	tokio::task::spawn_blocking(move || {
		let _slot = slot;
		search_index(&grant, indexes, &query, vector, limit)
	})
	.await?
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
