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
	let lesson: Lesson = toml::from_str("content = \"a quoted user rule\"")
		.expect("only the quote is required; the rest default");
	assert_eq!(lesson.content, "a quoted user rule");
	assert_eq!(lesson.memory_type, "learning");
	assert_eq!(lesson.scope, "scoped");
	assert_eq!(lesson.confidence, "medium");
	assert!((lesson.importance - 0.5).abs() < 1e-9);
}

#[test]
fn learning_config_defaults_to_disabled_with_the_recommended_model() {
	let config = LearningConfig::default();
	assert!(!config.enabled);
	assert_eq!(config.model, "anthropic:claude-haiku-4-5-20251001");
}
