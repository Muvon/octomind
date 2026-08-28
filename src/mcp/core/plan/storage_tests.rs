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

fn sample_task(status: TaskStatus) -> PlanTask {
	PlanTask {
		title: "Implement feature".to_string(),
		description: "Add the new endpoint".to_string(),
		details: "step 1 done".to_string(),
		summary: Some("Feature shipped".to_string()),
		status,
		completed_at: Some(Utc::now()),
		message_range: Some(MessageRange {
			start_index: 3,
			end_index: 9,
		}),
		phase: Some("implementation".to_string()),
		valid_if: Some("tests pass".to_string()),
	}
}

#[test]
fn task_data_new_sets_all_fields() {
	let data = TaskData::new(
		"Title".to_string(),
		"Description".to_string(),
		Some("phase-1".to_string()),
		Some("valid if X".to_string()),
	);

	assert_eq!(data.title, "Title");
	assert_eq!(data.description, "Description");
	assert_eq!(data.phase.as_deref(), Some("phase-1"));
	assert_eq!(data.valid_if.as_deref(), Some("valid if X"));
}

#[test]
fn task_data_new_accepts_none_optionals() {
	let data = TaskData::new("Title".to_string(), "Description".to_string(), None, None);

	assert_eq!(data.phase, None);
	assert_eq!(data.valid_if, None);
}

#[test]
fn plan_status_serialization_roundtrip() {
	for status in [PlanStatus::Active, PlanStatus::Completed] {
		let json = serde_json::to_string(&status).expect("serialize PlanStatus");
		let back: PlanStatus = serde_json::from_str(&json).expect("deserialize PlanStatus");
		match (status, back) {
			(PlanStatus::Active, PlanStatus::Active)
			| (PlanStatus::Completed, PlanStatus::Completed) => {}
			_ => panic!("round-trip mismatch for {json}"),
		}
	}

	assert_eq!(
		serde_json::to_string(&PlanStatus::Active).unwrap(),
		"\"Active\""
	);
	assert_eq!(
		serde_json::to_string(&PlanStatus::Completed).unwrap(),
		"\"Completed\""
	);
}

#[test]
fn task_status_serialization_roundtrip() {
	for status in [TaskStatus::InProgress, TaskStatus::Completed] {
		let json = serde_json::to_string(&status).expect("serialize TaskStatus");
		let back: TaskStatus = serde_json::from_str(&json).expect("deserialize TaskStatus");
		match (status, back) {
			(TaskStatus::InProgress, TaskStatus::InProgress)
			| (TaskStatus::Completed, TaskStatus::Completed) => {}
			_ => panic!("round-trip mismatch for {json}"),
		}
	}

	assert_eq!(
		serde_json::to_string(&TaskStatus::InProgress).unwrap(),
		"\"InProgress\""
	);
	assert_eq!(
		serde_json::to_string(&TaskStatus::Completed).unwrap(),
		"\"Completed\""
	);
}

#[test]
fn message_range_serialization_roundtrip() {
	let range = MessageRange {
		start_index: 12,
		end_index: 47,
	};

	let json = serde_json::to_string(&range).expect("serialize MessageRange");
	assert!(json.contains("\"start_index\":12"));
	assert!(json.contains("\"end_index\":47"));

	let back: MessageRange = serde_json::from_str(&json).expect("deserialize MessageRange");
	assert_eq!(back.start_index, 12);
	assert_eq!(back.end_index, 47);
}

#[test]
fn plan_task_full_optional_fields_roundtrip() {
	let task = sample_task(TaskStatus::Completed);

	let json = serde_json::to_string(&task).expect("serialize PlanTask");
	assert!(json.contains("\"message_range\""));
	assert!(json.contains("\"phase\""));
	assert!(json.contains("\"valid_if\""));

	let back: PlanTask = serde_json::from_str(&json).expect("deserialize PlanTask");
	assert_eq!(back.title, task.title);
	assert_eq!(back.description, task.description);
	assert_eq!(back.details, task.details);
	assert_eq!(back.summary, task.summary);
	assert!(matches!(back.status, TaskStatus::Completed));
	assert_eq!(back.completed_at, task.completed_at);

	let range = back.message_range.expect("message_range survives");
	assert_eq!(range.start_index, 3);
	assert_eq!(range.end_index, 9);
	assert_eq!(back.phase.as_deref(), Some("implementation"));
	assert_eq!(back.valid_if.as_deref(), Some("tests pass"));
}

#[test]
fn plan_task_minimal_omits_optional_fields() {
	let task = PlanTask {
		title: "Investigate".to_string(),
		description: "Look into the bug".to_string(),
		details: String::new(),
		summary: None,
		status: TaskStatus::InProgress,
		completed_at: None,
		message_range: None,
		phase: None,
		valid_if: None,
	};

	let json = serde_json::to_string(&task).expect("serialize PlanTask");
	assert!(!json.contains("message_range"));
	assert!(!json.contains("phase"));
	assert!(!json.contains("valid_if"));

	let back: PlanTask = serde_json::from_str(&json).expect("deserialize PlanTask");
	assert_eq!(back.title, "Investigate");
	assert!(matches!(back.status, TaskStatus::InProgress));
	assert!(back.message_range.is_none());
	assert_eq!(back.phase, None);
	assert_eq!(back.valid_if, None);
	assert_eq!(back.completed_at, None);
}

#[test]
fn execution_plan_serialization_roundtrip() {
	let created_at = Utc::now();
	let plan = ExecutionPlan {
		title: "Release 1.0".to_string(),
		tasks: vec![
			sample_task(TaskStatus::Completed),
			PlanTask {
				title: "Ship it".to_string(),
				description: "Publish the release".to_string(),
				details: String::new(),
				summary: None,
				status: TaskStatus::InProgress,
				completed_at: None,
				message_range: None,
				phase: None,
				valid_if: None,
			},
		],
		current_task_index: 1,
		created_at,
		status: PlanStatus::Active,
	};

	let json = serde_json::to_string(&plan).expect("serialize ExecutionPlan");
	let back: ExecutionPlan = serde_json::from_str(&json).expect("deserialize ExecutionPlan");

	assert_eq!(back.title, "Release 1.0");
	assert_eq!(back.tasks.len(), 2);
	assert_eq!(back.current_task_index, 1);
	assert_eq!(back.created_at, created_at);
	assert!(matches!(back.status, PlanStatus::Active));

	assert_eq!(back.tasks[0].title, "Implement feature");
	assert!(matches!(back.tasks[0].status, TaskStatus::Completed));
	assert_eq!(back.tasks[1].title, "Ship it");
	assert!(matches!(back.tasks[1].status, TaskStatus::InProgress));
	assert_eq!(back.tasks[1].summary, None);
}
