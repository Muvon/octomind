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

// Session-keyed registries are process-global. Tests use unique session
// ids (uuid-like) so parallel-running tests don't see each other's
// state. Each test also cleans up its own session at the end so the
// registries don't grow unbounded across the suite.

fn unique_id(label: &str) -> SessionId {
	format!(
		"test-{}-{}-{}",
		label,
		std::process::id(),
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map(|d| d.as_nanos())
			.unwrap_or(0)
	)
}

#[tokio::test]
async fn current_session_id_returns_none_outside_scope() {
	assert!(current_session_id().is_none());
}

#[tokio::test]
async fn with_session_id_propagates_id_to_inner_future() {
	let id = unique_id("propagate");
	let observed = with_session_id(id.clone(), async {
		current_session_id().expect("inside scope")
	})
	.await;
	assert_eq!(observed, id);
	// And the id is gone after the scope ends.
	assert!(current_session_id().is_none());
}

#[tokio::test]
async fn with_session_id_propagates_through_spawned_task_when_inherited() {
	// `tokio::task_local!` propagates explicitly via `.scope().await`,
	// not implicitly into `tokio::spawn`. Confirm that pattern: an
	// inner async block sees the id; a detached `tokio::spawn` does not.
	let id = unique_id("scope-vs-spawn");
	with_session_id(id.clone(), async {
		// Direct child future inherits.
		let direct = current_session_id();
		assert_eq!(direct.as_deref(), Some(id.as_str()));

		// Detached spawn does NOT inherit — that's by design.
		let handle = tokio::spawn(async { current_session_id() });
		let spawned = handle.await.unwrap();
		assert!(
			spawned.is_none(),
			"task-local should not leak across tokio::spawn without explicit propagation"
		);
	})
	.await;
}

#[test]
fn active_skills_are_session_scoped() {
	let a = unique_id("skills-a");
	let b = unique_id("skills-b");

	add_active_skill(&a, "programming-rust");
	add_active_skill(&a, "shell");
	add_active_skill(&b, "marketing-backlink");

	assert_eq!(
		get_active_skills(&a),
		vec!["programming-rust".to_string(), "shell".to_string()]
	);
	assert_eq!(
		get_active_skills(&b),
		vec!["marketing-backlink".to_string()]
	);

	assert!(has_active_skill(&a, "shell"));
	assert!(!has_active_skill(&a, "marketing-backlink"));
	assert!(!has_active_skill(&b, "shell"));

	clear_active_skills(&a);
	clear_active_skills(&b);
}

#[test]
fn add_active_skill_is_idempotent() {
	let id = unique_id("idempotent");
	add_active_skill(&id, "shell");
	add_active_skill(&id, "shell");
	add_active_skill(&id, "shell");
	assert_eq!(get_active_skills(&id), vec!["shell".to_string()]);
	clear_active_skills(&id);
}

#[test]
fn remove_active_skill_drops_only_target() {
	let id = unique_id("remove");
	add_active_skill(&id, "shell");
	add_active_skill(&id, "docker");
	add_active_skill(&id, "kubernetes");

	remove_active_skill(&id, "docker");
	assert_eq!(
		get_active_skills(&id),
		vec!["shell".to_string(), "kubernetes".to_string()]
	);
	clear_active_skills(&id);
}

#[test]
fn clear_active_skills_removes_entire_session_entry() {
	let id = unique_id("clear");
	add_active_skill(&id, "shell");
	assert!(!get_active_skills(&id).is_empty());

	clear_active_skills(&id);
	assert!(get_active_skills(&id).is_empty());
	assert!(!has_active_skill(&id, "shell"));
}

#[test]
fn get_active_skills_for_unknown_session_returns_empty() {
	let id = unique_id("unknown");
	assert!(get_active_skills(&id).is_empty());
	assert!(!has_active_skill(&id, "anything"));
}
