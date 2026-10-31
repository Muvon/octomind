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
fn test_github_url() {
	let tap = Tap {
		name: "muvon/tap".to_string(),
		local_path: None,
	};
	assert_eq!(tap.github_url(), "https://github.com/muvon/octomind-tap");

	// Names that are not user/repo fall back to being used verbatim
	let odd = Tap {
		name: "https://example.com/x.git".to_string(),
		local_path: None,
	};
	assert_eq!(odd.github_url(), "https://example.com/x.git");
}

#[test]
fn test_parse_tap_arg() {
	let tap = parse_tap_arg("user/repo").expect("valid GitHub tap");
	assert_eq!(tap.name, "user/repo");
	assert!(tap.local_path.is_none());

	let local = parse_tap_arg("user/repo /some/path").expect("valid local tap");
	assert_eq!(local.name, "user/repo");
	assert_eq!(local.local_path.as_deref(), Some("/some/path"));

	// Not user/repo format
	assert!(parse_tap_arg("plain").is_err());
	assert!(parse_tap_arg("a/b/c").is_err());
}

#[test]
fn test_expand_path() {
	let home = dirs::home_dir().expect("home dir in test env");
	assert_eq!(expand_path("~/x").expect("tilde"), home.join("x"));
	assert_eq!(expand_path("/abs/path").expect("absolute"), PathBuf::from("/abs/path"));
	let cwd = std::env::current_dir().expect("cwd");
	assert_eq!(expand_path("./rel").expect("relative"), cwd.join("rel"));
}

#[test]
fn tap_refresh_is_due_only_when_the_last_fetch_is_older_than_the_interval() {
	let tap = tempfile::tempdir().expect("tempdir");
	assert!(refresh_due(tap.path()), "a clone that never fetched refreshes at once");

	let git = tap.path().join(".git");
	fs::create_dir_all(&git).expect("create .git");
	let fetch_head = fs::File::create(git.join("FETCH_HEAD")).expect("create FETCH_HEAD");
	assert!(!refresh_due(tap.path()), "a fresh fetch skips the pull");

	let stale = SystemTime::now() - TAP_REFRESH_INTERVAL - Duration::from_secs(1);
	fetch_head.set_modified(stale).expect("age FETCH_HEAD");
	assert!(refresh_due(tap.path()), "an old fetch pulls again");
}

#[test]
fn a_started_refresh_counts_as_fresh_until_the_interval_passes() {
	let tap = tempfile::tempdir().expect("tempdir");
	let git = tap.path().join(".git");
	fs::create_dir_all(&git).expect("create .git");
	let stale = SystemTime::now() - TAP_REFRESH_INTERVAL - Duration::from_secs(1);

	let fetch_head = fs::File::create(git.join("FETCH_HEAD")).expect("create FETCH_HEAD");
	fetch_head.set_modified(stale).expect("age FETCH_HEAD");
	assert!(refresh_due(tap.path()), "an old fetch alone is due");

	let stamp = fs::File::create(git.join(REFRESH_STAMP)).expect("create stamp");
	assert!(!refresh_due(tap.path()), "a fresh stamp holds off another process");

	stamp.set_modified(stale).expect("age stamp");
	assert!(refresh_due(tap.path()), "an old stamp and an old fetch are due again");

	fetch_head.set_modified(SystemTime::now()).expect("refresh FETCH_HEAD");
	assert!(!refresh_due(tap.path()), "the newer of the two decides");
}

#[test]
fn locks_left_by_a_killed_git_are_removed_once_stale() {
	// Prod 2026-10-24: a pull killed mid-merge left ORIG_HEAD.lock and every
	// later pull failed for a week.
	let tap = tempfile::tempdir().expect("tempdir");
	let git = tap.path().join(".git");
	fs::create_dir_all(&git).expect("create .git");
	let stale = fs::File::create(git.join("ORIG_HEAD.lock")).expect("create stale lock");
	stale
		.set_modified(SystemTime::now() - STALE_LOCK_AGE - Duration::from_secs(1))
		.expect("age lock");
	fs::File::create(git.join("index.lock")).expect("create live lock");
	fs::File::create(git.join("FETCH_HEAD")).expect("create FETCH_HEAD");

	remove_stale_locks(&git);

	assert!(
		!git.join("ORIG_HEAD.lock").exists(),
		"a lock older than the limit has no owner"
	);
	assert!(
		git.join("index.lock").exists(),
		"a fresh lock may belong to a running git"
	);
	assert!(git.join("FETCH_HEAD").exists(), "only lock files are touched");
}
