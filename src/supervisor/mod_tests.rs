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

//! Behavior tests for the small public surface of `supervisor/mod.rs`.

use super::*;

#[test]
fn verification_policy_as_str_covers_every_variant() {
	assert_eq!(
		crate::supervisor::VerificationPolicy::Unspecified.as_str(),
		"unspecified"
	);
	assert_eq!(
		crate::supervisor::VerificationPolicy::Forbidden.as_str(),
		"forbidden"
	);
	assert_eq!(
		crate::supervisor::VerificationPolicy::Allowed.as_str(),
		"allowed"
	);
}

#[test]
fn role_context_returns_the_system_message_trimmed() {
	let messages = vec![
		crate::session::Message {
			role: "user".to_string(),
			content: "a user turn".to_string(),
			..Default::default()
		},
		crate::session::Message {
			role: "system".to_string(),
			content: "  you are a careful engineer  ".to_string(),
			..Default::default()
		},
	];
	assert_eq!(role_context(&messages), "you are a careful engineer");
}

#[test]
fn role_context_without_a_system_message_is_empty() {
	let messages = vec![crate::session::Message {
		role: "user".to_string(),
		content: "just asking".to_string(),
		..Default::default()
	}];
	assert_eq!(role_context(&messages), "");
}

#[test]
fn role_context_caps_an_oversized_system_message_at_the_boundary() {
	let long: String = "rule ".repeat(2_000);
	assert!(long.chars().count() > ROLE_CONTEXT_CHARS);
	let messages = vec![crate::session::Message {
		role: "system".to_string(),
		content: long.clone(),
		..Default::default()
	}];
	let capped = role_context(&messages);
	assert_eq!(capped.chars().count(), ROLE_CONTEXT_CHARS);
	assert!(capped.starts_with("rule rule"));
}

#[test]
fn notify_never_panics_outside_a_terminal() {
	// Under the test harness stderr is piped, so notify takes its early
	// return; the call itself is the behavior under test.
	notify("line one\nline two");
}
