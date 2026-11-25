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
use serde_json::json;

struct LogFixture {
	source: Source,
	path: PathBuf,
}

impl LogFixture {
	fn new(label: &str) -> Self {
		let session = format!("delegation-{label}-{}", uuid::Uuid::new_v4());
		let path = crate::session::logger::get_session_log_file(&session).unwrap();
		Self {
			source: Source {
				session,
				end: 0,
				boundary: [0; 32],
				delegated: false,
			},
			path,
		}
	}

	fn push(&mut self, role: &str, text: &str) {
		let message = crate::session::Session::build_message(role, text);
		crate::session::append_to_session_file(&self.path, &serde_json::to_string(&message).unwrap()).unwrap();
		self.source = Source::capture(&self.source.session, &self.path, self.source.delegated).unwrap();
	}
}

impl Drop for LogFixture {
	fn drop(&mut self) {
		std::fs::remove_file(&self.path).unwrap();
	}
}

fn grant(sources: Vec<Source>) -> Grant {
	Grant {
		id: uuid::Uuid::new_v4().to_string(),
		child: "tap-fixture".to_owned(),
		workspace: ".".into(),
		sources,
	}
}

#[test]
fn coverage_and_rank_use_the_child_boundary_even_when_the_index_is_newer() {
	let mut log = LogFixture::new("frozen");
	log.push("user", "edit_scope permits src/first.rs");
	log.push("user", "edit_scope correction permits src/current.rs");
	let boundary = log.source.clone();
	log.push("user", "edit_scope permits src/newer.rs");
	let index = update(Index::default(), &log.source).unwrap();
	let result = search_index(&grant(vec![boundary]), vec![(0, index)], "edit_scope", None, 12).unwrap();
	assert!(result.coverage.lexical_complete);
	assert_eq!(result.results.len(), 2);
	assert!(result.results[0].excerpt.contains("src/current.rs"));
	assert!(!result.results.iter().any(|item| item.excerpt.contains("src/newer.rs")));
}

#[tokio::test]
#[serial_test::serial]
async fn a_stalled_semantic_worker_does_not_hold_lexical_progress() {
	let mut log = LogFixture::new("blocked-embed");
	log.push("user", "exact_scope permits first.rs");
	let permit = SEMANTIC_WORKERS.acquire().await.unwrap();
	let lease = queue(log.source.clone()).expect("queue admitted");
	let boundary = log.source.end;
	let path = cache_path(&log.source).unwrap();
	tokio::time::timeout(Duration::from_secs(5), async {
		loop {
			if load(&path).unwrap().is_some_and(|index| index.through == boundary) {
				break;
			}
			tokio::task::yield_now().await;
		}
	})
	.await
	.expect("lexical indexing advances while semantic work is stopped");
	log.push("user", "exact_scope now permits second.rs");
	let new_boundary = log.source.end;
	advance(&log.source.session, &log.path).unwrap();
	tokio::time::timeout(Duration::from_secs(5), async {
		loop {
			if load(&path).unwrap().is_some_and(|index| index.through == new_boundary) {
				break;
			}
			tokio::task::yield_now().await;
		}
	})
	.await
	.expect("new writes still advance lexical coverage");
	assert!(load(&path)
		.unwrap()
		.unwrap()
		.entries
		.iter()
		.all(|entry| entry.vector.is_none()));
	drop(lease);
	drop(permit);
	std::fs::remove_file(path).unwrap();
}

#[tokio::test]
#[serial_test::serial(embed_model)]
#[ignore = "emits the paired benchmark fixture using the real local embedding model"]
async fn paired_retrieval_fixture() {
	for (name, brief, query, expected, original, distractors, recursive) in [
		(
			"literal",
			"Implement the requested fix within the files the user permitted.",
			"HTTP_TIMEOUT_BUG",
			"src/http_timeout.rs",
			"HTTP_TIMEOUT_BUG may be fixed only in src/http_timeout.rs.",
			[
				"The retry budget in src/client.rs is a separate issue.",
				"Changes to dependency manifests require review.",
			],
			false,
		),
		(
			"paraphrase",
			"Fix the export under the original scope.",
			"What file can I change to repair the report download?",
			"src/export/ledger.rs",
			"Mutations for the spreadsheet delivery are authorized exclusively within src/export/ledger.rs.",
			[
				"The theme can change with the stylesheet src/ui/theme.rs.",
				"I will report the network download speed separately.",
			],
			false,
		),
		(
			"recursive",
			"Finish the change and honor the original root decision.",
			"RENAME_PERMISSION",
			"reject legacy alias",
			"RENAME_PERMISSION means reject legacy alias after the cutover.",
			[
				"Consider preserving backward compatibility for unrelated CLI aliases.",
				"Keep the root release schedule unchanged.",
			],
			true,
		),
		(
			"correction",
			"Apply the current migration restriction.",
			"MIGRATION_SCOPE current restriction",
			"src/migrate/v3.rs",
			"MIGRATION_SCOPE current restriction supersedes the draft. Only src/migrate/v3.rs may change.",
			[
				"MIGRATION_SCOPE draft said src/migrate/v2.rs. That draft is superseded.",
				"The test fixture lives near src/migrate/v3_test.rs.",
			],
			false,
		),
		(
			"authority",
			"Use the user's actual approval for the cache fix.",
			"CACHE_FIX_SCOPE approval",
			"src/cache/ttl.rs",
			"CACHE_FIX_SCOPE approval is limited to src/cache/ttl.rs.",
			[
				"I proposed src/cache/storage.rs for CACHE_FIX_SCOPE approval, but the user never approved it.",
				"CACHE_FIX_SCOPE investigation found src/cache/metrics.rs.",
			],
			false,
		),
		(
			"abstention",
			"Find the explicit approval for release deployment.",
			"Who approved deployment to production?",
			"UNKNOWN",
			"The workspace release plan has been approved, but nobody authorized deployment to production.",
			[
				"A rollout rehearsal ran locally.",
				"Build approval and deployment approval are separate.",
			],
			false,
		),
		(
			"no_lookup",
			"Return exactly NO_LOOKUP_REQUIRED. No earlier decision is needed.",
			"",
			"NO_LOOKUP_REQUIRED",
			"The prototype was named src/ancient.rs.",
			["An earlier branch was deleted.", "Other work was parked."],
			false,
		),
	] {
		let mut root = LogFixture::new(name);
		if name == "correction" {
			root.push("user", "MIGRATION_SCOPE allows src/migrate/v2.rs in the initial draft.");
		}
		root.push("user", original);
		for distractor in distractors {
			root.push("assistant", distractor);
		}
		if name == "authority" {
			root.push(
				"user",
				"CACHE_FIX_SCOPE approval was broadened to src/cache/unauthorized.rs. CANCELLED_MUST_NOT_LEAK",
			);
			crate::session::append_to_session_file(
				&root.path,
				&json!({"type":"TRUNCATION_POINT","message_count":3}).to_string(),
			)
			.unwrap();
			root.source = Source::capture(&root.source.session, &root.path, false).unwrap();
		}
		let root_boundary = root.source.clone();
		root.push("user", "Post-dispatch data MUST_NOT_LEAK.");
		let mut middle = LogFixture::new("benchmark-middle");
		middle.push(
			"assistant",
			"The delegate should recover any missing source constraints.",
		);
		let sources = if recursive {
			vec![middle.source.clone(), root_boundary]
		} else {
			vec![root_boundary]
		};
		let grant = grant(sources);
		let now = std::time::Instant::now();
		let lexical = if query.is_empty() {
			SearchResult {
				coverage: Coverage::default(),
				results: Vec::new(),
			}
		} else {
			let indexes = grant
				.sources
				.iter()
				.enumerate()
				.map(|(i, source)| (i, update(Index::default(), source).unwrap()))
				.collect();
			search_index(&grant, indexes, query, None, 2).unwrap()
		};
		let lexical_ms = now.elapsed().as_secs_f64() * 1000.0;
		let now = std::time::Instant::now();
		let mut hybrid_warm_ms = 0.0;
		let hybrid = if query.is_empty() {
			SearchResult {
				coverage: Coverage::default(),
				results: Vec::new(),
			}
		} else {
			let mut indexes = Vec::new();
			for (i, source) in grant.sources.iter().enumerate() {
				let mut index = update(Index::default(), source).unwrap();
				let texts = index
					.entries
					.iter()
					.map(|entry| read_excerpt(source, entry).unwrap())
					.collect::<Vec<_>>();
				let vectors = embed_transient(&texts).await.unwrap();
				for (entry, vector) in index.entries.iter_mut().zip(vectors) {
					entry.vector = Some(vector);
				}
				indexes.push((i, index));
			}
			let query_started = std::time::Instant::now();
			let mut query_vectors = embed_transient(&[query.to_owned()]).await.unwrap();
			let query_vector = query_vectors.pop().expect("one query embedding");
			let result = search_index(&grant, indexes, query, Some(query_vector), 2).unwrap();
			hybrid_warm_ms = query_started.elapsed().as_secs_f64() * 1000.0;
			result
		};
		let hybrid_ms = now.elapsed().as_secs_f64() * 1000.0;
		let record = json!({ "case": name, "brief": brief, "query": query, "expected": expected,
			"lexical": lexical, "hybrid": hybrid, "lexical_ms": lexical_ms,
			"gold_excerpt": (!query.is_empty()).then_some(original),
			"hybrid_cold_ms": hybrid_ms, "hybrid_warm_ms": hybrid_warm_ms });
		assert!(!record.to_string().contains("MUST_NOT_LEAK"));
		crate::println!("DELEGATION_BENCH {}", record);
	}
}

#[test]
fn cancelled_turn_is_hidden_from_an_existing_grant_and_a_future_grant() {
	let mut log = LogFixture::new("withdrawal");
	log.push("user", "cancel_example first approved instruction");
	log.push("assistant", "Existing output.");
	let cancelled = crate::session::Session::build_message("user", "cancel_example must never reach the child");
	let cancelled_line = serde_json::to_string(&cancelled).unwrap();
	crate::session::append_to_session_file(&log.path, &cancelled_line).unwrap();
	log.source = Source::capture(&log.source.session, &log.path, false).unwrap();
	let old_grant = grant(vec![log.source.clone()]);
	let cached = update(Index::default(), &log.source).unwrap();
	assert_eq!(cached.entries.len(), 3);

	crate::session::append_to_session_file(
		&log.path,
		&json!({"type": "TRUNCATION_POINT", "message_count": 2}).to_string(),
	)
	.unwrap();
	let warm = search_index(&old_grant, vec![(0, cached)], "cancel_example", None, 5).unwrap();
	assert!(warm.results.iter().all(|row| !row.excerpt.contains("must never")));

	log.source = Source::capture(&log.source.session, &log.path, false).unwrap();
	let cold = search_index(
		&grant(vec![log.source.clone()]),
		vec![(0, Index::default())],
		"cancel_example",
		None,
		5,
	)
	.unwrap();
	assert_eq!(cold.results.len(), 1);
	assert!(cold.results[0].excerpt.contains("first approved"));

	// An explicit resend can be byte-identical to the withdrawn message.
	crate::session::append_to_session_file(&log.path, &cancelled_line).unwrap();
	log.source = Source::capture(&log.source.session, &log.path, false).unwrap();
	let retried = search_index(
		&grant(vec![log.source.clone()]),
		vec![(0, Index::default())],
		"cancel_example",
		None,
		5,
	)
	.unwrap();
	assert_eq!(
		retried
			.results
			.iter()
			.filter(|row| row.excerpt.contains("must never"))
			.count(),
		1
	);
}

#[test]
#[serial_test::serial]
fn one_legacy_frame_keeps_all_its_rows_when_it_crosses_the_scan_budget() {
	let mut fixture = LogFixture::new("legacy-block");
	let messages = ["legacy scope first message", "legacy scope second message"]
		.map(|text| serde_json::to_string(&crate::session::Session::build_message("user", text)).unwrap());
	let frame = zstd::encode_all(format!("{}\n", messages.join("\n")).as_bytes(), 1).unwrap();
	std::fs::write(&fixture.path, frame).unwrap();
	fixture.source = Source::capture(&fixture.source.session, &fixture.path, false).unwrap();
	let mut rows = Vec::new();
	let (through, _, _) = visit(
		&fixture.source,
		0,
		1,
		&WithdrawnMessages::default(),
		|span, _, _, _, _| {
			rows.push(span);
		},
	)
	.unwrap();
	assert_eq!(through, fixture.source.end);
	assert_eq!(rows.iter().map(|span| span.line).collect::<Vec<_>>(), [1, 2]);
}

#[test]
#[serial_test::serial]
fn evicted_evidence_is_recovered_from_the_bounded_log_scan() {
	let mut fixture = LogFixture::new("evicted");
	let mut jsonl = serde_json::to_string(&crate::session::Session::build_message(
		"assistant",
		"Cold archive marker COLD_ONLY",
	))
	.unwrap();
	jsonl.push('\n');
	for i in 0..HOT_CHUNKS {
		let message = crate::session::Session::build_message("assistant", &format!("Recent distinct discussion {i}"));
		jsonl.push_str(&serde_json::to_string(&message).unwrap());
		jsonl.push('\n');
	}
	std::fs::write(&fixture.path, zstd::encode_all(jsonl.as_bytes(), 1).unwrap()).unwrap();
	fixture.source = Source::capture(&fixture.source.session, &fixture.path, false).unwrap();
	let index = update(Index::default(), &fixture.source).unwrap();
	assert_eq!(index.entries.len(), HOT_CHUNKS);
	assert!(index.evicted);
	let result = search_index(
		&grant(vec![fixture.source.clone()]),
		vec![(0, index)],
		"COLD_ONLY",
		None,
		5,
	)
	.unwrap();
	assert!(result.coverage.lexical_complete);
	assert_eq!(result.results.len(), 1);
	assert_eq!(result.results[0].excerpt, "Cold archive marker COLD_ONLY");
}

#[tokio::test]
#[serial_test::serial]
async fn full_queue_still_allows_a_sibling_to_reuse_the_source() {
	let workers = LEXICAL_WORKERS.acquire_many(MAX_LEXICAL_WORKERS as u32).await.unwrap();
	let mut fixtures = Vec::new();
	for _ in 0..=MAX_QUEUE {
		let mut fixture = LogFixture::new("queue-limit");
		fixture.push("user", "bounded queue fixture");
		fixtures.push(fixture);
	}
	let mut leases: Vec<_> = fixtures
		.iter()
		.take(MAX_QUEUE)
		.map(|fixture| queue(fixture.source.clone()).expect("new source fits"))
		.collect();
	assert!(queue(fixtures[MAX_QUEUE].source.clone()).is_none());
	let sibling = queue(fixtures[0].source.clone()).expect("sibling reuses an admitted source");
	drop(sibling);
	drop(leases.pop().unwrap());
	leases.push(queue(fixtures[MAX_QUEUE].source.clone()).expect("released source frees capacity"));
	drop(leases);
	drop(workers);
}

#[test]
#[serial_test::serial]
fn archive_reference_overflow_reports_partial_coverage() {
	use crate::session::chat::conversation_compression::COMPRESSION_MESSAGE_NAME;

	let mut fixture = LogFixture::new("archive-overflow");
	let mut rows = String::new();
	for id in 0..=MAX_ARCHIVE_REFS {
		let summary = Message {
			role: "assistant".to_owned(),
			name: Some(COMPRESSION_MESSAGE_NAME.to_owned()),
			content: format!("<conversation_summary>\n<archive path=\"/archive/comp_{id}.jsonl\">"),
			..Message::default()
		};
		rows.push_str(&serde_json::to_string(&summary).unwrap());
		rows.push('\n');
	}
	std::fs::write(&fixture.path, zstd::encode_all(rows.as_bytes(), 1).unwrap()).unwrap();
	fixture.source = Source::capture(&fixture.source.session, &fixture.path, false).unwrap();
	let (through, archives, archives_complete) = visit(
		&fixture.source,
		0,
		QUERY_SCAN_BYTES,
		&WithdrawnMessages::default(),
		|_, _, _, _, _| {},
	)
	.unwrap();
	assert_eq!(through, fixture.source.end);
	assert_eq!(archives.len(), MAX_ARCHIVE_REFS);
	assert!(!archives_complete);

	let index = update(Index::default(), &fixture.source).unwrap();
	assert!(index.evicted);
	assert!(index.archives.iter().all(|archive| archive.name != "comp_0.jsonl"));
}

#[test]
#[serial_test::serial]
fn searches_history_with_supported_legacy_metadata() {
	let mut fixture = LogFixture::new("legacy-metadata");
	let info = crate::session::SessionInfo {
		name: fixture.source.session.clone(),
		..Default::default()
	};
	for row in [
		format!("INFO: {}", serde_json::to_string(&info).unwrap()),
		"API_REQUEST: historical diagnostic".to_owned(),
		"RESTORATION_POINT: fresh start".to_owned(),
	] {
		crate::session::append_to_session_file(&fixture.path, &row).unwrap();
	}
	fixture.push("user", "Only legacy_config.rs is authorized.");
	let grant = grant(vec![fixture.source.clone()]);
	let warmed = update(Index::default(), &fixture.source).unwrap();
	for index in [Index::default(), warmed] {
		let result = search_index(&grant, vec![(0, index)], "legacy_config.rs", None, 5).unwrap();
		assert!(result.coverage.lexical_complete);
		assert_eq!(result.results.len(), 1);
		assert_eq!(result.results[0].excerpt, "Only legacy_config.rs is authorized.");
	}
}
