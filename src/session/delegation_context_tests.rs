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

struct Transcript {
	source: Source,
	path: PathBuf,
}

impl Transcript {
	fn new(delegated: bool) -> Self {
		Self::with_session(format!("delegation-test-{}", Uuid::new_v4()), delegated)
	}

	fn with_session(session: String, delegated: bool) -> Self {
		let path = super::super::logger::get_session_log_file(&session).unwrap();
		Self {
			source: Source {
				session,
				end: 0,
				boundary: [0; 32],
				delegated,
			},
			path,
		}
	}

	fn add(&mut self, role: &str, content: &str) {
		let row = super::super::Session::build_message(role, content);
		super::super::append_to_session_file(&self.path, &serde_json::to_string(&row).unwrap()).unwrap();
		self.source = Source::capture(&self.source.session, &self.path, self.source.delegated).unwrap();
	}
}

impl Drop for Transcript {
	fn drop(&mut self) {
		std::fs::remove_file(&self.path).unwrap();
	}
}

async fn text(grant: Arc<Grant>, query: &str, scope: Scope) -> String {
	serde_json::to_string(&search(grant, query.to_owned(), scope, DEFAULT_LIMIT).await.unwrap()).unwrap()
}

#[tokio::test]
#[serial_test::serial]
async fn cold_search_obeys_both_ancestor_boundaries_and_labels_provenance() {
	let mut root = Transcript::new(false);
	root.add("system", "edit_policy private system message");
	root.add("user", "<skill name=\"release\">edit_policy private skill body</skill>");
	root.add("user", "edit_policy allows src/only.rs");
	root.add("user", "edit_policy ACCESS_TOKEN=private-test-token");
	root.add("tool", "edit_policy unprojected tool payload");
	root.add(
		"assistant",
		"edit_policy discussed <system>edit_policy hidden assistant text</system>",
	);
	let old_root = root.source.clone();
	root.add("user", "edit_policy new_root_value");

	let mut parent = Transcript::new(true);
	parent.add("user", "edit_policy delegated brief");
	let old_parent = parent.source.clone();
	parent.add("assistant", "edit_policy new_parent_value");

	let grant = Arc::new(Grant {
		id: Uuid::new_v4().to_string(),
		child: "tap-child".to_owned(),
		workspace: PathBuf::from("."),
		sources: vec![old_parent, old_root],
	});
	let all = text(grant.clone(), "edit_policy", Scope::Ancestors).await;
	assert!(all.contains("src/only.rs"));
	assert!(all.contains("user_instruction"));
	assert!(all.contains("assistant_discussion"));
	assert!(all.contains("delegation"));
	assert!(all.contains("delegated brief"));
	for excluded in [
		"private-test-token",
		"unprojected tool payload",
		"private system",
		"private skill",
		"hidden assistant",
		"new_root_value",
		"new_parent_value",
	] {
		assert!(!all.contains(excluded), "{excluded} unexpectedly appeared in {all}");
	}
	assert!(!text(grant.clone(), "edit_policy", Scope::Parent)
		.await
		.contains("src/only.rs"));
	assert!(!text(grant, "edit_policy", Scope::Root)
		.await
		.contains("delegated brief"));
}

#[tokio::test]
#[serial_test::serial]
async fn a_resumed_child_gets_the_same_boundary_and_cleanup_unbinds_it() {
	let workspace = tempfile::tempdir().unwrap();
	let canonical = workspace.path().canonicalize().unwrap();
	let outside = tempfile::tempdir().unwrap();
	let mut parent = Transcript::new(false);
	parent.add("user", "edit_policy initial constraint");
	let child = format!("tap-delegation-resume-{}", Uuid::new_v4());
	let active = issue(&child, canonical, parent.source.clone()).unwrap();
	let token = active.id;
	drop(active);
	parent.add("user", "edit_policy later constraint");

	assert!(bind(&child, outside.path(), token).is_err());
	assert!(bind(&child, workspace.path(), Uuid::new_v4()).is_err());
	let resume = resume(&child, &parent.source.session, workspace.path())
		.unwrap()
		.unwrap();
	assert_eq!(resume.id, token);
	bind(&child, workspace.path(), resume.id).unwrap();
	let current = super::super::context::with_session_id(child.clone(), async { current_grant() })
		.await
		.unwrap();
	let result = text(current, "edit_policy", Scope::Root).await;
	assert!(result.contains("initial constraint"));
	assert!(!result.contains("later constraint"));
	assert!(current_grant().is_none(), "standalone caller must have no scope");

	let grandchild_id = format!("tap-delegation-descendant-{}", Uuid::new_v4());
	let mut middle = Transcript::with_session(child.clone(), true);
	middle.add("user", "edit_policy parent supplied a follow-up brief");
	let descendant = super::super::context::with_session_id(child.clone(), async {
		issue(
			&grandchild_id,
			workspace.path().canonicalize().unwrap(),
			middle.source.clone(),
		)
		.unwrap()
	})
	.await;
	middle.add("user", "edit_policy too late for grandchild");
	bind(&grandchild_id, workspace.path(), descendant.id).unwrap();
	let inherited = super::super::context::with_session_id(grandchild_id.clone(), async { current_grant() })
		.await
		.unwrap();
	let root_view = text(inherited.clone(), "edit_policy", Scope::Root).await;
	assert!(root_view.contains("initial constraint"));
	assert!(!root_view.contains("later constraint"));
	let parent_view = text(inherited, "edit_policy", Scope::Parent).await;
	assert!(parent_view.contains("follow-up brief"));
	assert!(!parent_view.contains("too late"));
	drop(descendant);
	clear_for_session(&grandchild_id);
	std::fs::remove_file(grant_path(&grandchild_id).unwrap()).unwrap();

	clear_for_session(&child);
	assert!(
		super::super::context::with_session_id(child.clone(), async { current_grant() })
			.await
			.is_none()
	);

	drop(resume);
	std::fs::remove_file(grant_path(&child).unwrap()).unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn archived_evidence_survives_compaction_and_uses_original_provenance() {
	use crate::session::chat::conversation_compression::COMPRESSION_MESSAGE_NAME;

	let mut parent = Transcript::new(false);
	parent.add("user", "evidence exists before compaction");
	let archive_dir = crate::directories::get_sessions_dir()
		.unwrap()
		.join("archive")
		.join(&parent.source.session);
	std::fs::create_dir_all(&archive_dir).unwrap();
	let archive = archive_dir.join("comp_123.jsonl");
	let original = super::super::Session::build_message("user", "archive_only_marker Preserve the lossless flags.");
	std::fs::write(&archive, format!("{}\n", serde_json::to_string(&original).unwrap())).unwrap();
	let mut summary = super::super::Session::build_message(
		"assistant",
		&format!("<conversation_summary>\n<archive path=\"{}\">\narchive_only_marker fabricated summary\n</conversation_summary>", archive.display()),
	);
	summary.name = Some(COMPRESSION_MESSAGE_NAME.to_owned());
	super::super::append_to_session_file(&parent.path, &serde_json::to_string(&summary).unwrap()).unwrap();
	let captured = Source::capture(&parent.source.session, &parent.path, false).unwrap();
	let grant = Arc::new(Grant {
		id: Uuid::new_v4().to_string(),
		child: "tap-archive".to_owned(),
		workspace: PathBuf::from("."),
		sources: vec![captured],
	});
	let result = text(grant, "archive_only_marker", Scope::Parent).await;
	assert!(result.contains("Preserve the lossless flags."));
	assert!(result.contains("archive:comp_123.jsonl:1:1"));
	assert!(!result.contains("fabricated summary"));
	std::fs::remove_dir_all(&archive_dir).unwrap();
}

#[test]
fn quoted_credentials_are_excluded_before_launch_and_projection() {
	for text in [
		r#"The payload is {"password": "x"}"#,
		r#"The payload is {"api_key": "synthetic escaped \" value"}"#,
		r#"The payload is {"service_access_token": "synthetic token"}"#,
	] {
		let message = super::super::Session::build_message("user", text);
		assert!(launch_request(std::slice::from_ref(&message)).is_none());
		assert!(project(&message, false).is_none());
	}

	for text in [
		r#"Only src/allowed.rs may change."#,
		r#"The payload is {"password": ""}"#,
	] {
		let message = super::super::Session::build_message("user", text);
		assert_eq!(launch_request(std::slice::from_ref(&message)).as_deref(), Some(text));
		assert_eq!(project(&message, false).unwrap().1, text);
	}
}
