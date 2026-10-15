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

//! Handler-level tests for the `/new` session command: bare switch to a
//! freshly named session, and title persistence under a throwaway data dir.

use super::*;

#[test]
fn new_without_title_switches_to_a_fresh_named_session() {
	let mut session = ChatSession::for_tests(Vec::new());
	let result = handle_new(&mut session, &[]).expect("handled");
	let CommandResult::SwitchSession(name) = result else {
		panic!("expected a session switch");
	};
	assert!(!name.is_empty(), "a session name is generated");
}

#[test]
#[serial_test::serial]
fn new_with_title_persists_the_title_for_the_new_session() {
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	let dir = tempfile::tempdir().expect("temporary data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", dir.path());

	let mut session = ChatSession::for_tests(Vec::new());
	let result = handle_new(&mut session, &["My", "Title"]).expect("handled");
	let CommandResult::SwitchSession(name) = result else {
		panic!("expected a session switch");
	};

	// Read the title store while the throwaway data dir is still active.
	let meta = crate::session::titles::get_session_meta(&name);
	match previous {
		Some(old) => std::env::set_var("OCTOMIND_DATA_DIR", old),
		None => std::env::remove_var("OCTOMIND_DATA_DIR"),
	}

	let meta = meta.expect("title metadata written for the new session");
	assert_eq!(meta.title.as_deref(), Some("My Title"));
	assert_eq!(meta.role.as_deref(), Some("core"));
	assert_eq!(meta.model.as_deref(), Some("anthropic/claude-3-5-sonnet"));
}
