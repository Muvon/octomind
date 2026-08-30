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

fn all_sources() -> Vec<InboxSource> {
	vec![
		InboxSource::Schedule { id: "s1".into() },
		InboxSource::Monitor {
			id: "m1".into(),
			description: "build".into(),
		},
		InboxSource::BackgroundAgent {
			name: "reviewer".into(),
		},
		InboxSource::TapRun {
			id: "t1".into(),
			role: "developer".into(),
		},
		InboxSource::Skill {
			name: "deploy".into(),
		},
		InboxSource::SkillValidator {
			name: "deploy".into(),
		},
		InboxSource::Inject,
		InboxSource::Webhook { hook: "ci".into() },
		InboxSource::GuardrailHook {
			script: "check.sh".into(),
		},
		InboxSource::GuardValidator {
			name: "tests".into(),
		},
	]
}

#[test]
fn every_source_kind_is_distinct_snake_case() {
	let kinds: Vec<&str> = all_sources().iter().map(|s| s.display_kind()).collect();
	let unique: std::collections::HashSet<&&str> = kinds.iter().collect();
	assert_eq!(unique.len(), kinds.len(), "duplicate kind in {kinds:?}");
	for kind in kinds {
		assert!(
			kind.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
			"'{kind}' is not snake_case — structured clients key on it"
		);
	}
}

#[test]
fn every_source_renders_a_label_and_an_icon() {
	for source in all_sources() {
		assert!(!source.display_label().is_empty());
		assert!(!source.display_icon().is_empty());
	}
}

#[test]
fn only_user_initiated_sources_are_not_system_managed() {
	// A schedule is user-authored configuration, but each firing is runtime
	// control input: it must not replace the underlying task during compression.
	assert!(InboxSource::Schedule { id: "s".into() }.is_system_managed());
	assert!(!InboxSource::Inject.is_system_managed());
	assert!(!InboxSource::Webhook { hook: "h".into() }.is_system_managed());
	for source in all_sources() {
		let expected = !matches!(source, InboxSource::Inject | InboxSource::Webhook { .. });
		assert_eq!(
			source.is_system_managed(),
			expected,
			"{} classified wrong",
			source.display_kind()
		);
	}
}

#[test]
fn coalesced_monitor_content_remains_bounded() {
	let mut content = "first".to_string();
	append_monitor_content(&mut content, &"x".repeat(10_000), 1024);
	assert!(content.len() <= 1024 + 2048);
	assert!(content.contains("additional monitor output omitted"));
	bound_monitor_content(&mut content, 1024);
	assert!(content.len() <= 1024 + 2048);
}
