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

//! Retrieval-and-active-pack tests against the real file backend under a
//! throwaway role/project scope. Follow-up-call retrieval is LLM-free
//! (keyword/recency ranking), so most of the flow runs with no stub; the
//! first-call keyword query rides the scripted fake provider.

use super::*;
use crate::session::chat::test_support::{fake_provider_config, final_response, spawn_stub, ENV_LOCK};
use crate::supervisor::learning::backend::FileBackend;
use crate::supervisor::learning::Lesson;
use serial_test::serial;

const ROLE: &str = "__inject_test_role";

struct TestDataDir {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl TestDataDir {
	fn new() -> Self {
		let dir = tempfile::tempdir().expect("temporary data dir");
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self { previous, _dir: dir }
	}
}

impl Drop for TestDataDir {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(value) => std::env::set_var("OCTOMIND_DATA_DIR", value),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

fn cleanup(proj: &str) {
	if let Ok(dir) = crate::directories::get_learning_dir(ROLE, proj) {
		let _ = std::fs::remove_dir_all(dir);
	}
}

fn lesson(proj: &str, content: &str, memory_type: &str, tags: &[&str]) -> Lesson {
	Lesson {
		content: content.to_string(),
		title: String::new(),
		memory_type: memory_type.to_string(),
		importance: 0.8,
		confidence: "high".to_string(),
		tags: tags.iter().map(|t| t.to_string()).collect(),
		source: "inject-test".to_string(),
		role: ROLE.to_string(),
		project: proj.to_string(),
		scope: "scoped".to_string(),
		created: chrono::Utc::now().to_rfc3339(),
		..Default::default()
	}
}

/// Keep the sender alive across the call — a dropped sender reads as a
/// cancelled operation to the LLM-call cancellation wrapper.
fn cancel_pair() -> (tokio::sync::watch::Sender<bool>, tokio::sync::watch::Receiver<bool>) {
	tokio::sync::watch::channel(false)
}

#[serial]
#[tokio::test]
async fn test_followup_retrieval_injects_and_dedupes() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let proj = "__inject_test_proj_followup";
	cleanup(proj);
	let config = fake_provider_config();
	let backend = FileBackend;
	let mut rule = lesson(
		proj,
		"always run the test suite on the dev box",
		"learning",
		&["testing", "box"],
	);
	rule.user_rule = Some(crate::supervisor::learning::mod_tests::user_rule(
		&rule,
		"Always run the test suite on the dev box.",
	));
	backend.store(&rule).await.expect("store lesson");
	// Legacy record: `learning` type but no grounded provenance handle.
	backend
		.store(&lesson(
			proj,
			"skip the linter on hotfix branches",
			"learning",
			&["lint"],
		))
		.await
		.expect("store legacy lesson");
	backend
		.store(&lesson(
			proj,
			"the build uses a cargo workspace",
			"orientation",
			&["build"],
		))
		.await
		.expect("store orientation");

	// Follow-up call (first_call=false): no LLM. An empty
	// user input takes the deterministic embedding-free branch (plain
	// recency listing); a non-empty one would need the MiniLM warmup, which
	// only other tests happen to trigger — never depend on that here.
	let (_tx1, rx1) = cancel_pair();
	let (text, injected_now) = retrieve_and_format(&config, "", ROLE, proj, false, rx1).await.unwrap();
	let dir = crate::directories::get_learning_dir(ROLE, proj).expect("dir");
	let files: Vec<String> = std::fs::read_dir(&dir)
		.map(|entries| {
			entries
				.filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
				.collect()
		})
		.unwrap_or_default();
	let all = backend.retrieve_all(ROLE, proj).await.unwrap_or_default();
	assert!(
		text.contains("always run the test suite on the dev box"),
		"lesson missing from recall block:\ntext={text:?}\nstore dir {dir:?} files={files:?}\nretrieve_all={}",
		all.len()
	);
	assert!(
		text.contains("<active_memory_pack "),
		"missing active memory wrapper:\n{text}"
	);
	assert!(text.contains("<orientation"), "orientation block missing:\n{text}");
	// The verified user rule carries user authority, outside the data-only wrapper;
	// the legacy rule stays contextual inside it.
	let rules_end = text.find("</user_rules>").expect("user_rules block missing");
	let wrapper_at = text.find("<active_memory_pack ").unwrap();
	let rule_at = text.find("always run the test suite on the dev box").unwrap();
	assert!(rule_at < rules_end, "user rule rendered outside user_rules:\n{text}");
	assert!(rules_end < wrapper_at);
	assert!(text.contains("Original user quote: Always run the test suite on the dev box."));
	assert_eq!(
		injected_now
			.iter()
			.find(|item| item.content == rule.content)
			.unwrap()
			.user_rule,
		rule.user_rule
	);
	let legacy_at = text
		.find("skip the linter on hotfix branches")
		.expect("legacy lesson missing");
	assert!(legacy_at > wrapper_at, "legacy rule gained user authority:\n{text}");
	assert!(text.contains("<lessons>"));
	assert!(!injected_now.is_empty());

	// A replacement pack may deliberately select the same relevant memory again;
	// it replaces the previous pack instead of accumulating beside it.
	let (_tx2, rx2) = cancel_pair();
	let (text2, selected2) = retrieve_and_format(&config, "", ROLE, proj, false, rx2).await.unwrap();
	assert!(text2.contains("always run the test suite on the dev box"));
	assert!(!selected2.is_empty());
	assert!(
		crate::session::estimate_tokens(&text2) <= MAX_MEMORY_PACK_TOKENS,
		"active pack exceeded token budget"
	);

	cleanup(proj);
}

#[serial]
#[tokio::test]
async fn test_first_call_retrieval_uses_keyword_query() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let proj = "__inject_test_proj_first";
	cleanup(proj);
	let mut config = fake_provider_config();
	config.supervisor.model.model = Some("ollama:fake-model".to_string());
	let backend = FileBackend;
	backend
		.store(&lesson(
			proj,
			"prefer rsync over scp for box deployments",
			"learning",
			&["deploy", "rsync"],
		))
		.await
		.expect("store lesson");

	// First call: the keyword-query model call happens against the stub.
	let url = spawn_stub(vec![final_response("deploy\nrsync\nbox\n")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let (_tx3, rx3) = cancel_pair();
	let (text, _) = retrieve_and_format(&config, "deploy the service to the box", ROLE, proj, true, rx3)
		.await
		.unwrap();
	assert!(
		text.contains("prefer rsync over scp"),
		"scoped lesson missing on first call:\n{text}"
	);

	std::env::remove_var("OLLAMA_API_URL");
	cleanup(proj);
}

#[tokio::test]
async fn test_disabled_learning_injects_nothing() {
	let mut config = fake_provider_config();
	config.supervisor.learning.enabled = false;
	let (_tx4, rx4) = cancel_pair();
	let (text, injected) = retrieve_and_format(&config, "anything", ROLE, "__inject_test_proj_disabled", true, rx4)
		.await
		.unwrap();
	assert!(text.is_empty());
	assert!(injected.is_empty());
}

#[serial]
#[tokio::test]
async fn test_active_pack_is_token_bounded_with_stable_pack_ids() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let proj = "__inject_test_proj_budget";
	cleanup(proj);
	let config = fake_provider_config();
	let backend = FileBackend;
	for i in 0..20 {
		backend
			.store(&lesson(
				proj,
				&format!("memory {i}: {}", "long reusable context ".repeat(120)),
				"learning",
				&["memory"],
			))
			.await
			.expect("store lesson");
	}

	let (_tx, rx) = cancel_pair();
	let (text, selected) = retrieve_and_format(&config, "", ROLE, proj, false, rx).await.unwrap();
	assert!(!selected.is_empty());
	assert!(selected.len() < 20, "long memories must be budget-pruned");
	assert!(crate::session::estimate_tokens(&text) <= MAX_MEMORY_PACK_TOKENS);
	for (index, memory) in selected.iter().enumerate() {
		assert_eq!(memory.id, format!("M{}", index + 1));
		assert!(text.contains(&format!("[{} ", memory.id)));
	}

	cleanup(proj);
}

#[serial]
#[tokio::test]
async fn test_long_experience_injects_card_with_full_file_reference() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let proj = "__inject_test_proj_experience";
	cleanup(proj);
	let config = fake_provider_config();
	let backend = FileBackend;
	let mut experience = lesson(
		proj,
		&format!(
			"## Objective\nRecover provider continuation safely.\n\n## Durable knowledge\n{}\n\n## Outcome and evidence\nVerified by the provider error and successful retry.\n\n## Reuse conditions\nUse only for invalid continuation failures.",
			"Keep the resolved provider identity stable and clear only the invalid continuation. ".repeat(180)
		),
		"experience",
		&["provider", "continuation"],
	);
	experience.title = "Provider continuation recovery".to_string();
	experience.outcome = crate::supervisor::learning::TrajectoryOutcome::Verified;
	experience.related = vec!["related-memory-id".to_string()];
	experience.evidence = vec!["session://s/message/3".to_string()];
	backend.store(&experience).await.expect("store experience");

	let (_tx, rx) = cancel_pair();
	let (text, selected) = retrieve_and_format(&config, "", ROLE, proj, false, rx).await.unwrap();
	assert!(text.contains("<experiences>"));
	assert!(text.contains("Provider continuation recovery"));
	assert!(text.contains("full memory:"));
	assert!(text.contains("related-memory-id"));
	assert!(text.contains("session://s/message/3"));
	assert!(selected.iter().any(|memory| memory.content == experience.content));
	assert!(crate::session::estimate_tokens(&text) <= MAX_MEMORY_PACK_TOKENS);

	cleanup(proj);
}

#[test]
fn experience_card_names_only_the_live_skills_it_is_given() {
	let experience = Lesson {
		content: "body".to_string(),
		title: "Schema migration".to_string(),
		memory_type: "experience".to_string(),
		artifacts: vec!["evo-schema".to_string()],
		..Default::default()
	};
	let card = render_item("M1", &experience, false, None, &["evolved-schema".to_string()]);
	assert!(card.contains(" skills=evolved-schema] Schema migration"));
	assert!(!render_item("M1", &experience, false, None, &[]).contains("skills="));
	// Outside a session nothing is applied, so a stored harness names nothing.
	assert!(crate::supervisor::learning::evolution::applied_skill_names(&experience.artifacts).is_empty());
}

#[test]
fn retrieval_rewrite_validation_accepts_keywords_and_rejects_answers() {
	let valid = validate_retrieval_patterns(
		"oauth callback\npkce verifier\nstate parameter\nsession fixation\ncsrf protection",
	)
	.expect("valid keyword rewrite");
	assert_eq!(valid.len(), 5);
	assert!(validate_retrieval_patterns(
		"Good morning!\nThis is the answer to the user's request.\ntranslation\nicelandic\ngreeting\nlanguage"
	)
	.is_err());
	assert!(validate_retrieval_patterns("one\ntwo").is_err());
}

// ---------------------------------------------------------------------------
// Evaluation gate (decision.recall): the relevance filter between
// ranking and admission. `install_fake_evaluation` holds ENV_LOCK itself.
// ---------------------------------------------------------------------------

use crate::session::chat::test_support::{evaluate_counter, install_fake_evaluation, nouls, FakeEvaluationStep};
use crate::supervisor::evaluate::Seam;

fn recall_gate_config() -> crate::config::Config {
	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.decision.recall = true;
	config
}

#[serial]
#[tokio::test]
async fn recall_gate_drops_unrelated_scoped_candidates_and_keeps_rank_order() {
	let fake = install_fake_evaluation(vec![FakeEvaluationStep::Answers(nouls(&[
		("c0", 0.91),
		("c1", 0.08),
		("c2", 0.74),
	]))])
	.await;
	let config = recall_gate_config();
	let proj = "__inject_test_proj_gate";
	let applied = evaluate_counter(Seam::Recall, "applied");
	let mut candidates = vec![(lesson(proj, "global rule: always run fmt", "learning", &[]), true)];
	for content in ["reqwest retry pattern", "svelte store naming", "http timeout defaults"] {
		candidates.push((lesson(proj, content, "learning", &[]), false));
	}
	let request = "add retry with backoff to the http client";
	let kept = filter_scoped_by_relevance(&config, request, candidates).await;
	let contents: Vec<&str> = kept.iter().map(|(l, _)| l.content.as_str()).collect();
	assert_eq!(
		contents,
		[
			"global rule: always run fmt",
			"reqwest retry pattern",
			"http timeout defaults"
		]
	);
	assert_eq!(evaluate_counter(Seam::Recall, "applied"), applied + 1);

	// The state holds the request and the scoped candidates only: no global
	// lesson, no tool result, no assistant text.
	let state = fake.requests()[0].state.clone();
	let mut keys: Vec<&String> = state.as_object().unwrap().keys().collect();
	keys.sort();
	assert_eq!(keys, ["candidates", "request"]);
	assert_eq!(state["request"], request);
	assert!(!state.to_string().contains("global rule"));
	let sent = state["candidates"].as_array().unwrap();
	assert_eq!(sent.len(), 3);
	assert_eq!(sent[1]["index"], 1);
	assert_eq!(sent[1]["content"], "svelte store naming");
	assert_eq!(fake.requests()[0].questions.len(), 3);
}

#[serial]
#[tokio::test]
async fn recall_gate_keeps_a_candidate_the_model_refused() {
	let mut answers = nouls(&[("c0", 0.91), ("c2", 0.08)]);
	answers.extend(crate::session::chat::test_support::refusals(&["c1"]));
	let _fake = install_fake_evaluation(vec![FakeEvaluationStep::Answers(answers)]).await;
	let config = recall_gate_config();
	let proj = "__inject_test_proj_gate_refusal";
	let candidates: Vec<_> = ["kept by score", "refused", "dropped by score"]
		.into_iter()
		.map(|content| (lesson(proj, content, "learning", &[]), false))
		.collect();
	let kept = filter_scoped_by_relevance(&config, "anything", candidates).await;
	let contents: Vec<&str> = kept.iter().map(|(l, _)| l.content.as_str()).collect();
	assert_eq!(contents, ["kept by score", "refused"]);
}

#[serial]
#[tokio::test]
async fn recall_gate_skips_the_call_without_scoped_candidates_and_can_leave_globals_only() {
	let fake = install_fake_evaluation(vec![FakeEvaluationStep::Answers(nouls(&[("c0", 0.1)]))]).await;
	let config = recall_gate_config();
	let proj = "__inject_test_proj_gate_empty";
	let calls = evaluate_counter(Seam::Recall, "calls");
	let global_only = vec![(lesson(proj, "global rule", "learning", &[]), true)];
	let kept = filter_scoped_by_relevance(&config, "anything", global_only.clone()).await;
	assert_eq!(kept.len(), 1);
	assert!(fake.requests().is_empty(), "zero scoped candidates make no call");
	assert_eq!(evaluate_counter(Seam::Recall, "calls"), calls);

	let mut mixed = global_only;
	mixed.push((lesson(proj, "scoped thing", "learning", &[]), false));
	let kept = filter_scoped_by_relevance(&config, "anything", mixed).await;
	assert_eq!(kept.len(), 1);
	assert!(kept[0].1, "only the global lesson survives");
	assert_eq!(fake.requests().len(), 1);
}

#[serial]
#[tokio::test]
async fn recall_gate_end_to_end_empties_the_pack_or_falls_through_when_unavailable() {
	let fake = install_fake_evaluation(vec![
		FakeEvaluationStep::Answers(nouls(&[("c0", 0.05)])),
		FakeEvaluationStep::MissingKey("CLOUDFLARE_API_KEY"),
	])
	.await;
	let _data = TestDataDir::new();
	let proj = "__inject_test_proj_gate_e2e";
	cleanup(proj);
	let config = recall_gate_config();
	FileBackend
		.store(&lesson(
			proj,
			"always run the test suite on the dev box",
			"learning",
			&["testing"],
		))
		.await
		.expect("store lesson");

	// Every scoped candidate excluded, no global rules: the pack is empty.
	let (_tx1, rx1) = cancel_pair();
	let (text, selected) = retrieve_and_format(&config, "", ROLE, proj, false, rx1).await.unwrap();
	assert!(text.is_empty(), "pack should be empty, got: {text}");
	assert!(selected.is_empty());
	assert_eq!(fake.requests().len(), 1);

	// Provider unavailable: the pre-change pack, one unavailable, no panic.
	let unavailable = evaluate_counter(Seam::Recall, "unavailable");
	let (_tx2, rx2) = cancel_pair();
	let (text, selected) = retrieve_and_format(&config, "", ROLE, proj, false, rx2).await.unwrap();
	assert!(text.contains("always run the test suite on the dev box"));
	assert_eq!(selected.len(), 1);
	assert_eq!(evaluate_counter(Seam::Recall, "unavailable"), unavailable + 1);

	cleanup(proj);
}

#[test]
fn decoded_memory_pack_keeps_memory_ids_without_source_line_ids() {
	let record = lesson(
		"hash-rendering",
		"Keep the first rule.\n  Keep the second rule.",
		"learning",
		&[],
	);
	let rendered = render_item("M1", &record, false, Some("/tmp/memory.md"), &[]);
	assert_eq!(
		rendered,
		"- [M1 scope=scoped confidence=high ref=/tmp/memory.md] Keep the first rule.\n  Keep the second rule.\n"
	);
}

#[serial]
#[tokio::test]
async fn verified_rules_are_selected_whole_within_the_pack_budget() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let config = fake_provider_config();
	let project = "rule-budget";
	let mut originals = Vec::new();
	for index in 0..24 {
		let content = format!(
			"Rule {index}: {}",
			"For release builds only, use the documented remote build host. ".repeat(12)
		);
		let mut rule = lesson(project, &content, "learning", &["release"]);
		rule.user_rule = Some(crate::supervisor::learning::mod_tests::user_rule(&rule, &content));
		FileBackend.store(&rule).await.unwrap();
		originals.push(rule);
	}
	let (_tx, rx) = cancel_pair();
	let (text, selected) = retrieve_and_format(&config, "", ROLE, project, false, rx)
		.await
		.unwrap();
	assert!(!selected.is_empty());
	assert!(selected.len() < originals.len());
	assert!(crate::session::estimate_tokens(&text) <= MAX_MEMORY_PACK_TOKENS);
	for item in &selected {
		let rule = item.user_rule.as_ref().unwrap();
		assert!(
			text.contains(&rule.quote),
			"a selected rule's condition must not be cut"
		);
		assert!(text.contains(&rule.content));
	}
	let stored = FileBackend.retrieve_all(ROLE, project).await.unwrap();
	assert_eq!(stored.len(), originals.len());
	for original in originals {
		assert!(stored
			.iter()
			.any(|item| item.content == original.content && item.user_rule == original.user_rule));
	}
}

#[tokio::test]
#[serial]
async fn verified_rules_still_need_to_apply_to_the_current_request() {
	let fake = install_fake_evaluation(vec![FakeEvaluationStep::Answers(nouls(&[("c0", 0.9), ("c1", 0.0)]))]).await;
	let config = recall_gate_config();
	let mut applicable = lesson("project", "For release builds, use the dev box", "learning", &[]);
	applicable.user_rule = Some(crate::supervisor::learning::mod_tests::user_rule(
		&applicable,
		&applicable.content,
	));
	let mut unrelated = lesson(
		"project",
		"For product screenshots, use the staging site",
		"learning",
		&[],
	);
	unrelated.user_rule = Some(crate::supervisor::learning::mod_tests::user_rule(
		&unrelated,
		&unrelated.content,
	));
	let filtered = filter_scoped_by_relevance(
		&config,
		"Build a release",
		vec![(applicable.clone(), false), (unrelated, false)],
	)
	.await;
	assert_eq!(filtered.len(), 1);
	assert_eq!(filtered[0].0.user_rule, applicable.user_rule);
	assert_eq!(fake.requests()[0].state["candidates"][0]["content"], applicable.content);
}

#[serial]
#[tokio::test]
async fn unmatched_scoped_verified_rules_fill_the_remaining_budget_by_importance() {
	let _fake = install_fake_evaluation(vec![FakeEvaluationStep::Answers(nouls(&[("c0", 0.0), ("c1", 0.0)]))]).await;
	let _data = TestDataDir::new();
	let project = "rule-leftover";
	let mut low = lesson(project, "Never run builds locally; use the dev box", "learning", &[]);
	low.importance = 0.6;
	low.user_rule = Some(crate::supervisor::learning::mod_tests::user_rule(&low, &low.content));
	let mut high = lesson(project, "Never push directly to main", "learning", &[]);
	high.importance = 0.9;
	high.user_rule = Some(crate::supervisor::learning::mod_tests::user_rule(&high, &high.content));
	FileBackend.store(&low).await.unwrap();
	FileBackend.store(&high).await.unwrap();

	// The relevance gate rejects both for this wording, yet a restriction
	// targets actions, so both still stand, ordered by importance.
	let (_tx, rx) = cancel_pair();
	let (text, selected) = retrieve_and_format(&recall_gate_config(), "fix the bug", ROLE, project, false, rx)
		.await
		.unwrap();
	let rules: Vec<_> = selected.iter().map(|item| item.user_rule.clone()).collect();
	assert_eq!(rules, [high.user_rule, low.user_rule]);
	assert!(text.contains("use the dev box"));
}

#[serial]
#[tokio::test]
async fn equal_summaries_do_not_discard_distinct_user_quotes_or_scopes() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let mut global = lesson("project", "Use the build host for the environment", "learning", &[]);
	global.scope = "global".into();
	global.user_rule = Some(crate::supervisor::learning::mod_tests::user_rule(
		&global,
		"For production builds, use prod-builder.",
	));
	let mut scoped = lesson("project", &global.content, "learning", &[]);
	scoped.user_rule = Some(crate::supervisor::learning::mod_tests::user_rule(
		&scoped,
		"For development builds in this project, use dev-builder.",
	));
	FileBackend.store(&global).await.unwrap();
	FileBackend.store(&scoped).await.unwrap();
	let (_tx, rx) = cancel_pair();
	let (text, selected) = retrieve_and_format(&fake_provider_config(), "", ROLE, "project", false, rx)
		.await
		.unwrap();
	assert_eq!(selected.iter().filter(|item| item.user_rule.is_some()).count(), 2);
	assert!(text.contains("prod-builder"));
	assert!(text.contains("dev-builder"));
}

#[serial]
#[tokio::test]
async fn task_matched_cold_global_rule_precedes_standing_preferences() {
	let _guard = ENV_LOCK.lock().await;
	let _data = TestDataDir::new();
	let config = fake_provider_config();
	for index in 0..12 {
		let mut rule = lesson(
			"project",
			&format!("Formatting preference {index}: keep code comments concise"),
			"learning",
			&[],
		);
		rule.scope = "global".into();
		rule.importance = 0.9;
		rule.user_rule = Some(crate::supervisor::learning::mod_tests::user_rule(&rule, &rule.content));
		FileBackend.store(&rule).await.unwrap();
	}
	let mut target = lesson(
		"project",
		"Before a database rollback, preserve the audit trail",
		"learning",
		&["postgres", "transaction"],
	);
	target.scope = "global".into();
	target.importance = 0.05;
	target.user_rule = Some(crate::supervisor::learning::mod_tests::user_rule(
		&target,
		"Across my projects, preserve the audit trail before any database rollback.",
	));
	FileBackend.store(&target).await.unwrap();
	let (_, archived) = crate::supervisor::learning::retention::archive_record(&target).unwrap();
	let (_tx, rx) = cancel_pair();
	let (text, selected) = retrieve_and_format(&config, "postgres transaction rollback", ROLE, "project", false, rx)
		.await
		.unwrap();
	assert_eq!(selected[0].user_rule, target.user_rule);
	assert!(text.contains(&target.user_rule.as_ref().unwrap().quote));
	assert!(crate::session::estimate_tokens(&text) <= MAX_MEMORY_PACK_TOKENS);
	assert!(archived.exists(), "exposure must not reactivate or rewrite a record");
}
