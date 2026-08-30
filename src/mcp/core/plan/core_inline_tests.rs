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

#[tokio::test]
async fn staleness_marker_fires_only_for_an_active_plan_older_than_the_request() {
	// A unique session id scopes plan storage to this test — no global
	// contamination across parallel tests.
	crate::session::context::with_session_id("plan-staleness-test".to_string(), async {
		// No plan → no marker regardless of timestamp.
		assert_eq!(plan_staleness_marker(u64::MAX), None);

		let storage = get_storage();
		storage
			.lock()
			.unwrap()
			.create_plan(
				"demo".to_string(),
				vec![TaskData::new("a".into(), "do a".into(), None, None)],
			)
			.unwrap();

		// Fresh plan against an older request → no marker.
		assert_eq!(plan_staleness_marker(1), None);

		// Plan untouched since before the latest request → marker; the
		// same second is NOT stale (the mutation follows the message
		// within one turn).
		storage.lock().unwrap().set_touched_at(10);
		assert_eq!(plan_staleness_marker(11), Some(PLAN_STALENESS_MARKER));
		assert_eq!(plan_staleness_marker(10), None);

		// The recitation renderer appends the marker as the last line —
		// and only when actually stale.
		let recited = render_plan_checklist_with_staleness(Some(11)).unwrap();
		assert!(recited.starts_with("Live plan"));
		assert!(recited.trim_end().ends_with(PLAN_STALENESS_MARKER));
		let fresh = render_plan_checklist_with_staleness(Some(9)).unwrap();
		assert!(!fresh.contains(PLAN_STALENESS_MARKER));
		// No user task timestamp available → no marker.
		let unknown = render_plan_checklist_with_staleness(None).unwrap();
		assert!(!unknown.contains(PLAN_STALENESS_MARKER));

		// Engaging the plan clears the marker (self-clearing steering).
		storage
			.lock()
			.unwrap()
			.complete_current_task("done a".into())
			.unwrap();
		assert_eq!(plan_staleness_marker(11), None);

		// A finished plan is not recited and cannot be stale.
		storage
			.lock()
			.unwrap()
			.complete_plan("done".into())
			.unwrap();
		assert_eq!(plan_staleness_marker(u64::MAX), None);
		assert_eq!(render_plan_checklist_with_staleness(Some(u64::MAX)), None);
	})
	.await;
}

#[test]
fn condition_check_only_answers_the_machine_checkable() {
	assert_eq!(check_condition("file_exists: Cargo.toml"), Some(true));
	assert_eq!(
		check_condition("file_exists: definitely/not/a/real/path.xyz"),
		Some(false)
	);
	assert_eq!(
		check_condition("file_absent: definitely/not/a/real/path.xyz"),
		Some(true)
	);
	assert_eq!(check_condition("file_absent: Cargo.toml"), Some(false));
	// Free-form prose is not machine-checkable — left to the agent/verifier.
	assert_eq!(check_condition("the API still returns 200"), None);
	assert_eq!(check_condition("file_exists:"), None);
}

#[test]
fn sidecar_task_count_is_bounded() {
	let task = crate::supervisor::plan::PlanTaskDirective {
		title: "phase".to_string(),
		done_when: "outcome exists".to_string(),
	};
	assert!(sidecar_tasks(std::slice::from_ref(&task), 2).is_err());
	let six = vec![task.clone(); 6];
	assert!(sidecar_tasks(&six, 2).is_ok());
	let seven = vec![task; 7];
	assert!(sidecar_tasks(&seven, 2).is_err());
}

#[test]
fn verified_finish_closes_every_remaining_plan_item_atomically() {
	let mut storage = MemoryPlanStorage::new();
	storage
		.create_plan(
			"review".to_string(),
			["inspect", "synthesize", "deliver"]
				.into_iter()
				.map(|title| {
					TaskData::new(
						title.to_string(),
						format!("Done when: {title} is evidenced"),
						None,
						None,
					)
				})
				.collect(),
		)
		.unwrap();
	storage
		.complete_current_task("inspection already evidenced".to_string())
		.unwrap();

	finish_accepted_plan(&mut storage, "full deliverable verified").unwrap();

	assert!(!storage.has_active_plan().unwrap());
	assert_eq!(storage.get_completed_task_count().unwrap(), 3);
	let tasks = &storage.get_plan().unwrap().tasks;
	assert_eq!(
		tasks[0].summary.as_deref(),
		Some("inspection already evidenced")
	);
	assert!(tasks
		.iter()
		.all(|task| matches!(task.status, TaskStatus::Completed)));
	assert!(tasks[1..]
		.iter()
		.all(|task| task.summary.as_deref() == Some("full deliverable verified")));
}
