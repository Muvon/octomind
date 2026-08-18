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

//! Sidecar plan lifecycle through the supervisor-owned entry points, run
//! inside a unique task-local session so the plan storage is isolated.

use super::*;

fn directive(title: &str) -> crate::supervisor::plan::PlanTaskDirective {
	crate::supervisor::plan::PlanTaskDirective {
		title: title.to_string(),
		done_when: format!("{title} is verified"),
	}
}

#[tokio::test]
async fn test_sidecar_plan_lifecycle() {
	crate::session::context::with_session_id("plan-test-lifecycle".to_string(), async {
		assert!(!has_active_plan());
		assert!(render_plan_checklist().is_none());

		sidecar_start(
			"Ship the feature",
			&[directive("implement"), directive("verify")],
		)
		.expect("plan starts");
		assert!(has_active_plan());

		// Double-start is rejected while a plan is active
		assert!(sidecar_start("Another", &[directive("a"), directive("b")]).is_err());

		let checklist = render_plan_checklist().expect("active plan renders");
		assert!(checklist.contains("implement"), "checklist: {checklist}");
		assert!(checklist.contains("verify"), "checklist: {checklist}");

		sidecar_advance("implemented the thing").expect("advance");
		let checklist = render_plan_checklist().expect("still active");
		assert!(!checklist.is_empty());

		sidecar_revise(
			"scope changed",
			&[directive("implement v2"), directive("verify v2")],
		)
		.expect("revise");
		let checklist = render_plan_checklist().expect("revised plan renders");
		assert!(checklist.contains("v2"), "checklist: {checklist}");

		sidecar_finish("all wrapped up").expect("finish");
		assert!(!has_active_plan());
	})
	.await;
}

#[tokio::test]
async fn test_sidecar_validation_errors() {
	crate::session::context::with_session_id("plan-test-validation".to_string(), async {
		// Empty title rejected
		assert!(sidecar_start("  ", &[directive("a"), directive("b")]).is_err());
		// Fewer than two tasks rejected (a one-step plan is not a plan)
		assert!(sidecar_start("Title", &[directive("only")]).is_err());
		// Advancing without an active plan fails loudly; finishing without
		// one is an idempotent no-op (double /done must not error)
		assert!(sidecar_advance("x").is_err());
		assert!(sidecar_finish("x").is_ok());
		// Empty summaries rejected before any plan lookup
		assert!(sidecar_advance("   ").is_err());
		assert!(sidecar_finish("   ").is_err());
	})
	.await;
}

#[tokio::test]
async fn test_staleness_marker() {
	crate::session::context::with_session_id("plan-test-staleness".to_string(), async {
		// No plan → no marker regardless of timestamp
		assert!(plan_staleness_marker(u64::MAX).is_none());

		sidecar_start("Track staleness", &[directive("a"), directive("b")]).expect("start");
		// A task newer than the last plan touch marks the plan stale
		assert!(plan_staleness_marker(u64::MAX).is_some());
		// A task older than the plan's creation does not
		assert!(plan_staleness_marker(0).is_none());
		// The staleness-aware checklist renders in both cases
		assert!(render_plan_checklist_with_staleness(Some(u64::MAX)).is_some());
		assert!(render_plan_checklist_with_staleness(None).is_some());

		sidecar_finish("done").expect("finish");
	})
	.await;
}

#[tokio::test]
async fn test_task_start_index_helpers() {
	crate::session::context::with_session_id("plan-test-task-idx".to_string(), async {
		assert!(get_current_task_start_index().is_none());
		set_current_task_start_index(5);
		assert_eq!(get_current_task_start_index(), Some(5));
		clear_task_start_index();
		assert!(get_current_task_start_index().is_none());
	})
	.await;
}
