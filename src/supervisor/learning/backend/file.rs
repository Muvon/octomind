// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License")

use super::super::Lesson;
use anyhow::Result;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct FileBackend;

const COLD_RECALL_LIMIT: usize = 2;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Write beside the destination, sync, then rename into place. Same-directory
/// rename gives the learning authority an atomic old-or-new view without
/// pulling the test-only `tempfile` crate into production dependencies.
fn atomic_write(path: &std::path::Path, content: &[u8]) -> Result<()> {
	let dir = path
		.parent()
		.ok_or_else(|| anyhow::anyhow!("learning file has no parent: {}", path.display()))?;
	let name = path
		.file_name()
		.and_then(|value| value.to_str())
		.unwrap_or("memory");
	for _ in 0..32 {
		let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
		let temporary = dir.join(format!(".{name}.{}.{sequence}.tmp", std::process::id()));
		let opened = std::fs::OpenOptions::new()
			.create_new(true)
			.write(true)
			.open(&temporary);
		let mut file = match opened {
			Ok(file) => file,
			Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
			Err(error) => return Err(error.into()),
		};
		let result = (|| -> std::io::Result<()> {
			file.write_all(content)?;
			file.sync_all()?;
			drop(file);
			std::fs::rename(&temporary, path)
		})();
		if result.is_err() {
			let _ = std::fs::remove_file(&temporary);
		}
		return result.map_err(Into::into);
	}
	anyhow::bail!(
		"could not reserve temporary learning file in {}",
		dir.display()
	)
}

/// Reverse the `store` escaping of the quoted YAML string values: `\"` -> `"`,
/// `\\` -> `\`, `\n` -> newline. Single-pass so an escaped backslash is never
/// re-interpreted as an escape (which is exactly the runaway-backslash bug:
/// reinforce read-modify-writes a lesson every importance bump, so any
/// store/parse asymmetry compounds one level per recall). Inverse of `escape`.
fn unescape(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	let mut chars = s.chars();
	while let Some(c) = chars.next() {
		if c == '\\' {
			match chars.next() {
				Some('"') => out.push('"'),
				Some('\\') => out.push('\\'),
				Some('n') => out.push('\n'),
				Some(other) => {
					out.push('\\');
					out.push(other);
				}
				None => out.push('\\'),
			}
		} else {
			out.push(c);
		}
	}
	out
}

/// Escape a value for a double-quoted YAML string: `\` -> `\\`, `"` -> `\"`,
/// newline -> `\n`. Backslash MUST be escaped first, so the backslashes
/// introduced for `"` and newline are not themselves doubled. The parser is
/// line-based, so an unescaped newline would split one value across lines and
/// corrupt the record. Exact inverse of `unescape`.
fn escape(s: &str) -> String {
	s.replace('\\', "\\\\")
		.replace('"', "\\\"")
		.replace('\n', "\\n")
}

fn parse_string_list(value: &str) -> Vec<String> {
	serde_json::from_str::<Vec<String>>(value).unwrap_or_else(|_| {
		value
			.trim_start_matches('[')
			.trim_end_matches(']')
			.split(',')
			.map(|item| item.trim().trim_matches('"').to_string())
			.filter(|item| !item.is_empty())
			.collect()
	})
}

impl FileBackend {
	fn learning_dir(role: &str, project: &str) -> Result<PathBuf> {
		crate::directories::get_learning_dir(role, project)
	}

	/// Parse a lesson `.md` file with YAML frontmatter.
	/// Simple key-value parser — no serde_yaml dependency needed.
	fn parse_lesson_file(content: &str) -> Option<Lesson> {
		let content = content.trim();
		if !content.starts_with("---") {
			return None;
		}
		let after_first = &content[3..];
		let end = after_first.find("---")?;
		let yaml_str = after_first[..end].trim();

		let mut lesson = Lesson::default();
		for line in yaml_str.lines() {
			let line = line.trim();
			let Some((key, val)) = line.split_once(':') else {
				continue;
			};
			let key = key.trim();
			// Strip exactly one surrounding quote pair (store wraps in one).
			// `trim_matches('"')` would greedily eat an escaped trailing quote
			// (`"…\""`) and leave a dangling backslash for `unescape` to mangle.
			let val = val.trim();
			let val = val
				.strip_prefix('"')
				.and_then(|v| v.strip_suffix('"'))
				.unwrap_or(val);
			match key {
				"title" => lesson.title = unescape(val),
				"content" => lesson.content = unescape(val),
				"memory_type" => lesson.memory_type = val.to_string(),
				"importance" => lesson.importance = val.parse().unwrap_or(0.5),
				"confidence" => lesson.confidence = val.to_string(),
				"tags" => {
					// Parse [tag1, tag2] format
					let inner = val.trim_start_matches('[').trim_end_matches(']');
					lesson.tags = inner
						.split(',')
						.map(|t| t.trim().to_string())
						.filter(|t| !t.is_empty())
						.collect();
				}
				"source" => lesson.source = unescape(val),
				"role" => lesson.role = unescape(val),
				"project" => lesson.project = unescape(val),
				"scope" => lesson.scope = val.to_string(),
				"created" => lesson.created = unescape(val),
				"related" => lesson.related = parse_string_list(val),
				"evidence" => lesson.evidence = parse_string_list(val),
				"outcome" => {
					lesson.outcome = val.parse().unwrap_or_default();
				}
				"last_used" => lesson.last_used = unescape(val),
				"use_count" => lesson.use_count = val.parse().unwrap_or(0),
				_ => {}
			}
		}

		if lesson.content.is_empty() {
			None
		} else {
			Some(lesson)
		}
	}

	/// Read all lesson `.md` files from a directory, parsed and sorted by
	/// importance descending. Missing dir or unreadable files → empty/skipped.
	/// Shared by `retrieve_all` (scoped) and `retrieve_global`.
	fn read_lessons_sorted(dir: &std::path::Path) -> Vec<Lesson> {
		let Ok(entries) = std::fs::read_dir(dir) else {
			return Vec::new();
		};
		let mut lessons = Vec::new();
		for entry in entries.flatten() {
			let path = entry.path();
			if path.extension().is_some_and(|e| e == "md") {
				if let Ok(content) = std::fs::read_to_string(&path) {
					if let Some(mut lesson) = Self::parse_lesson_file(&content) {
						lesson.storage_path = path.display().to_string();
						lessons.push(lesson);
					}
				}
			}
		}
		lessons.sort_by(|a, b| {
			b.importance
				.partial_cmp(&a.importance)
				.unwrap_or(std::cmp::Ordering::Equal)
		});
		lessons
	}

	pub(crate) fn read_archived(dir: &std::path::Path) -> Vec<Lesson> {
		let archive = dir.join(".archive");
		let Ok(types) = std::fs::read_dir(archive) else {
			return Vec::new();
		};
		let mut lessons = Vec::new();
		for type_dir in types.flatten().filter(|entry| entry.path().is_dir()) {
			let Ok(entries) = std::fs::read_dir(type_dir.path()) else {
				continue;
			};
			for entry in entries.flatten() {
				let path = entry.path();
				if path.extension().is_some_and(|extension| extension == "md") {
					if let Ok(content) = std::fs::read_to_string(&path) {
						if let Some(mut lesson) = Self::parse_lesson_file(&content) {
							lesson.storage_path = path.display().to_string();
							lessons.push(lesson);
						}
					}
				}
			}
		}
		lessons
	}

	/// Page a tiny exact-match slice from cold storage. The append-only catalog
	/// avoids embedding or parsing the entire archive; stale catalog rows are
	/// harmless because their file path is checked before use.
	pub(crate) fn retrieve_archived(
		dir: &std::path::Path,
		patterns: &[String],
		intent: &str,
		limit: usize,
	) -> Vec<Lesson> {
		if limit == 0 {
			return Vec::new();
		}
		let mut pattern_terms: Vec<String> = patterns
			.iter()
			.map(|term| term.trim().to_ascii_lowercase())
			.filter(|term| !term.is_empty())
			.collect();
		pattern_terms.sort();
		pattern_terms.dedup();
		let stopwords = [
			"about", "after", "before", "could", "from", "have", "into", "memory", "right",
			"should", "their", "there", "these", "this", "using", "what", "when", "where", "which",
			"with", "would", "your",
		];
		let mut intent_terms: Vec<String> = intent
			.split(|character: char| !character.is_alphanumeric())
			.map(str::to_ascii_lowercase)
			.filter(|term| term.len() >= 4 && !stopwords.contains(&term.as_str()))
			.take(12)
			.collect();
		intent_terms.sort();
		intent_terms.dedup();
		if pattern_terms.is_empty() && intent_terms.is_empty() {
			return Vec::new();
		}

		let catalog = dir.join(".archive").join("catalog.jsonl");
		let Ok(content) = std::fs::read_to_string(catalog) else {
			return Vec::new();
		};
		let mut ranked = content
			.lines()
			.filter_map(|line| {
				serde_json::from_str::<super::super::retention::ArchiveCatalogEntry>(line).ok()
			})
			.filter_map(|entry| {
				let text = entry.search_text();
				let pattern_hits = pattern_terms
					.iter()
					.filter(|term| text.contains(term.as_str()))
					.count();
				let intent_hits = intent_terms
					.iter()
					.filter(|term| text.contains(term.as_str()))
					.count();
				// LLM-prepared patterns are already selective. Raw follow-up intent
				// needs two independent terms so one generic word cannot page noise.
				(pattern_hits > 0 || intent_hits >= 2).then_some((
					pattern_hits * 4 + intent_hits,
					entry.importance,
					entry,
				))
			})
			.collect::<Vec<_>>();
		ranked.sort_by(|left, right| {
			right.0.cmp(&left.0).then_with(|| {
				right
					.1
					.partial_cmp(&left.1)
					.unwrap_or(std::cmp::Ordering::Equal)
			})
		});

		let mut recalled = Vec::new();
		let mut seen = std::collections::HashSet::new();
		for (_, _, entry) in ranked {
			let path = entry.path(dir);
			if !seen.insert(path.clone()) {
				continue;
			}
			let Ok(content) = std::fs::read_to_string(&path) else {
				continue;
			};
			if let Some(mut lesson) = Self::parse_lesson_file(&content) {
				lesson.storage_path = path.display().to_string();
				recalled.push(lesson);
				if recalled.len() >= limit {
					break;
				}
			}
		}
		recalled
	}

	fn find_archived_by_content(dir: &std::path::Path, content: &str) -> Option<Lesson> {
		let catalog = std::fs::read_to_string(dir.join(".archive").join("catalog.jsonl")).ok()?;
		for line in catalog.lines().rev() {
			let Ok(entry) =
				serde_json::from_str::<super::super::retention::ArchiveCatalogEntry>(line)
			else {
				continue;
			};
			let path = entry.path(dir);
			let Ok(stored) = std::fs::read_to_string(&path) else {
				continue;
			};
			let Some(mut lesson) = Self::parse_lesson_file(&stored) else {
				continue;
			};
			if lesson.content.trim() == content.trim() {
				lesson.storage_path = path.display().to_string();
				return Some(lesson);
			}
		}
		None
	}

	fn reactivate_archived(item: &mut Lesson) -> Result<()> {
		let cold = std::path::PathBuf::from(&item.storage_path);
		if item.storage_path.is_empty()
			|| !cold
				.components()
				.any(|component| component.as_os_str() == std::ffi::OsStr::new(".archive"))
		{
			return Ok(());
		}
		let hot_dir = if item.scope == "global" {
			crate::directories::get_global_learning_dir()?
		} else {
			Self::learning_dir(&item.role, &item.project)?
		};
		let hot = hot_dir.join(format!("{}.md", item.file_id()));
		std::fs::rename(&cold, &hot)?;
		item.storage_path = hot.display().to_string();
		Ok(())
	}
}

/// Importance at/below which a reinforced entry leaves ordinary recall for the
/// lossless cold archive.
const IMPORTANCE_FLOOR: f64 = 0.1;
/// Stale entries are pruned only once their importance has fallen to/below this;
/// entries bumped above it by reinforcement survive regardless of age.
const PRUNE_THRESHOLD: f64 = 0.4;

impl FileBackend {
	pub(crate) async fn store(&self, lesson: &Lesson) -> Result<()> {
		// Global lessons live in the shared `learning/_/` dir; scoped lessons
		// in `learning/{project}/{role}/`. Filename is the canonical file_id.
		let dir = if lesson.scope == "global" {
			crate::directories::get_global_learning_dir()?
		} else {
			Self::learning_dir(&lesson.role, &lesson.project)?
		};
		let filename = format!("{}.md", lesson.file_id());

		let tags_str = lesson.tags.join(", ");
		let content = format!(
			"---\ntitle: \"{}\"\ncontent: \"{}\"\nmemory_type: {}\nimportance: {}\nconfidence: {}\ntags: [{}]\nsource: \"{}\"\nrole: \"{}\"\nproject: \"{}\"\nscope: {}\ncreated: \"{}\"\nrelated: {}\nevidence: {}\noutcome: {}\nlast_used: \"{}\"\nuse_count: {}\n---\n",
			escape(&lesson.title),
			escape(&lesson.content),
			lesson.memory_type,
			lesson.importance,
			lesson.confidence,
			tags_str,
			escape(&lesson.source),
			escape(&lesson.role),
			escape(&lesson.project),
			lesson.scope,
			escape(&lesson.created),
			serde_json::to_string(&lesson.related)?,
			serde_json::to_string(&lesson.evidence)?,
			lesson.outcome.as_str(),
			escape(&lesson.last_used),
			lesson.use_count,
		);

		// A consolidation stores its replacement before moving source files. Make
		// that ordering crash-safe: a partial write must never become the durable
		// authority for either ordinary extraction or retention maintenance.
		atomic_write(&dir.join(filename), content.as_bytes())?;
		Ok(())
	}

	pub(crate) async fn retrieve(
		&self,
		intent: &str,
		patterns: &[String],
		role: &str,
		project: &str,
		limit: usize,
	) -> Result<Vec<Lesson>> {
		let dir = Self::learning_dir(role, project)?;
		if !dir.exists() {
			return Ok(Vec::new());
		}

		let all = self.retrieve_all(role, project).await?;
		if patterns.is_empty() && intent.trim().is_empty() {
			return Ok(all.into_iter().take(limit).collect());
		}
		let cold = Self::retrieve_archived(&dir, patterns, intent, COLD_RECALL_LIMIT.min(limit));
		if all.is_empty() {
			return Ok(cold);
		}

		// Sparse signal: LLM-extracted keywords → substring count → ranked by hits.
		let keyword_ranking = rank_by_keywords(&all, patterns);

		// Dense signal: MiniLM-L6 cosine. Skip silently if the model isn't
		// ready yet (warmup pending, no network, etc.) — keyword ranking
		// alone still produces a result. Same fall-through pattern as
		// capability auto-activation.
		let cosine_ranking = if intent.trim().is_empty() || !crate::embeddings::is_ready() {
			Vec::new()
		} else {
			match cosine_scores(&all, intent, PRODUCTION_DENSE_SCORING).await {
				Ok(scores) => scores
					.into_iter()
					.filter_map(|(score, index)| (score > COSINE_FLOOR).then_some(index))
					.collect(),
				Err(e) => {
					crate::log_debug!("learning retrieve: cosine ranking failed ({})", e);
					Vec::new()
				}
			}
		};

		// Fuse both rankings via Reciprocal Rank Fusion (Cormack et al. 2009).
		// Returns indices into `all` sorted by fused score descending.
		let keyword_weight = adaptive_keyword_weight(&keyword_ranking, &cosine_ranking, &all);
		let mut rankings: Vec<(&[usize], f32)> = Vec::with_capacity(2);
		rankings.push((&keyword_ranking, keyword_weight));
		if !cosine_ranking.is_empty() {
			rankings.push((&cosine_ranking, 1.0));
		}
		let mut fused = weighted_reciprocal_rank_fusion(all.len(), &rankings);

		// Recency reweight: nudge recent lessons up *among the already-relevant*
		// candidates. RRF has already dropped zero-signal items, so this only
		// reorders things that matched — it never surfaces irrelevant-but-new
		// lessons. Relevance still dominates; recency breaks ties and gives
		// fresh context a mild edge over stale.
		for (score, idx) in fused.iter_mut() {
			*score *= 1.0 + RECENCY_WEIGHT * recency_factor(&all[*idx].created);
			// Outcome credit must influence future scoped recall, not only global
			// ordering and eventual pruning. Keep it a bounded rerank factor so
			// lexical/semantic relevance remains the admission signal.
			*score *= importance_factor(all[*idx].importance);
		}
		fused.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
		promote_sparse_rescue(&mut fused, &keyword_ranking, &all);

		let hot = expand_ranked_with_links(&all, &fused, limit);
		let mut recalled = Vec::with_capacity(limit);
		let mut seen = std::collections::HashSet::new();
		// Cold entries require exact lexical evidence while hot entries may arrive
		// semantically. Put the precise cold page first, then fill from hot rank.
		for lesson in cold.into_iter().chain(hot) {
			if seen.insert(lesson.content.clone()) {
				recalled.push(lesson);
				if recalled.len() >= limit {
					break;
				}
			}
		}
		Ok(recalled)
	}

	pub(crate) async fn delete(&self, id: &str, role: &str, project: &str) -> Result<()> {
		// A lesson id is unique across scopes (content slug + timestamp), so we
		// search both the scoped dir and the global dir; first match wins.
		let dirs = [
			Self::learning_dir(role, project)?,
			crate::directories::get_global_learning_dir()?,
		];
		for dir in dirs {
			if !dir.exists() {
				continue;
			}
			for entry in std::fs::read_dir(&dir)? {
				let entry = entry?;
				let path = entry.path();
				if path.extension().is_some_and(|e| e == "md")
					&& path.file_stem().and_then(|s| s.to_str()) == Some(id)
				{
					std::fs::remove_file(&path)?;
					return Ok(());
				}
			}
		}
		anyhow::bail!("lesson '{}' not found", id)
	}

	pub(crate) async fn retrieve_all(&self, role: &str, project: &str) -> Result<Vec<Lesson>> {
		let dir = Self::learning_dir(role, project)?;
		Ok(Self::read_lessons_sorted(&dir))
	}

	pub(crate) async fn retrieve_global(&self) -> Result<Vec<Lesson>> {
		let dir = crate::directories::get_global_learning_dir()?;
		Ok(Self::read_lessons_sorted(&dir))
	}

	pub(crate) async fn retrieve_archived_global(
		&self,
		intent: &str,
		patterns: &[String],
		limit: usize,
	) -> Result<Vec<Lesson>> {
		let dir = crate::directories::get_global_learning_dir()?;
		Ok(Self::retrieve_archived(
			&dir,
			patterns,
			intent,
			limit.min(COLD_RECALL_LIMIT),
		))
	}

	/// Dir scan only — no parsing: the caller just needs to know whether the
	/// scope is worth a retrieval query.
	pub(crate) async fn has_lessons(&self, role: &str, project: &str) -> bool {
		let Ok(dir) = Self::learning_dir(role, project) else {
			return false;
		};
		let hot = std::fs::read_dir(&dir).is_ok_and(|entries| {
			entries
				.flatten()
				.any(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
		});
		hot || std::fs::metadata(dir.join(".archive").join("catalog.jsonl"))
			.is_ok_and(|metadata| metadata.len() > 0)
	}

	pub(crate) async fn reinforce(
		&self,
		content: &str,
		role: &str,
		project: &str,
		delta: f64,
	) -> Result<()> {
		// Find the recalled entry (scoped first, then global) by content.
		let mut entries = Self::read_lessons_sorted(&Self::learning_dir(role, project)?);
		entries.extend(Self::read_lessons_sorted(
			&crate::directories::get_global_learning_dir()?,
		));
		let mut entry = entries
			.into_iter()
			.find(|l| l.content.trim() == content.trim())
			.or_else(|| {
				Self::find_archived_by_content(&Self::learning_dir(role, project).ok()?, content)
			})
			.or_else(|| {
				Self::find_archived_by_content(
					&crate::directories::get_global_learning_dir().ok()?,
					content,
				)
			});
		let Some(mut entry) = entry.take() else {
			return Ok(());
		};
		Self::reactivate_archived(&mut entry)?;
		let new_importance = (entry.importance + delta).clamp(0.0, 1.0);
		entry.last_used = chrono::Utc::now().to_rfc3339();
		entry.use_count = entry.use_count.saturating_add(1);
		if new_importance <= IMPORTANCE_FLOOR {
			entry.importance = new_importance;
			self.store(&entry).await?;
			super::super::retention::archive_record(&entry)?;
			crate::supervisor::stats::memory_retention(0, 1);
			crate::log_debug!(
				"Reinforce: cold-archived (importance {:.2}): {}",
				new_importance,
				entry.content
			);
		} else {
			entry.importance = new_importance;
			self.store(&entry).await?; // same file_id → overwrites in place
			crate::log_debug!(
				"Reinforce: {} importance -> {:.2}",
				entry.content,
				new_importance
			);
		}
		Ok(())
	}

	pub(crate) async fn prune_stale(
		&self,
		role: &str,
		project: &str,
		decay_days: u64,
	) -> Result<()> {
		if decay_days == 0 {
			return Ok(());
		}
		let cutoff_secs = (decay_days * 86_400) as i64;
		let now = chrono::Utc::now();
		let mut archived = 0;
		for entry in Self::read_lessons_sorted(&Self::learning_dir(role, project)?) {
			if entry.importance > PRUNE_THRESHOLD {
				continue; // proven useful — keep regardless of age
			}
			let stale = chrono::DateTime::parse_from_rfc3339(&entry.created)
				.map(|c| (now - c.with_timezone(&chrono::Utc)).num_seconds() > cutoff_secs)
				.unwrap_or(false);
			if stale && super::super::retention::archive_record(&entry).is_ok() {
				archived += 1;
				crate::log_debug!("Decay: cold-archived stale weak entry: {}", entry.content);
			}
		}
		if archived > 0 {
			crate::supervisor::stats::memory_retention(0, archived);
		}
		Ok(())
	}
}

/// RRF constant from Cormack, Clarke & Buettcher (2009). 60 is the
/// canonical value — high enough that early ranks dominate without
/// crushing later ranks completely.
const RRF_K: f32 = 60.0;
/// Dense relevance is the primary semantic signal. Sparse matches are valuable
/// for exact identifiers but noisier around stale/corrected memories, so when
/// dense evidence exists they contribute one quarter of its RRF mass. With no
/// dense ranking, uniform scaling preserves the complete keyword order.
const KEYWORD_RRF_WEIGHT: f32 = 0.25;
const LOW_TRUST_IMPORTANCE: f64 = 0.4;

/// Recency reweight strength: a brand-new lesson gets at most +50% on its
/// fused relevance score. Small enough that relevance still leads.
const RECENCY_WEIGHT: f32 = 0.5;
/// Recency half-life in days: a lesson this old gets a ~0.5 recency factor.
const RECENCY_HALFLIFE_DAYS: f32 = 30.0;
/// Importance 0..1 maps to a modest 0.75x..1.25x rerank multiplier.
const IMPORTANCE_RERANK_WEIGHT: f32 = 0.5;
const COSINE_FLOOR: f32 = 0.2;

#[derive(Clone, Copy)]
struct DenseScoring {
	chunk_tokens: usize,
	max_chunk_weight: f32,
}

const PRODUCTION_DENSE_SCORING: DenseScoring = DenseScoring {
	chunk_tokens: 128,
	max_chunk_weight: 1.0,
};

fn importance_factor(importance: f64) -> f32 {
	1.0 + IMPORTANCE_RERANK_WEIGHT * (importance.clamp(0.0, 1.0) as f32 - 0.5)
}

/// Equal RRF is strongest for neutral memories, but sparse phrasing becomes
/// hazardous when a weak/stale correction candidate is among its first hits.
/// Downweight sparse evidence only for that conflict-contaminated query; dense
/// outage still preserves the full sparse order.
fn adaptive_keyword_weight(sparse: &[usize], dense: &[usize], lessons: &[Lesson]) -> f32 {
	if dense.is_empty() {
		return 1.0;
	}
	if sparse
		.iter()
		.take(3)
		.any(|index| lessons[*index].importance < LOW_TRUST_IMPORTANCE)
	{
		KEYWORD_RRF_WEIGHT
	} else {
		1.0
	}
}

/// Preserve one precise sparse avenue without letting lexical noise control
/// the head of the ranking. Among the first three sparse hits, the strongest
/// outcome-adjusted memory may occupy rank five when dense fusion buried it.
/// This protects indirect identifier/correction recall while ranks 1-4 remain
/// entirely score-driven.
fn promote_sparse_rescue(ranked: &mut Vec<(f32, usize)>, sparse: &[usize], lessons: &[Lesson]) {
	let candidate = sparse.iter().take(3).copied().max_by(|left, right| {
		lessons[*left]
			.importance
			.partial_cmp(&lessons[*right].importance)
			.unwrap_or(std::cmp::Ordering::Equal)
	});
	let Some(candidate) = candidate else {
		return;
	};
	if ranked.iter().take(5).any(|(_, index)| *index == candidate) {
		return;
	}
	let Some(current) = ranked.iter().position(|(_, index)| *index == candidate) else {
		return;
	};
	let item = ranked.remove(current);
	let position = ranked.len().min(4);
	ranked.insert(position, item);
}

/// Interleave each directly retrieved memory with its explicit one-hop targets
/// and backlinks. Direct ranking still chooses every root; graph edges only
/// spend remaining result slots and never recurse.
fn expand_ranked_with_links(all: &[Lesson], ranked: &[(f32, usize)], limit: usize) -> Vec<Lesson> {
	if limit == 0 {
		return Vec::new();
	}
	let mut by_id = std::collections::HashMap::new();
	for (index, lesson) in all.iter().enumerate() {
		by_id.insert(lesson.file_id(), index);
	}
	let mut backlinks: std::collections::HashMap<&str, Vec<usize>> =
		std::collections::HashMap::new();
	for (index, lesson) in all.iter().enumerate() {
		for target in &lesson.related {
			backlinks.entry(target.as_str()).or_default().push(index);
		}
	}

	let mut selected = Vec::new();
	let mut seen = std::collections::HashSet::new();
	for &(_, root) in ranked {
		if root >= all.len() {
			continue;
		}
		let root_id = all[root].file_id();
		if seen.insert(root) {
			selected.push(all[root].clone());
		}
		if selected.len() >= limit {
			break;
		}

		let forward = all[root]
			.related
			.iter()
			.filter_map(|id| by_id.get(id).copied());
		let reverse = backlinks
			.get(root_id.as_str())
			.into_iter()
			.flat_map(|indices| indices.iter().copied());
		for neighbor in forward.chain(reverse) {
			if seen.insert(neighbor) {
				selected.push(all[neighbor].clone());
				if selected.len() >= limit {
					break;
				}
			}
		}
		if selected.len() >= limit {
			break;
		}
	}
	selected
}

/// Map a lesson's `created` (RFC3339) to a recency factor in (0, 1]: ~1.0 for
/// brand-new, decaying toward 0 with age. Unparseable/empty dates → 0 (no boost).
fn recency_factor(created: &str) -> f32 {
	match chrono::DateTime::parse_from_rfc3339(created) {
		Ok(t) => {
			let age_secs = (chrono::Utc::now().timestamp() - t.timestamp()).max(0) as f32;
			let age_days = age_secs / 86_400.0;
			1.0 / (1.0 + age_days / RECENCY_HALFLIFE_DAYS)
		}
		Err(_) => 0.0,
	}
}

/// Rank lessons by sparse keyword hit count (descending). Returns indices
/// into the input slice, in ranked order. Lessons with zero hits are
/// excluded so they don't pollute the fused ranking. Pure helper —
/// embedding-free, instant.
fn rank_by_keywords(lessons: &[Lesson], patterns: &[String]) -> Vec<usize> {
	if patterns.is_empty() {
		return Vec::new();
	}
	const STOPWORDS: &[&str] = &[
		"about", "after", "before", "could", "from", "have", "into", "should", "their", "there",
		"these", "this", "using", "what", "when", "where", "which", "with", "would", "your",
	];
	let patterns_lower: Vec<String> = patterns.iter().map(|p| p.to_lowercase()).collect();
	let mut scored: Vec<(usize, usize)> = lessons
		.iter()
		.enumerate()
		.map(|(i, l)| {
			let haystack = format!(
				"{} {} {}",
				l.title.to_lowercase(),
				l.content.to_lowercase(),
				l.tags.join(" ").to_lowercase()
			);
			let hits = patterns_lower.iter().fold(0usize, |score, pattern| {
				if haystack.contains(pattern.as_str()) {
					return score + 4; // exact phrase or identifier
				}
				let mut terms: Vec<&str> = pattern
					.split(|character: char| !character.is_alphanumeric() && character != '_')
					.filter(|term| term.len() >= 3 && !STOPWORDS.contains(term))
					.collect();
				terms.sort_unstable();
				terms.dedup();
				let term_hits = terms
					.into_iter()
					.filter(|term| haystack.contains(term))
					.count();
				// One generic shared word ("summary", "query", "model") is
				// not sparse evidence. Require two concepts when the exact phrase
				// is absent; asymmetric RRF keeps this fallback secondary to dense.
				score + if term_hits >= 2 { term_hits } else { 0 }
			});
			(hits, i)
		})
		.filter(|(hits, _)| *hits > 0)
		.collect();
	scored.sort_by_key(|b| std::cmp::Reverse(b.0));
	scored.into_iter().map(|(_, i)| i).collect()
}

/// Fold a lesson's chunk vectors into ONE vector by mean-pooling +
/// L2-renormalizing — the standard way to represent a long text as a single
/// embedding. A single chunk (the common case) is already normalized and
/// returned as-is. Mean (not median: a component-wise median of unit vectors is
/// not a meaningful aggregate) is right here because a lesson is single-topic,
/// so its chunks are coherent and the average represents the whole rule.
fn pool_normalize(chunk_vecs: &[&[f32]]) -> Vec<f32> {
	if chunk_vecs.len() == 1 {
		return chunk_vecs[0].to_vec();
	}
	let dim = chunk_vecs.first().map_or(0, |v| v.len());
	let mut acc = vec![0.0_f32; dim];
	for v in chunk_vecs {
		for (a, x) in acc.iter_mut().zip(*v) {
			*a += x;
		}
	}
	let norm: f32 = acc.iter().map(|x| x * x).sum::<f32>().sqrt();
	if norm > 0.0 {
		for a in acc.iter_mut() {
			*a /= norm;
		}
	}
	acc
}

/// Score every memory against the intent under one explicit dense policy.
/// Results are descending and remain one-to-one with memories; admission
/// thresholds belong to the caller because benchmarks need the raw scores.
async fn cosine_scores(
	lessons: &[Lesson],
	intent: &str,
	scoring: DenseScoring,
) -> Result<Vec<(f32, usize)>> {
	// Query gets the same no-truncation treatment as lessons: a within-cap query
	// embeds as one vector — kept on `embed`, which deliberately doesn't persist
	// high-volume per-turn input to the disk cache — while an oversized query is
	// chunked and mean-pooled so its tail isn't lost.
	let intent_chunks =
		crate::embeddings::chunk_to_token_limit(intent, crate::embeddings::EMBED_MAX_INPUT_TOKENS);
	let intent_vec = if intent_chunks.len() > 1 {
		let vecs = crate::embeddings::embed_many(&intent_chunks).await?;
		let refs: Vec<&[f32]> = vecs.iter().map(|v| v.as_slice()).collect();
		pool_normalize(&refs)
	} else {
		let one = intent_chunks.first().map_or(intent, |s| s.as_str());
		crate::embeddings::embed(one).await?
	};

	// Flatten lessons into chunks, remembering which lesson each came from.
	// Short lessons yield one chunk; long ones yield several. No truncation.
	let mut chunk_texts: Vec<String> = Vec::new();
	let mut chunk_owner: Vec<usize> = Vec::new();
	for (i, l) in lessons.iter().enumerate() {
		for chunk in semantic_retrieval_chunks(l, scoring.chunk_tokens) {
			chunk_texts.push(chunk);
			chunk_owner.push(i);
		}
	}
	let chunk_vecs = crate::embeddings::embed_many(&chunk_texts).await?;
	debug_assert_eq!(
		chunk_vecs.len(),
		chunk_owner.len(),
		"chunk vec/owner misalignment"
	);

	// Group each lesson's chunk vectors, then fold to one vector per lesson.
	let mut per_lesson: Vec<Vec<&[f32]>> = vec![Vec::new(); lessons.len()];
	for (owner, v) in chunk_owner.iter().zip(&chunk_vecs) {
		per_lesson[*owner].push(v.as_slice());
	}
	let mut scored: Vec<(f32, usize)> = per_lesson
		.iter()
		.enumerate()
		.filter(|(_, chunks)| !chunks.is_empty())
		.map(|(i, chunks)| {
			let mean_vec = pool_normalize(chunks);
			let mean_score = crate::embeddings::cosine(&intent_vec, &mean_vec);
			let max_score = chunks
				.iter()
				.map(|chunk| crate::embeddings::cosine(&intent_vec, chunk))
				.fold(f32::NEG_INFINITY, f32::max);
			let weight = scoring.max_chunk_weight.clamp(0.0, 1.0);
			(mean_score * (1.0 - weight) + max_score * weight, i)
		})
		.collect();
	scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	Ok(scored)
}

fn semantic_retrieval_chunks(lesson: &Lesson, max_tokens: usize) -> Vec<String> {
	let max_tokens = max_tokens.clamp(32, crate::embeddings::EMBED_MAX_INPUT_TOKENS);
	let metadata = format!("{} {}", lesson.title, lesson.tags.join(" "));
	let mut chunks = Vec::new();
	let mut current = String::new();
	for paragraph in lesson
		.content
		.lines()
		.map(str::trim)
		.filter(|line| !line.is_empty())
	{
		for part in crate::embeddings::chunk_to_token_limit(paragraph, max_tokens) {
			let prospective = if current.is_empty() {
				part.clone()
			} else {
				format!("{current}\n{part}")
			};
			if !current.is_empty() && crate::session::estimate_tokens(&prospective) > max_tokens {
				chunks.push(current);
				current = part;
			} else {
				current = prospective;
			}
		}
	}
	if !current.is_empty() {
		chunks.push(current);
	}
	if chunks.is_empty() {
		return vec![metadata];
	}
	if chunks.len() == 1 {
		return vec![format!(
			"{} {} {}",
			lesson.title,
			lesson.content,
			lesson.tags.join(" ")
		)];
	}
	chunks
		.into_iter()
		.map(|chunk| format!("{metadata}\n{chunk}"))
		.collect()
}

/// Reciprocal Rank Fusion: given multiple ranked lists of indices into
/// the same item set, fuse into a single ranking by summing
/// `1 / (RRF_K + rank)` across methods. Items appearing high in multiple
/// rankings score highest; items appearing in only one method still
/// contribute. Returns `(fused_score, item_index)` sorted by score
/// descending. Items not in any ranking are excluded.
///
/// Reference: Cormack, Clarke & Buettcher, "Reciprocal Rank Fusion
/// outperforms Condorcet and individual rank learning methods" (SIGIR
/// 2009). Used in production by Anthropic Contextual Retrieval and
/// most modern hybrid-search engines.
#[cfg(test)]
fn reciprocal_rank_fusion(total: usize, rankings: &[&[usize]]) -> Vec<(f32, usize)> {
	let weighted: Vec<(&[usize], f32)> = rankings.iter().map(|ranking| (*ranking, 1.0)).collect();
	weighted_reciprocal_rank_fusion(total, &weighted)
}

fn weighted_reciprocal_rank_fusion(
	total: usize,
	rankings: &[(&[usize], f32)],
) -> Vec<(f32, usize)> {
	if total == 0 || rankings.is_empty() {
		return Vec::new();
	}
	let mut scores = vec![0.0_f32; total];
	for (ranking, weight) in rankings {
		for (rank_zero_based, &idx) in ranking.iter().enumerate() {
			if idx < scores.len() {
				// RRF uses 1-indexed rank; +1 to convert from zero-based.
				scores[idx] += weight / (RRF_K + rank_zero_based as f32 + 1.0);
			}
		}
	}
	let mut out: Vec<(f32, usize)> = scores
		.iter()
		.enumerate()
		.map(|(i, s)| (*s, i))
		.filter(|(s, _)| *s > 0.0)
		.collect();
	out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	out
}

#[cfg(test)]
#[path = "file_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "file_benchmark_tests.rs"]
mod benchmark_tests;

#[cfg(test)]
#[path = "longmemeval_benchmark_tests.rs"]
mod longmemeval_benchmark_tests;
