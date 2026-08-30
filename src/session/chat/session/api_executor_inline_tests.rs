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
use serial_test::serial;

#[serial]
#[tokio::test]
async fn outcome_credit_updates_only_materially_used_memory() {
	let _guard = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let data = tempfile::tempdir().unwrap();
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let role = "__credit_role";
	let project = "__credit_project";
	let backend = crate::supervisor::learning::backend::FileBackend;
	for content in [
		"exposed but unused",
		"materially used",
		"used without verdict",
	] {
		backend
			.store(&crate::supervisor::learning::Lesson {
				content: content.to_string(),
				role: role.to_string(),
				project: project.to_string(),
				created: chrono::Utc::now().to_rfc3339(),
				..Default::default()
			})
			.await
			.unwrap();
	}
	let mut session = ChatSession::for_tests(Vec::new());
	session.recalled_refs = vec![
		(
			"M1".to_string(),
			"exposed but unused".to_string(),
			role.to_string(),
			project.to_string(),
		),
		(
			"M2".to_string(),
			"materially used".to_string(),
			role.to_string(),
			project.to_string(),
		),
	];
	session.used_memory_ids.insert("M2".to_string());
	reinforce_recalled(&mut session, 0.05).await;
	session.recalled_refs = vec![(
		"M3".to_string(),
		"used without verdict".to_string(),
		role.to_string(),
		project.to_string(),
	)];
	session.used_memory_ids.insert("M3".to_string());
	reinforce_recalled(&mut session, 0.0).await;
	let memories = backend.retrieve_all(role, project).await.unwrap();
	let unused = memories
		.iter()
		.find(|memory| memory.content == "exposed but unused")
		.unwrap();
	let used = memories
		.iter()
		.find(|memory| memory.content == "materially used")
		.unwrap();
	let unused_importance = unused.importance;
	let used_importance = used.importance;
	let used_count = used.use_count;
	let used_last_used = used.last_used.clone();
	let neutral = memories
		.iter()
		.find(|memory| memory.content == "used without verdict")
		.unwrap();
	let neutral_importance = neutral.importance;
	let neutral_count = neutral.use_count;

	match previous {
		Some(value) => std::env::set_var("OCTOMIND_DATA_DIR", value),
		None => std::env::remove_var("OCTOMIND_DATA_DIR"),
	}
	assert_eq!(unused_importance, 0.5);
	assert!((used_importance - 0.55).abs() < f64::EPSILON);
	assert_eq!(used_count, 1);
	assert!(!used_last_used.is_empty());
	assert_eq!(neutral_importance, 0.5);
	assert_eq!(neutral_count, 1);
	assert_eq!(session.session.info.learning_stats.used, 2);
	assert_eq!(session.session.info.learning_stats.credit_positive, 1);
	assert_eq!(session.session.info.learning_stats.used_without_verdict, 1);
}

#[test]
fn turn_answer_joins_every_pass_oldest_first() {
	// Turn-boundary and tool-call filtering happen at the append/clear
	// sites (the ledger is state); assembly joins the passes in order.
	let answers = vec![
		"THE BRIEF".to_string(),
		"The link is grounded; the brief stands.".to_string(),
	];
	assert_eq!(
		current_turn_answer(&answers, 8192),
		format!("THE BRIEF{ANSWER_PART_SEPARATOR}The link is grounded; the brief stands.")
	);
}

#[test]
fn turn_answer_keeps_the_newest_pass_when_over_budget() {
	// The older pass alone exceeds the token budget; the newest is always kept.
	let old = "many different words fill the older pass ".repeat(50);
	let answers = vec![old, "the amendment".to_string()];
	assert_eq!(current_turn_answer(&answers, 16), "the amendment");
}

#[test]
fn pregate_feedback_is_domain_agnostic() {
	assert!(PREGATE_NOTE.contains("state changes"));
	assert!(PREGATE_NOTE.contains("domain-specific validator"));
	assert!(!PREGATE_NOTE.contains("code changes"));
	assert!(!PREGATE_NOTE.contains("build / test / lint"));
}

#[test]
fn system_managed_response_cannot_complete_the_latest_user_task() {
	use crate::supervisor::detect::SelfReport;

	assert!(claims_user_task_completion(
		true,
		Some(SelfReport::Done),
		false
	));
	assert!(claims_user_task_completion(true, None, true));
	assert!(!claims_user_task_completion(
		false,
		Some(SelfReport::Done),
		false
	));
	assert!(!claims_user_task_completion(false, None, true));
}
