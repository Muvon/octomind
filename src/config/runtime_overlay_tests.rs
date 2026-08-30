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
use std::sync::Mutex;

// Serialize all tests in this module — they share the process-global OVERLAY static.
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn fresh_registry() {
	if let Ok(mut r) = registry().write() {
		r.clear();
	}
}

#[test]
fn extras_unions_across_capabilities() {
	let _guard = TEST_LOCK.lock().unwrap();
	fresh_registry();
	let mut shell_map = HashMap::new();
	shell_map.insert("octofs".to_string(), vec!["shell".to_string()]);
	set_capability_extras("shell", shell_map);

	let mut fs_map = HashMap::new();
	fs_map.insert(
		"octofs".to_string(),
		vec!["text_editor".to_string(), "view".to_string()],
	);
	set_capability_extras("filesystem-write", fs_map);

	let mut got = extras_for_server("octofs");
	got.sort();
	let mut expected = vec![
		"shell".to_string(),
		"text_editor".to_string(),
		"view".to_string(),
	];
	expected.sort();
	assert_eq!(got, expected);
}

#[test]
fn clear_removes_only_named_capability() {
	let _guard = TEST_LOCK.lock().unwrap();
	fresh_registry();
	let mut a = HashMap::new();
	a.insert("svr".to_string(), vec!["one".to_string()]);
	set_capability_extras("a", a);

	let mut b = HashMap::new();
	b.insert("svr".to_string(), vec!["two".to_string()]);
	set_capability_extras("b", b);

	clear_capability_extras("a");

	let got = extras_for_server("svr");
	assert_eq!(got, vec!["two".to_string()]);
}

#[test]
fn empty_per_server_is_treated_as_clear() {
	let _guard = TEST_LOCK.lock().unwrap();
	fresh_registry();
	let mut a = HashMap::new();
	a.insert("svr".to_string(), vec!["one".to_string()]);
	set_capability_extras("a", a);

	// Calling with empty map removes the capability rather than
	// inserting an empty entry — keeps the registry tight.
	set_capability_extras("a", HashMap::new());
	assert!(extras_for_server("svr").is_empty());
}

#[test]
fn unknown_server_returns_empty() {
	let _guard = TEST_LOCK.lock().unwrap();
	fresh_registry();
	assert!(extras_for_server("never-seen").is_empty());
}

#[test]
fn duplicates_within_one_server_are_deduped() {
	let _guard = TEST_LOCK.lock().unwrap();
	fresh_registry();
	let mut a = HashMap::new();
	a.insert(
		"svr".to_string(),
		vec!["x".to_string(), "y".to_string(), "x".to_string()],
	);
	set_capability_extras("a", a);

	let mut b = HashMap::new();
	b.insert("svr".to_string(), vec!["y".to_string(), "z".to_string()]);
	set_capability_extras("b", b);

	let got = extras_for_server("svr");
	assert_eq!(got.len(), 3);
	assert!(got.contains(&"x".to_string()));
	assert!(got.contains(&"y".to_string()));
	assert!(got.contains(&"z".to_string()));
}
