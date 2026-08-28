// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License")

use super::super::Lesson;
use super::LearningBackend;
use crate::config::Config;
use anyhow::Result;
use async_trait::async_trait;
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

#[async_trait]
impl LearningBackend for FileBackend {
	async fn store(&self, lesson: &Lesson, _config: &Config) -> Result<()> {
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

	async fn retrieve(
		&self,
		intent: &str,
		patterns: &[String],
		role: &str,
		project: &str,
		limit: usize,
		config: &Config,
	) -> Result<Vec<Lesson>> {
		let dir = Self::learning_dir(role, project)?;
		if !dir.exists() {
			return Ok(Vec::new());
		}

		let all = self.retrieve_all(role, project, config).await?;
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
			match rank_by_cosine(&all, intent).await {
				Ok(r) => r,
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

	async fn delete(&self, id: &str, role: &str, project: &str, _config: &Config) -> Result<()> {
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

	async fn retrieve_all(
		&self,
		role: &str,
		project: &str,
		_config: &Config,
	) -> Result<Vec<Lesson>> {
		let dir = Self::learning_dir(role, project)?;
		Ok(Self::read_lessons_sorted(&dir))
	}

	async fn retrieve_global(&self, _config: &Config) -> Result<Vec<Lesson>> {
		let dir = crate::directories::get_global_learning_dir()?;
		Ok(Self::read_lessons_sorted(&dir))
	}

	async fn retrieve_archived_global(
		&self,
		intent: &str,
		patterns: &[String],
		limit: usize,
		_config: &Config,
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
	async fn has_lessons(&self, role: &str, project: &str, _config: &Config) -> bool {
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

	async fn reinforce(
		&self,
		content: &str,
		role: &str,
		project: &str,
		delta: f64,
		config: &Config,
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
			self.store(&entry, config).await?;
			super::super::retention::archive_record(&entry)?;
			crate::supervisor::stats::memory_retention(0, 1);
			crate::log_debug!(
				"Reinforce: cold-archived (importance {:.2}): {}",
				new_importance,
				entry.content
			);
		} else {
			entry.importance = new_importance;
			self.store(&entry, config).await?; // same file_id → overwrites in place
			crate::log_debug!(
				"Reinforce: {} importance -> {:.2}",
				entry.content,
				new_importance
			);
		}
		Ok(())
	}

	async fn prune_stale(
		&self,
		role: &str,
		project: &str,
		decay_days: u64,
		_config: &Config,
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

/// Rank lessons by MiniLM-L6 cosine vs the user intent (descending). Each lesson
/// becomes exactly one vector — embedded directly if it fits the cap, or
/// recursively chunked and mean-pooled if oversized, so no text is lost while
/// ranking stays 1-to-1. Lessons with cosine ≤ 0.2 are excluded as noise.
/// Returns indices into the input slice.
async fn rank_by_cosine(lessons: &[Lesson], intent: &str) -> Result<Vec<usize>> {
	Ok(score_by_cosine(lessons, intent)
		.await?
		.into_iter()
		.filter(|(score, _)| *score > COSINE_FLOOR)
		.map(|(_, index)| index)
		.collect())
}

async fn score_by_cosine(lessons: &[Lesson], intent: &str) -> Result<Vec<(f32, usize)>> {
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
		let combined = format!("{} {} {}", l.title, l.content, l.tags.join(" "));
		for chunk in crate::embeddings::chunk_to_token_limit(
			&combined,
			crate::embeddings::EMBED_MAX_INPUT_TOKENS,
		) {
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
			let vec = pool_normalize(chunks);
			(crate::embeddings::cosine(&intent_vec, &vec), i)
		})
		.collect();
	scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	Ok(scored)
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
mod tests {
	use super::*;

	#[test]
	fn test_escaping_roundtrip() {
		// `unescape` is the exact inverse of `escape` on adversarial inputs —
		// backslashes, quotes, the trailing-quote case, newlines, and already-
		// escaped-looking sequences. This is the store/parse cycle that
		// `reinforce` runs on every importance bump, so any asymmetry compounds.
		for s in [
			"plain",
			"has \"quotes\"",
			"back\\slash",
			"trailing quote\\\"",
			"embedded\nnewline",
			"C:\\tmp and a \" quote",
			"\\\\already\\\\escaped",
		] {
			assert_eq!(unescape(&escape(s)), s, "round-trip failed for {s:?}");
		}

		// A full record whose quoted fields contain a quote and a newline
		// survives the store-format -> parse cycle without corruption or the
		// newline truncating the line-based parser.
		let title = "say \"hi\"\nthen bye";
		let content = "path C:\\tmp and a \" quote";
		let project = "weird\\proj\"name";
		let record = format!(
			"---\ntitle: \"{}\"\ncontent: \"{}\"\nmemory_type: learning\nimportance: 0.5\nconfidence: high\ntags: []\nsource: \"s\"\nrole: \"r\"\nproject: \"{}\"\nscope: scoped\ncreated: \"c\"\n---\n",
			escape(title),
			escape(content),
			escape(project),
		);
		let lesson = FileBackend::parse_lesson_file(&record).unwrap();
		assert_eq!(lesson.title, title);
		assert_eq!(lesson.content, content);
		assert_eq!(lesson.project, project);
	}

	#[test]
	fn test_parse_lesson_file_valid() {
		let content = r#"---
content: "Bearer token auth required"
memory_type: learning
importance: 0.8
confidence: high
tags: [auth, api]
source: "test-session"
role: "developer"
project: "octofs"
created: "2026-04-05T14:30:00Z"
related: ["memory-a","memory-b"]
evidence: ["session://test/message/2"]
outcome: failed
last_used: "2026-08-28T00:00:00Z"
use_count: 7
---
"#;
		let lesson = FileBackend::parse_lesson_file(content).unwrap();
		assert_eq!(lesson.content, "Bearer token auth required");
		assert_eq!(lesson.importance, 0.8);
		assert_eq!(lesson.confidence, "high");
		assert_eq!(lesson.tags, vec!["auth", "api"]);
		assert_eq!(lesson.role, "developer");
		assert_eq!(lesson.project, "octofs");
		assert_eq!(lesson.related, vec!["memory-a", "memory-b"]);
		assert_eq!(lesson.evidence, vec!["session://test/message/2"]);
		assert_eq!(lesson.last_used, "2026-08-28T00:00:00Z");
		assert_eq!(lesson.use_count, 7);
		assert_eq!(
			lesson.outcome,
			crate::supervisor::learning::TrajectoryOutcome::Failed
		);
	}

	#[test]
	fn test_parse_lesson_file_missing_frontmatter() {
		let content = "Just some text without frontmatter";
		assert!(FileBackend::parse_lesson_file(content).is_none());
	}

	#[test]
	fn test_parse_lesson_file_empty_content() {
		let content = r#"---
memory_type: learning
importance: 0.5
---
"#;
		// content field is empty -> should return None
		assert!(FileBackend::parse_lesson_file(content).is_none());
	}

	#[test]
	fn test_file_id() {
		let lesson = Lesson {
			content: "Bearer token auth".into(),
			created: "2026-04-05T14:30:00Z".into(),
			..Default::default()
		};
		assert_eq!(lesson.file_id(), "20260405143000-bearer-token-auth");

		let empty = Lesson {
			content: "!!!".into(),
			created: "2026-04-05T14:30:00Z".into(),
			..Default::default()
		};
		// No alphanumerics → slug empty → id is just the timestamp.
		assert_eq!(empty.file_id(), "20260405143000");
	}

	#[tokio::test]
	async fn test_store_and_retrieve_all() {
		let dir = tempfile::tempdir().unwrap();
		let role = "developer";
		let project = "test_proj";

		// Create learning dir manually for test
		let learning_dir = dir.path().join("learning").join(project).join(role);
		std::fs::create_dir_all(&learning_dir).unwrap();

		// Write a lesson file directly
		let content = r#"---
content: "Always use bearer tokens"
memory_type: learning
importance: 0.8
confidence: high
tags: [auth]
source: "test"
role: "developer"
project: "test_proj"
created: "2026-04-05T00:00:00Z"
---
"#;
		std::fs::write(learning_dir.join("20260405-bearer-tokens.md"), content).unwrap();

		// Read it back
		let mut lessons = Vec::new();
		for entry in std::fs::read_dir(&learning_dir).unwrap() {
			let entry = entry.unwrap();
			if let Ok(file_content) = std::fs::read_to_string(entry.path()) {
				if let Some(lesson) = FileBackend::parse_lesson_file(&file_content) {
					lessons.push(lesson);
				}
			}
		}

		assert_eq!(lessons.len(), 1);
		assert_eq!(lessons[0].content, "Always use bearer tokens");
		assert_eq!(lessons[0].confidence, "high");
	}

	#[test]
	fn test_pattern_matching() {
		let lesson = Lesson {
			content: "Bearer token auth is required for API endpoints".into(),
			tags: vec!["auth".into(), "api".into()],
			..Default::default()
		};

		let text = lesson.content.to_lowercase();
		let tags_text = lesson.tags.join(" ").to_lowercase();
		let combined = format!("{} {}", text, tags_text);

		assert!(combined.contains("bearer"));
		assert!(combined.contains("auth"));
		assert!(combined.contains("api"));
		assert!(!combined.contains("database"));
	}

	// ----------------------------------------------------------------------
	// Pure-logic tests for RRF + keyword ranking. These cover the fusion
	// math without touching the embedding model.
	// ----------------------------------------------------------------------

	fn lesson_with(content: &str, tags: &[&str]) -> Lesson {
		Lesson {
			content: content.to_string(),
			tags: tags.iter().map(|s| s.to_string()).collect(),
			..Default::default()
		}
	}

	#[test]
	fn rank_by_keywords_returns_empty_when_no_patterns() {
		let lessons = vec![lesson_with("anything", &[])];
		assert!(rank_by_keywords(&lessons, &[]).is_empty());
	}

	#[test]
	fn rank_by_keywords_excludes_lessons_with_zero_hits() {
		let lessons = vec![
			lesson_with("postgres slow query", &["db"]),
			lesson_with("filesystem read", &["files"]),
		];
		let ranking = rank_by_keywords(&lessons, &["postgres".to_string()]);
		// Only the postgres lesson hits; filesystem lesson is excluded.
		assert_eq!(ranking, vec![0]);
	}

	#[test]
	fn rank_by_keywords_orders_by_hit_count_descending() {
		let lessons = vec![
			lesson_with("postgres", &[]),                // 1 hit
			lesson_with("postgres slow query", &["db"]), // 2 hits (postgres, query)
			lesson_with("just a note", &[]),             // 0 hits — excluded
		];
		let ranking = rank_by_keywords(&lessons, &["postgres".to_string(), "query".to_string()]);
		// Lesson 1 (2 hits) ranks before lesson 0 (1 hit); lesson 2 excluded.
		assert_eq!(ranking, vec![1, 0]);
	}

	#[test]
	fn rank_by_keywords_is_case_insensitive() {
		let lessons = vec![lesson_with("PostgreSQL EXPLAIN ANALYZE", &[])];
		let ranking = rank_by_keywords(&lessons, &["postgresql".to_string()]);
		assert_eq!(ranking, vec![0]);
	}

	#[test]
	fn rank_by_keywords_uses_phrase_terms_as_weak_fallback() {
		let lessons = vec![
			lesson_with(
				"validate callback state and retain the PKCE verifier",
				&["oauth"],
			),
			lesson_with("unrelated callback rendering", &["ui"]),
		];
		let ranking = rank_by_keywords(
			&lessons,
			&["state parameter".to_string(), "oauth callback".to_string()],
		);
		assert_eq!(ranking.first().copied(), Some(0));
	}

	#[test]
	fn importance_rerank_is_bounded_and_neutral_at_half() {
		assert_eq!(importance_factor(-1.0), 0.75);
		assert_eq!(importance_factor(0.5), 1.0);
		assert_eq!(importance_factor(2.0), 1.25);
	}

	#[test]
	fn ranked_retrieval_expands_forward_links_and_backlinks_once() {
		let mut root = lesson_with("root memory", &[]);
		root.created = "2026-01-01T00:00:01Z".to_string();
		let mut target = lesson_with("target memory", &[]);
		target.created = "2026-01-01T00:00:02Z".to_string();
		let mut backlink = lesson_with("backlink memory", &[]);
		backlink.created = "2026-01-01T00:00:03Z".to_string();
		root.related.push(target.file_id());
		backlink.related.push(root.file_id());
		let all = vec![root, target, backlink];

		let expanded = expand_ranked_with_links(&all, &[(1.0, 0)], 3);
		assert_eq!(
			expanded
				.iter()
				.map(|memory| memory.content.as_str())
				.collect::<Vec<_>>(),
			vec!["root memory", "target memory", "backlink memory"]
		);
	}

	#[test]
	fn rrf_returns_empty_for_empty_inputs() {
		let empty: Vec<&[usize]> = Vec::new();
		assert!(reciprocal_rank_fusion(0, &empty).is_empty());
		assert!(reciprocal_rank_fusion(5, &empty).is_empty());
	}

	#[test]
	fn rrf_single_ranker_preserves_order() {
		// With one ranker, RRF is just rank order with smaller scores
		// further down — fused order should equal input order.
		let r = vec![2usize, 0, 1];
		let fused = reciprocal_rank_fusion(3, &[&r]);
		let order: Vec<usize> = fused.iter().map(|(_, i)| *i).collect();
		assert_eq!(order, vec![2, 0, 1]);
	}

	#[test]
	fn rrf_excludes_items_not_in_any_ranking() {
		// Only items 0 and 2 appear; item 1 should be absent from output.
		let r = vec![0usize, 2];
		let fused = reciprocal_rank_fusion(3, &[&r]);
		let indices: Vec<usize> = fused.iter().map(|(_, i)| *i).collect();
		assert_eq!(indices, vec![0, 2]);
		assert!(!indices.contains(&1));
	}

	#[test]
	fn rrf_promotes_items_appearing_in_multiple_rankings() {
		// Item 0 ranks #2 in keyword and #1 in cosine (mid-rank in both).
		// Item 1 ranks #1 in keyword and not at all in cosine.
		// Item 2 ranks #3 in keyword and #2 in cosine.
		// Item 0 should win because it appears in BOTH rankings —
		// even though item 1 is keyword-#1, missing from cosine drops it.
		let keyword = vec![1usize, 0, 2];
		let cosine = vec![0usize, 2];
		let fused = reciprocal_rank_fusion(3, &[&keyword, &cosine]);
		let top = fused.first().expect("at least one fused result");
		assert_eq!(top.1, 0, "item 0 should win — present in both rankings");
	}

	#[test]
	fn rrf_top_rank_in_both_dominates() {
		// If an item is rank #1 in both, it must score highest.
		let r1 = vec![5usize, 0, 1];
		let r2 = vec![5usize, 1, 0];
		let fused = reciprocal_rank_fusion(6, &[&r1, &r2]);
		assert_eq!(fused.first().unwrap().1, 5);
	}

	#[test]
	fn weighted_rrf_keeps_dense_correction_ahead_of_sparse_stale_match() {
		let sparse = vec![1usize, 0]; // stale phrasing matches first
		let dense = vec![0usize, 2]; // current rule is semantically strongest
		let fused =
			weighted_reciprocal_rank_fusion(3, &[(&sparse, KEYWORD_RRF_WEIGHT), (&dense, 1.0)]);
		assert_eq!(fused.first().unwrap().1, 0);

		// When embeddings are absent, keyword weighting is uniform and cannot
		// disturb its original exact-match order.
		let sparse_only = weighted_reciprocal_rank_fusion(3, &[(&sparse, 1.0)]);
		assert_eq!(
			sparse_only.iter().map(|item| item.1).collect::<Vec<_>>(),
			sparse
		);
	}

	#[test]
	fn adaptive_keyword_weight_changes_only_conflict_contaminated_queries() {
		let mut lessons = vec![lesson_with("current", &[]), lesson_with("stale", &[])];
		lessons[0].importance = 0.9;
		lessons[1].importance = 0.2;
		assert_eq!(
			adaptive_keyword_weight(&[1, 0], &[0], &lessons),
			KEYWORD_RRF_WEIGHT
		);
		lessons[1].importance = 0.5;
		assert_eq!(adaptive_keyword_weight(&[1, 0], &[0], &lessons), 1.0);
		assert_eq!(adaptive_keyword_weight(&[1, 0], &[], &lessons), 1.0);
	}

	#[test]
	fn sparse_rescue_uses_rank_five_and_prefers_outcome_importance() {
		let mut lessons = vec![
			lesson_with("current callback state verifier", &["oauth"]),
			lesson_with("stale callback state", &["oauth"]),
		];
		lessons[0].importance = 0.9;
		lessons[1].importance = 0.2;
		lessons.extend((0..6).map(|index| lesson_with(&format!("dense {index}"), &[])));
		let sparse = vec![1usize, 0];
		let mut ranked = (2..8)
			.enumerate()
			.map(|(rank, index)| (1.0 - rank as f32 / 10.0, index))
			.chain([(0.01, 1), (0.009, 0)])
			.collect::<Vec<_>>();
		promote_sparse_rescue(&mut ranked, &sparse, &lessons);
		assert_eq!(ranked[4].1, 0);
		assert_ne!(ranked[0].1, 0);
	}

	#[test]
	fn rrf_uses_one_indexed_ranks() {
		// With k=60, item at rank-0 (1-indexed: 1) scores 1/(60+1) = 1/61
		// across one ranker. Item at rank-1 scores 1/62. Verify the math.
		let r = vec![3usize, 7];
		let fused = reciprocal_rank_fusion(8, &[&r]);
		let by_idx: std::collections::HashMap<usize, f32> =
			fused.into_iter().map(|(s, i)| (i, s)).collect();
		let s_first = by_idx[&3];
		let s_second = by_idx[&7];
		let expected_first = 1.0_f32 / (RRF_K + 1.0);
		let expected_second = 1.0_f32 / (RRF_K + 2.0);
		assert!(
			(s_first - expected_first).abs() < 1e-6,
			"rank-1 score should be 1/61 ({}), got {}",
			expected_first,
			s_first
		);
		assert!(
			(s_second - expected_second).abs() < 1e-6,
			"rank-2 score should be 1/62 ({}), got {}",
			expected_second,
			s_second
		);
		assert!(s_first > s_second, "rank-1 must outscore rank-2");
	}
}

#[cfg(test)]
#[path = "file_benchmark_tests.rs"]
mod benchmark_tests;

#[cfg(test)]
#[path = "longmemeval_benchmark_tests.rs"]
mod longmemeval_benchmark_tests;
