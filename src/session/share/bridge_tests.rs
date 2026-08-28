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

#[test]
fn random_token_is_24_chars() {
	assert_eq!(random_token().len(), 24);
}

#[test]
fn random_token_is_alphanumeric() {
	let token = random_token();
	assert!(
		token.chars().all(|c| c.is_ascii_alphanumeric()),
		"non-alphanumeric char in token: {token}"
	);
}

#[test]
fn random_token_differs_between_calls() {
	let a = random_token();
	let b = random_token();
	assert_ne!(a, b, "two consecutive tokens must not collide");
}

#[test]
fn constant_time_eq_true_for_equal_slices() {
	assert!(constant_time_eq(b"bridge-token", b"bridge-token"));
	assert!(constant_time_eq(b"a", b"a"));
}

#[test]
fn constant_time_eq_false_for_unequal_slices() {
	assert!(!constant_time_eq(b"bridge-token", b"bridge-tokem"));
	assert!(!constant_time_eq(b"abc", b"abd"));
	// Same content, different case — still unequal bytes
	assert!(!constant_time_eq(b"Token", b"token"));
}

#[test]
fn constant_time_eq_false_for_different_lengths() {
	assert!(!constant_time_eq(b"short", b"longer-string"));
	assert!(!constant_time_eq(b"", b"x"));
	assert!(!constant_time_eq(b"x", b""));
}

#[test]
fn constant_time_eq_true_for_two_empty_slices() {
	assert!(constant_time_eq(b"", b""));
}

#[test]
fn bridge_info_supports_debug_and_clone() {
	let info = BridgeInfo {
		port: 8080,
		token: "abc123".to_string(),
	};
	let clone = info.clone();

	assert_eq!(clone.port, 8080);
	assert_eq!(clone.token, "abc123");

	let debug = format!("{info:?}");
	assert!(debug.contains("8080"), "Debug output missing port: {debug}");
	assert!(
		debug.contains("abc123"),
		"Debug output missing token: {debug}"
	);
}

#[test]
fn clear_for_session_is_noop_for_unknown_session() {
	// SessionId is a String alias; clearing an absent entry must not panic.
	let session_id: SessionId = "no-such-session".to_string();
	clear_for_session(&session_id);
}
