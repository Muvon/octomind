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

//! Additional tests for `src/agent/taps.rs` — taps.toml persistence, add/remove
//! lifecycle, agent tag discovery, and workflow fetch. Filesystem-touching
//! tests sandbox `OCTOMIND_DATA_DIR` and must stay `#[serial]` because env
//! vars are process-global.

use super::*;
use serial_test::serial;

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir for the duration of a test,
/// restoring the previous value on drop.
struct DataDirGuard {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl DataDirGuard {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("failed to create tempdir");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for DataDirGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

/// Pre-create the default tap directory so `ensure_default_tap` takes the
/// already-cloned branch (git pull failure is silently ignored) and no network
/// clone is attempted.
fn create_default_tap() -> PathBuf {
	let dir = Tap {
		name: DEFAULT_TAP.to_string(),
		local_path: None,
	}
	.local_dir()
	.expect("default tap dir");
	fs::create_dir_all(&dir).expect("create default tap dir");
	dir
}

fn write_taps(taps: Vec<Tap>) {
	write_taps_file(&TapsFile { taps }).expect("write taps file");
}

fn tap_dir_for(name: &str) -> PathBuf {
	Tap {
		name: name.to_string(),
		local_path: None,
	}
	.local_dir()
	.expect("tap dir")
}

// --- Tap directory layout -------------------------------------------------

#[serial]
#[test]
fn tap_directory_helpers_use_standard_layout() {
	let _data = DataDirGuard::new();
	let tap = Tap {
		name: "alice/tools".to_string(),
		local_path: None,
	};
	let dir = tap.local_dir().expect("local dir");
	let data = crate::directories::get_octomind_data_dir().unwrap();
	assert_eq!(dir, data.join("taps").join("alice").join("octomind-tools"));
	assert_eq!(tap.agents_dir().unwrap(), dir.join("agents"));
	assert_eq!(tap.deps_dir().unwrap(), dir.join("deps"));
	assert_eq!(tap.skills_dir().unwrap(), dir.join("skills"));
	assert_eq!(tap.workflows_dir().unwrap(), dir.join("workflows"));
	assert_eq!(tap.plugins_dir().unwrap(), dir.join("plugins"));
}

#[serial]
#[test]
fn tap_directory_helpers_reject_invalid_names() {
	for name in ["plain", "a/b/c", ""] {
		let tap = Tap {
			name: name.to_string(),
			local_path: None,
		};
		assert!(tap.local_dir().is_err(), "{name:?} must be rejected");
		assert!(
			tap.agents_dir().is_err(),
			"{name:?} agents dir must fail too"
		);
		assert!(
			tap.workflows_dir().is_err(),
			"{name:?} workflows dir must fail too"
		);
	}
}

// --- parse_tap_arg edge cases ----------------------------------------------

#[test]
fn parse_tap_arg_trims_local_path_whitespace() {
	let tap = parse_tap_arg("user/repo").expect("plain GitHub tap");
	assert_eq!(tap.name, "user/repo");
	assert!(tap.local_path.is_none());

	let local = parse_tap_arg("user/repo   /some/path  ").expect("local tap");
	assert_eq!(local.name, "user/repo");
	assert_eq!(local.local_path.as_deref(), Some("/some/path"));

	// The name is taken verbatim up to the first space: leading whitespace
	// yields an empty name and must be rejected.
	assert!(parse_tap_arg(" user/repo").is_err());
	assert!(parse_tap_arg("a/b/c /path").is_err());
	assert!(parse_tap_arg("").is_err());
}

// --- taps.toml persistence -------------------------------------------------

#[serial]
#[test]
fn taps_file_roundtrip_and_missing_file_default() {
	let _data = DataDirGuard::new();
	// No taps.toml yet → empty tap list, not an error.
	assert!(read_taps_file().unwrap().taps.is_empty());
	assert!(list_taps().unwrap().is_empty());

	write_taps(vec![Tap {
		name: "alice/tools".to_string(),
		local_path: Some("/tmp/x".to_string()),
	}]);
	let taps = read_taps_file().unwrap();
	assert_eq!(taps.taps.len(), 1);
	assert_eq!(taps.taps[0].name, "alice/tools");
	assert_eq!(taps.taps[0].local_path.as_deref(), Some("/tmp/x"));
}

#[serial]
#[test]
fn malformed_taps_file_is_an_error() {
	let _data = DataDirGuard::new();
	let path = taps_file_path().unwrap();
	fs::write(&path, "not = [valid\ntoml").unwrap();
	assert!(read_taps_file().is_err());
	assert!(list_taps().is_err());
	assert!(get_taps().is_err());
}

#[serial]
#[test]
fn get_taps_appends_builtin_default_last() {
	let _data = DataDirGuard::new();
	write_taps(vec![Tap {
		name: "alice/tools".to_string(),
		local_path: None,
	}]);
	let taps = get_taps().unwrap();
	assert_eq!(taps.len(), 2);
	assert_eq!(taps[0].name, "alice/tools");
	assert_eq!(taps[1].name, DEFAULT_TAP);
}

// --- add_tap ----------------------------------------------------------------

#[serial]
#[test]
fn add_tap_rejects_invalid_names_default_and_duplicates() {
	let _data = DataDirGuard::new();

	let err = add_tap("plainname").unwrap_err();
	assert!(err.to_string().contains("user/repo"), "{err}");

	let err = add_tap(DEFAULT_TAP).unwrap_err();
	assert!(err.to_string().contains("built-in default"), "{err}");

	let src = tempfile::tempdir().unwrap();
	add_tap(&format!("alice/tools {}", src.path().display())).expect("add local tap");
	let err = add_tap(&format!("alice/tools {}", src.path().display())).unwrap_err();
	assert!(err.to_string().contains("already added"), "{err}");
}

#[serial]
#[test]
fn add_tap_local_symlinks_and_persists() {
	let _data = DataDirGuard::new();
	let src = tempfile::tempdir().unwrap();
	add_tap(&format!("alice/tools {}", src.path().display())).expect("add local tap");

	let dir = tap_dir_for("alice/tools");
	let meta = dir.symlink_metadata().expect("symlink metadata");
	assert!(meta.file_type().is_symlink(), "tap dir must be a symlink");
	assert_eq!(fs::read_link(&dir).unwrap(), src.path());

	let listed = list_taps().unwrap();
	assert_eq!(listed.len(), 1);
	assert_eq!(listed[0].name, "alice/tools");
	assert_eq!(
		listed[0].local_path.as_deref(),
		Some(src.path().to_str().unwrap())
	);
}

#[serial]
#[test]
fn add_tap_local_missing_directory_errors() {
	let _data = DataDirGuard::new();
	let missing = std::env::temp_dir().join("octomind-definitely-missing-tap-src");
	let err = add_tap(&format!("alice/tools {}", missing.display())).unwrap_err();
	assert!(err.to_string().contains("does not exist"), "{err}");
}

#[cfg(unix)]
#[serial]
#[test]
fn add_tap_local_replaces_stale_symlink() {
	let _data = DataDirGuard::new();
	let dir = tap_dir_for("alice/tools");
	fs::create_dir_all(dir.parent().unwrap()).expect("create tap parent dir");
	// A dangling symlink left behind by a removed local tap: `exists()` is
	// false for it, but `symlink_metadata()` sees it, so add_tap removes it
	// before creating the fresh symlink.
	std::os::unix::fs::symlink("/nonexistent-stale-target", &dir).unwrap();
	assert!(!dir.exists());
	assert!(dir.symlink_metadata().is_ok());

	let src = tempfile::tempdir().unwrap();
	add_tap(&format!("alice/tools {}", src.path().display())).expect("replace stale symlink");
	assert!(dir.is_symlink(), "stale symlink must be replaced");
	assert_eq!(fs::read_link(&dir).unwrap(), src.path());
}

// --- remove_tap --------------------------------------------------------------

#[serial]
#[test]
fn remove_tap_rejects_default_and_unknown() {
	let _data = DataDirGuard::new();
	let err = remove_tap(DEFAULT_TAP).unwrap_err();
	assert!(err.to_string().contains("cannot be removed"), "{err}");
	let err = remove_tap("ghost/repo").unwrap_err();
	assert!(err.to_string().contains("not in your tap list"), "{err}");
}

#[serial]
#[test]
fn remove_tap_local_removes_symlink_but_keeps_source() {
	let _data = DataDirGuard::new();
	let src = tempfile::tempdir().unwrap();
	add_tap(&format!("alice/tools {}", src.path().display())).unwrap();
	let dir = tap_dir_for("alice/tools");

	remove_tap("alice/tools").expect("remove local tap");
	assert!(list_taps().unwrap().is_empty());
	assert!(
		dir.symlink_metadata().is_err(),
		"symlink must be removed with the tap"
	);
	assert!(src.path().exists(), "source directory must survive");
}

#[serial]
#[test]
fn remove_tap_github_leaves_clone_on_disk() {
	let _data = DataDirGuard::new();
	let dir = tap_dir_for("gh/repo");
	fs::create_dir_all(&dir).unwrap();
	write_taps(vec![Tap {
		name: "gh/repo".to_string(),
		local_path: None,
	}]);

	remove_tap("gh/repo").expect("remove GitHub tap");
	assert!(list_taps().unwrap().is_empty());
	assert!(dir.exists(), "GitHub clone is intentionally left on disk");
}

// --- list_agent_tags ----------------------------------------------------------

#[serial]
#[test]
fn list_agent_tags_discovers_sorts_and_skips_non_toml() {
	let _data = DataDirGuard::new();
	let src = tempfile::tempdir().unwrap();
	let agents = src.path().join("agents");
	fs::create_dir_all(agents.join("developer")).unwrap();
	fs::create_dir_all(agents.join("reviewer")).unwrap();
	fs::write(agents.join("developer").join("general.toml"), "").unwrap();
	fs::write(agents.join("developer").join("extra.toml"), "").unwrap();
	fs::write(agents.join("developer").join("notes.txt"), "").unwrap();
	fs::write(agents.join("reviewer").join("main.toml"), "").unwrap();
	// A plain file in the agents dir is not a category — skipped.
	fs::write(agents.join("stray.toml"), "").unwrap();
	add_tap(&format!("alice/tools {}", src.path().display())).unwrap();

	let tags = list_agent_tags().unwrap();
	assert_eq!(
		tags,
		vec!["developer:extra", "developer:general", "reviewer:main"]
	);
}

#[serial]
#[test]
fn list_agent_tags_first_tap_wins_on_duplicates() {
	let _data = DataDirGuard::new();
	assert!(list_agent_tags().unwrap().is_empty(), "no taps → no tags");

	let first = tempfile::tempdir().unwrap();
	let second = tempfile::tempdir().unwrap();
	for (src, name) in [(&first, "alice/one"), (&second, "bob/two")] {
		let shared = src.path().join("agents").join("shared");
		fs::create_dir_all(&shared).unwrap();
		fs::write(shared.join("x.toml"), "").unwrap();
		add_tap(&format!("{name} {}", src.path().display())).unwrap();
	}

	let tags = list_agent_tags().unwrap();
	assert_eq!(tags, vec!["shared:x"], "duplicate tag must appear once");
}

// --- load_taps / fetch_workflow ------------------------------------------------

#[serial]
#[test]
fn load_taps_lists_user_taps_then_default() {
	let _data = DataDirGuard::new();
	create_default_tap();

	let src = tempfile::tempdir().unwrap();
	add_tap(&format!("alice/tools {}", src.path().display())).unwrap();
	// A GitHub tap whose directory already exists is pulled silently (failure
	// ignored) — it must still be listed.
	let gh_dir = tap_dir_for("gh/repo");
	fs::create_dir_all(&gh_dir).unwrap();
	write_taps(vec![
		Tap {
			name: "alice/tools".to_string(),
			local_path: Some(src.path().to_string_lossy().into_owned()),
		},
		Tap {
			name: "gh/repo".to_string(),
			local_path: None,
		},
	]);

	let taps = load_taps().expect("load taps");
	let names: Vec<&str> = taps.iter().map(|t| t.name.as_str()).collect();
	assert_eq!(names, vec!["alice/tools", "gh/repo", DEFAULT_TAP]);
}

#[serial]
#[test]
fn fetch_workflow_reads_from_user_tap_before_default() {
	let _data = DataDirGuard::new();
	let default_dir = create_default_tap();
	let default_wf = default_dir.join("workflows");
	fs::create_dir_all(&default_wf).unwrap();
	fs::write(default_wf.join("greet.toml"), "name = \"default-greet\"").unwrap();

	let (content, source) = fetch_workflow("greet").expect("workflow from default tap");
	assert_eq!(content, "name = \"default-greet\"");
	assert_eq!(source, DEFAULT_TAP);

	let src = tempfile::tempdir().unwrap();
	let user_wf = src.path().join("workflows");
	fs::create_dir_all(&user_wf).unwrap();
	fs::write(user_wf.join("greet.toml"), "name = \"user-greet\"").unwrap();
	add_tap(&format!("alice/tools {}", src.path().display())).unwrap();

	let (content, source) = fetch_workflow("greet").expect("workflow from user tap");
	assert_eq!(content, "name = \"user-greet\"");
	assert_eq!(source, "alice/tools");
}

#[serial]
#[test]
fn fetch_workflow_missing_reports_lookup_detail() {
	let _data = DataDirGuard::new();
	create_default_tap();
	let err = fetch_workflow("does-not-exist").unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("does-not-exist"), "{msg}");
	assert!(msg.contains("workflows/does-not-exist.toml"), "{msg}");
}

// --- git helpers (local-path failures only — no network) ------------------------

#[test]
fn git_clone_failure_surfaces_error() {
	let dir = tempfile::tempdir().unwrap();
	// A local-looking path that is not a repository fails fast without network.
	let err = git_clone("octomind-no-such-local-repository", dir.path()).unwrap_err();
	assert!(err.to_string().contains("Failed to clone tap"), "{err}");
}

#[test]
fn git_pull_on_non_repository_is_ok() {
	let dir = tempfile::tempdir().unwrap();
	fs::create_dir_all(dir.path().join("sub")).unwrap();
	git_pull(&dir.path().to_path_buf()).expect("pull failures are logged, not propagated");
}
