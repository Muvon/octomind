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
use rmcp::model::{CallToolResult, ContentBlock, Resource};

#[test]
fn extracts_resource_links_and_ignores_plain_text() {
	let launched = CallToolResult::success(vec![
		ContentBlock::text("Started background job. Follow the linked resource."),
		ContentBlock::resource_link(Resource::new(
			"octofs://jobs/1234-7",
			"background shell job",
		)),
	]);
	assert_eq!(
		resource_links_in(&launched),
		vec!["octofs://jobs/1234-7".to_string()]
	);

	// Recognition is not scheme-bound: any resource link is followed.
	let other = CallToolResult::success(vec![ContentBlock::resource_link(Resource::new(
		"custommcp://tasks/9",
		"task",
	))]);
	assert_eq!(
		resource_links_in(&other),
		vec!["custommcp://tasks/9".to_string()]
	);

	let plain = CallToolResult::success(vec![ContentBlock::text("just text, no resource")]);
	assert!(resource_links_in(&plain).is_empty());
}

#[test]
fn watch_complete_and_pending_roundtrip() {
	let sid = "shell-jobs-unit-test-session";
	clear_for_session(sid);
	assert!(!has_pending_for_session(sid));

	let a = "octofs://jobs/a-1";
	let b = "octofs://jobs/a-2";
	register_for_session(sid, a);
	register_for_session(sid, b);
	assert!(has_pending_for_session(sid));
	assert!(is_watched_for_session(sid, a));
	assert!(!is_watched_for_session(sid, "octofs://jobs/never"));

	assert!(complete_for_session(sid, a), "a was watched");
	assert!(!is_watched_for_session(sid, a));
	assert!(
		has_pending_for_session(sid),
		"b still keeps the session pending"
	);
	assert!(
		!complete_for_session(sid, "octofs://jobs/unknown"),
		"completing an unwatched uri reports not-watched"
	);

	assert!(complete_for_session(sid, b), "b was watched");
	assert!(
		!has_pending_for_session(sid),
		"the empty set is dropped, so the session is no longer pending"
	);
	assert!(
		!complete_for_session(sid, b),
		"already-cleared uri reports not-watched"
	);
}
