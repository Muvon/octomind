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

//! Tests run real git in temporary directories and a stub plugin that records
//! what the host sends it.

use super::*;
use crate::plugins::protocol::{event, PromptSubmit, RequestBefore, RequestPhase, ToolAfter, TurnKind};
use crate::plugins::test_support::{connect_stub, StubPlugin};
use crate::plugins::EventContext;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

const ROLE: &str = "developer";

fn git(dir: &Path, args: &[&str]) {
	let status = std::process::Command::new("git")
		.args(args)
		.current_dir(dir)
		.stdin(std::process::Stdio::null())
		.stdout(std::process::Stdio::null())
		.status()
		.expect("git runs");
	assert!(status.success(), "git {args:?} failed");
}

/// A repository with two documents and a readme, none of them committed.
fn repository() -> TempDir {
	let dir = tempfile::tempdir().unwrap();
	git(dir.path(), &["init", "-q"]);
	std::fs::create_dir(dir.path().join("docs")).unwrap();
	std::fs::write(dir.path().join("docs/a.md"), "alpha\n").unwrap();
	std::fs::write(dir.path().join("docs/b.md"), "beta\n").unwrap();
	std::fs::write(dir.path().join("README.md"), "readme\n").unwrap();
	dir
}

fn entries(names: &[&str]) -> Vec<String> {
	names.iter().map(|name| name.to_string()).collect()
}

fn defaults() -> Limits {
	Limits::of(&PluginOptions::default())
}

async fn read(dir: &Path, names: &[&str]) -> Vec<FileText> {
	read_entries(dir, &entries(names), &mut BTreeMap::new(), defaults()).await
}

fn skipped_reason<'a>(texts: &'a [FileText], path: &str) -> Option<&'a str> {
	texts
		.iter()
		.find(|text| text.path == path)
		.and_then(|text| text.skipped.as_deref())
}

#[tokio::test]
async fn paths_and_globs_come_back_with_text_and_hash() {
	let dir = repository();
	let texts = read(dir.path(), &["docs/*.md", "README.md"]).await;
	let paths: Vec<&str> = texts.iter().map(|text| text.path.as_str()).collect();
	assert_eq!(paths, ["docs/a.md", "docs/b.md", "README.md"]);
	let readme = &texts[2];
	assert_eq!(readme.text.as_deref(), Some("readme\n"));
	assert_eq!(readme.hash, hex::encode(Sha256::digest(b"readme\n")));
	assert_eq!(readme.skipped, None);
	assert!(read(dir.path(), &["docs/*.txt"]).await.is_empty());
}

#[tokio::test]
async fn a_file_outside_the_root_is_refused() {
	let dir = repository();
	let outer = tempfile::tempdir().unwrap();
	std::fs::write(outer.path().join("secret"), "x").unwrap();
	let absolute = outer.path().join("secret").to_string_lossy().into_owned();
	let texts = read(dir.path(), &["../x", &absolute, "~/notes"]).await;
	assert_eq!(texts.len(), 3);
	for text in &texts {
		assert_eq!(text.skipped.as_deref(), Some("outside"), "{}", text.path);
		assert!(text.hash.is_empty() && text.text.is_none());
	}
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_that_leaves_the_root_is_refused() {
	let dir = repository();
	let outer = tempfile::tempdir().unwrap();
	std::fs::write(outer.path().join("secret"), "x").unwrap();
	std::os::unix::fs::symlink(outer.path().join("secret"), dir.path().join("link.md")).unwrap();
	let texts = read(dir.path(), &["link.md", "*.md"]).await;
	assert_eq!(skipped_reason(&texts, "link.md"), Some("outside"));
}

#[tokio::test]
async fn an_ignored_file_is_refused_and_no_glob_returns_it() {
	let dir = repository();
	std::fs::write(dir.path().join(".gitignore"), "secret.txt\n").unwrap();
	std::fs::write(dir.path().join("secret.txt"), "hidden").unwrap();
	std::fs::write(dir.path().join("open.txt"), "shown").unwrap();
	let texts = read(dir.path(), &["secret.txt"]).await;
	assert_eq!(skipped_reason(&texts, "secret.txt"), Some("ignored"));
	let globbed = read(dir.path(), &["*.txt"]).await;
	assert_eq!(
		globbed.iter().map(|text| text.path.as_str()).collect::<Vec<_>>(),
		["open.txt"]
	);
}

#[tokio::test]
async fn missing_large_and_binary_files_say_why() {
	let dir = repository();
	std::fs::write(dir.path().join("big.txt"), vec![b'a'; 20]).unwrap();
	std::fs::write(dir.path().join("blob.bin"), [0xff, 0xfe, 0x00]).unwrap();
	let options = PluginOptions {
		read_max_file_bytes: Some(10),
		..Default::default()
	};
	let names = entries(&["nope.md", "big.txt", "blob.bin", "docs"]);
	let texts = read_entries(dir.path(), &names, &mut BTreeMap::new(), Limits::of(&options)).await;
	assert_eq!(skipped_reason(&texts, "nope.md"), Some("missing"));
	assert_eq!(skipped_reason(&texts, "big.txt"), Some("too_large"));
	assert_eq!(skipped_reason(&texts, "blob.bin"), Some("not_text"));
	assert_eq!(skipped_reason(&texts, "docs"), Some("missing"));
}

#[tokio::test]
async fn outside_a_repository_the_working_directory_is_the_root_and_globs_use_the_glob_crate() {
	let dir = tempfile::tempdir().unwrap();
	std::fs::write(dir.path().join("a.txt"), "one").unwrap();
	std::fs::write(dir.path().join(".gitignore"), "a.txt\n").unwrap();
	let texts = read(dir.path(), &["*.txt", "../x"]).await;
	assert_eq!(texts.len(), 2);
	assert_eq!(texts[0].path, "a.txt");
	assert_eq!(texts[0].text.as_deref(), Some("one"));
	assert_eq!(skipped_reason(&texts, "../x"), Some("outside"));
}

#[tokio::test]
async fn the_count_limit_drops_the_files_beyond_it_in_the_order_given() {
	let dir = repository();
	let options = PluginOptions {
		read_max_files: Some(2),
		..Default::default()
	};
	let names = entries(&["README.md", "docs/*.md"]);
	let texts = read_entries(dir.path(), &names, &mut BTreeMap::new(), Limits::of(&options)).await;
	let paths: Vec<&str> = texts.iter().map(|text| text.path.as_str()).collect();
	assert_eq!(paths, ["README.md", "docs/a.md"]);
}

#[test]
fn a_plugin_may_ask_for_less_than_the_host_allows_never_for_more() {
	let ask = |files, bytes| {
		Limits::of(&PluginOptions {
			read_max_files: files,
			read_max_file_bytes: bytes,
			..Default::default()
		})
	};
	let none = ask(None, None);
	assert_eq!(
		(none.max_files, none.max_file_bytes),
		(READ_MAX_FILES, READ_MAX_FILE_BYTES)
	);
	let less = ask(Some(3), Some(100));
	assert_eq!((less.max_files, less.max_file_bytes), (3, 100));
	let more = ask(Some(READ_MAX_FILES + 1), Some(READ_MAX_FILE_BYTES + 1));
	assert_eq!(
		(more.max_files, more.max_file_bytes),
		(READ_MAX_FILES, READ_MAX_FILE_BYTES)
	);
}

// ---------------------------------------------------------------------------
// Through the host
// ---------------------------------------------------------------------------

/// A stub plugin switched on for a session in `dir`.
struct Rig {
	id: String,
	stub: StubPlugin,
	dir: TempDir,
}

async fn rig(dir: TempDir, stub: StubPlugin) -> Rig {
	let id = format!("read-{}", uuid::Uuid::new_v4());
	connect_stub(&id, ROLE, dir.path(), &stub).await;
	Rig { id, stub, dir }
}

fn stub() -> StubPlugin {
	StubPlugin::new(&[event::REQUEST_BEFORE, event::PROMPT_SUBMIT, event::TOOL_AFTER])
}

impl Rig {
	fn context(&self) -> EventContext<'_> {
		EventContext {
			session_id: &self.id,
			parent_session_id: None,
			role: ROLE,
			model: None,
			workdir: self.dir.path(),
			project: "proj",
			messages: &[],
			detached: false,
			status: None,
			plan: None,
			tools: None,
			active_skills: Vec::new(),
		}
	}

	async fn request(&self) {
		let payload = RequestBefore {
			phase: RequestPhase::TurnStart,
			turn: TurnKind::User,
			free_context_tokens: None,
			pending_async: false,
		};
		crate::plugins::request_before(&self.context(), payload).await;
	}

	async fn tool_after(&self) {
		let payload = ToolAfter {
			intent: None,
			calls: Vec::new(),
			tree: None,
			subagents: None,
			background: Vec::new(),
			stopping: None,
			context_files: Vec::new(),
		};
		crate::plugins::tool_after(&self.context(), payload).await;
	}

	async fn prompt(&self) {
		let payload = PromptSubmit {
			prompt: "go".into(),
			original: None,
			carried_summary: None,
			goal: None,
		};
		crate::plugins::prompt_submit(&self.context(), payload).await;
	}

	/// The `files` the plugin got with its `index`-th `method` event, as `(path, skipped)`.
	fn files(&self, method: &str, index: usize) -> Vec<(String, Option<String>)> {
		let sent = self.stub.sent(method);
		sent[index]["files"]
			.as_array()
			.map(|files| {
				files
					.iter()
					.map(|file| {
						(
							file["path"].as_str().unwrap().to_string(),
							file["skipped"].as_str().map(str::to_string),
						)
					})
					.collect()
			})
			.unwrap_or_default()
	}
}

fn paths_of(files: &[(String, Option<String>)]) -> Vec<&str> {
	files.iter().map(|(path, _)| path.as_str()).collect()
}

#[tokio::test]
async fn the_texts_arrive_with_the_next_event_and_not_with_the_one_after() {
	let stub = stub().queue([json!({"read": ["docs/*.md", "README.md"]})]);
	let rig = rig(repository(), stub).await;
	rig.request().await;
	rig.request().await;
	rig.request().await;

	assert!(rig.files(event::REQUEST_BEFORE, 0).is_empty());
	let second = rig.stub.sent(event::REQUEST_BEFORE)[1]["files"].clone();
	let texts: Vec<(&str, &str, usize)> = second
		.as_array()
		.unwrap()
		.iter()
		.map(|file| {
			(
				file["path"].as_str().unwrap(),
				file["text"].as_str().unwrap(),
				file["hash"].as_str().unwrap().len(),
			)
		})
		.collect();
	assert_eq!(
		texts,
		[
			("docs/a.md", "alpha\n", 64),
			("docs/b.md", "beta\n", 64),
			("README.md", "readme\n", 64)
		]
	);
	assert!(rig.files(event::REQUEST_BEFORE, 2).is_empty());
	crate::plugins::end_session(&rig.id);
}

#[tokio::test]
async fn an_edited_file_arrives_again_an_untouched_one_does_not_and_a_deleted_one_arrives_once() {
	let stub = stub().queue([json!({"read": ["docs/*.md"]})]);
	let rig = rig(repository(), stub).await;
	rig.request().await;
	rig.request().await;

	std::fs::write(rig.dir.path().join("docs/a.md"), "alpha, edited\n").unwrap();
	rig.tool_after().await;
	assert_eq!(rig.files(event::TOOL_AFTER, 0), [("docs/a.md".to_string(), None)]);
	rig.tool_after().await;
	assert!(rig.files(event::TOOL_AFTER, 1).is_empty());

	std::fs::remove_file(rig.dir.path().join("docs/b.md")).unwrap();
	rig.tool_after().await;
	assert_eq!(
		rig.files(event::TOOL_AFTER, 2),
		[("docs/b.md".to_string(), Some("missing".to_string()))]
	);
	rig.tool_after().await;
	assert!(rig.files(event::TOOL_AFTER, 3).is_empty());
	crate::plugins::end_session(&rig.id);
}

#[tokio::test]
async fn a_file_rewritten_with_the_same_text_is_not_sent_again() {
	let stub = stub().queue([json!({"read": ["README.md"]})]);
	let rig = rig(repository(), stub).await;
	rig.request().await;
	rig.request().await;
	// Same size, so only the modification time can differ.
	std::fs::write(rig.dir.path().join("README.md"), "readme\n").unwrap();
	rig.tool_after().await;
	assert!(rig.files(event::TOOL_AFTER, 0).is_empty());
	crate::plugins::end_session(&rig.id);
}

#[tokio::test]
async fn a_new_file_that_matches_an_earlier_glob_arrives_with_the_next_prompt_only() {
	let stub = stub().queue([json!({"read": ["docs/*.md"]})]);
	let rig = rig(repository(), stub).await;
	rig.request().await;
	rig.request().await;

	std::fs::write(rig.dir.path().join("docs/c.md"), "gamma\n").unwrap();
	rig.tool_after().await;
	assert!(rig.files(event::TOOL_AFTER, 0).is_empty());
	rig.prompt().await;
	assert_eq!(paths_of(&rig.files(event::PROMPT_SUBMIT, 0)), ["docs/c.md"]);
	rig.prompt().await;
	assert!(rig.files(event::PROMPT_SUBMIT, 1).is_empty());
	crate::plugins::end_session(&rig.id);
}

#[tokio::test]
async fn the_limits_the_plugin_declared_hold() {
	let stub = stub()
		.init_field("options", json!({"read_max_files": 1}))
		.queue([json!({"read": ["docs/*.md"]})]);
	let rig = rig(repository(), stub).await;
	rig.request().await;
	rig.request().await;
	assert_eq!(paths_of(&rig.files(event::REQUEST_BEFORE, 1)), ["docs/a.md"]);
	crate::plugins::end_session(&rig.id);
}

#[tokio::test]
async fn an_unanswered_event_leaves_the_waiting_files_for_the_next_one() {
	let calls = AtomicUsize::new(0);
	let stub = stub().respond(move |_, _| match calls.fetch_add(1, Ordering::SeqCst) {
		0 => Ok(json!({"read": ["README.md"]})),
		1 => Err(anyhow::anyhow!("no answer")),
		_ => Ok(json!({})),
	});
	let rig = rig(repository(), stub).await;
	rig.request().await;
	rig.request().await;
	rig.request().await;
	rig.request().await;
	assert_eq!(paths_of(&rig.files(event::REQUEST_BEFORE, 1)), ["README.md"]);
	assert_eq!(paths_of(&rig.files(event::REQUEST_BEFORE, 2)), ["README.md"]);
	assert!(rig.files(event::REQUEST_BEFORE, 3).is_empty());
	crate::plugins::end_session(&rig.id);
}

#[tokio::test]
async fn a_note_returned_on_prompt_submit_is_in_the_messages_before_the_first_request() {
	let stub = StubPlugin::new(&[event::PROMPT_SUBMIT]).answer(json!({"note": ["check the docs first"]}));
	let dir = tempfile::tempdir().unwrap();
	let id = format!("read-{}", uuid::Uuid::new_v4());
	connect_stub(&id, ROLE, dir.path(), &stub).await;
	let mut chat = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	chat.session.info.name = id.clone();
	crate::session::context::with_session_id(id.clone(), async {
		let prompt = chat.submit_prompt("go".into(), "go").await.unwrap();
		assert_eq!(prompt, "go");
	})
	.await;
	let written: Vec<&str> = chat
		.session
		.messages
		.iter()
		.map(|message| message.content.as_str())
		.collect();
	assert_eq!(written.len(), 1, "{written:?}");
	assert!(written[0].starts_with("<system-note>") && written[0].contains("check the docs first"));
	crate::plugins::end_session(&id);
}
