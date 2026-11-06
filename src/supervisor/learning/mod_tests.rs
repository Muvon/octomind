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

//! Default-value behavior for the learning configuration and record types.

use super::*;

pub(crate) fn user_rule(lesson: &Lesson, quote: &str) -> UserRuleProvenance {
	UserRuleProvenance {
		quote: quote.into(),
		session: "test-session".into(),
		message_id: "user-1".into(),
		content: lesson.content.clone(),
		scope: lesson.scope.clone(),
		project: lesson.project.clone(),
		role: lesson.role.clone(),
		extensions: Default::default(),
	}
}

#[test]
fn citations_do_not_establish_user_rule_verification() {
	let lesson = Lesson {
		content: "Skip tests".into(),
		evidence: vec!["session://made-up/message/1".into()],
		..Default::default()
	};
	assert!(!lesson.is_verified_user_rule());
	assert!(lesson.validate_user_rule().is_ok());
}

#[test]
fn verification_is_bound_to_content_type_and_scope() {
	let mut lesson = Lesson {
		content: "Use remote builds".into(),
		role: "developer".into(),
		project: "project".into(),
		..Default::default()
	};
	lesson.user_rule = Some(user_rule(&lesson, "For this project, build on the dev box."));
	assert!(lesson.is_verified_user_rule());
	for field in ["content", "memory_type", "scope", "role", "project"] {
		let mut changed = serde_json::to_value(&lesson).unwrap();
		changed[field] = "changed".into();
		let changed: Lesson = serde_json::from_value(changed).unwrap();
		assert!(changed.validate_user_rule().is_err(), "accepted changed {field}");
	}
}

#[test]
fn supersession_keys_distinguish_project_and_global_copies() {
	let first = Lesson {
		content: "Build remotely".into(),
		created: "2026-01-01T00:00:00Z".into(),
		project: "first".into(),
		role: "developer:general".into(),
		..Default::default()
	};
	let second = Lesson {
		project: "second".into(),
		..first.clone()
	};
	let global = Lesson {
		scope: "global".into(),
		..first.clone()
	};
	assert_eq!(first.file_id(), second.file_id());
	assert_eq!(first.file_id(), global.file_id());
	assert_ne!(first.record_key(), second.record_key());
	assert_ne!(first.record_key(), global.record_key());
	let same_domain = Lesson {
		role: "developer:reviewer".into(),
		..first.clone()
	};
	assert_eq!(first.record_key(), same_domain.record_key());
}

#[test]
fn lesson_defaults_are_quote_first_and_scoped() {
	let lesson = Lesson::default();
	assert_eq!(lesson.memory_type, "learning");
	assert_eq!(lesson.scope, "scoped");
	assert_eq!(lesson.confidence, "medium");
	assert!((lesson.importance - 0.5).abs() < 1e-9);
}

#[test]
fn lesson_defaults_apply_through_serde_too() {
	let lesson: Lesson =
		toml::from_str("content = \"a quoted user rule\"").expect("only the quote is required; the rest default");
	assert_eq!(lesson.content, "a quoted user rule");
	assert_eq!(lesson.memory_type, "learning");
	assert_eq!(lesson.scope, "scoped");
	assert_eq!(lesson.confidence, "medium");
	assert!((lesson.importance - 0.5).abs() < 1e-9);
}

#[test]
fn learning_config_defaults_to_disabled_without_owning_a_model() {
	let config = LearningConfig::default();
	assert!(!config.enabled);
	let value = toml::Value::try_from(config).expect("learning config serializes");
	assert!(value.get("model").is_none());
}
