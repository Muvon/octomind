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

/// Session-scoped routing: hints pushed inside a session scope land in that
/// session's accumulator, invisible to the global CLI bucket.
#[tokio::test]
async fn test_session_scoped_hint_routing() {
	crate::session::context::with_session_id("hint-test-scoped".to_string(), async {
		assert!(!has_hints());
		push_hint("prefer view over cat");
		push_hint("prefer view over cat");
		push_hint("use ranges for big files");
		assert!(has_hints());

		let drained = drain_hints();
		assert_eq!(
			drained,
			vec![
				"prefer view over cat".to_string(),
				"use ranges for big files".to_string()
			]
		);
		assert!(!has_hints());
	})
	.await;
}

/// Global CLI bucket (no session scope): push/drain/has route to the shared
/// accumulator and drain deduplicates while clearing.
#[serial_test::serial]
#[test]
fn test_global_hint_bucket_without_session() {
	// Start from a clean bucket so the assertions below are self-contained.
	let _ = drain_hints();

	assert!(!has_hints(), "clean global bucket must report no hints");
	push_hint("global hint one");
	push_hint("global hint one");
	push_hint("global hint two");
	assert!(has_hints(), "global bucket must report pending hints");

	let drained = drain_hints();
	assert_eq!(
		drained,
		vec!["global hint one".to_string(), "global hint two".to_string()],
		"drain must deduplicate and preserve insertion order"
	);
	assert!(!has_hints(), "drain must clear the global bucket");
	assert!(drain_hints().is_empty(), "second drain yields nothing");
}
