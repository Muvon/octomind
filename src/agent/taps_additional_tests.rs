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
		Self { previous, _dir: dir }
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

struct EnvVarGuard {
	name: &'static str,
	previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
	fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
		let previous = std::env::var_os(name);
		std::env::set_var(name, value);
		Self { name, previous }
	}
}

impl Drop for EnvVarGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(value) => std::env::set_var(self.name, value),
			None => std::env::remove_var(self.name),
		}
	}
}

/// Name of the built-in tap as the lookups currently see it.
fn default_tap_name() -> String {
	built_in_tap().expect("built-in tap").name
}

/// Serve the built-in tap at `git_ref` for the length of a test, then go back to the
/// template defaults. The setting is process-wide, so users of it must be `#[serial]`.
struct BuiltInRefGuard;

impl BuiltInRefGuard {
	fn set(git_ref: &str) -> Self {
		set_built_in_tap(&RegistryConfig {
			git_ref: git_ref.to_string(),
			..RegistryConfig::default()
		});
		Self
	}
}

impl Drop for BuiltInRefGuard {
	fn drop(&mut self) {
		*BUILT_IN_TAP.write().unwrap_or_else(PoisonError::into_inner) = None;
	}
}

/// Pre-create the default tap directory so `ensure_default_tap` takes the
/// already-cloned branch (a directory without `.git` starts no refresh) and no
/// network clone is attempted.
fn create_default_tap() -> PathBuf {
	let dir = built_in_tap()
		.expect("built-in tap")
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
fn agent_manifests_list_each_namespace_as_its_base_and_its_variants() {
	let _data = DataDirGuard::new();
	let tap = Tap {
		name: "alice/tools".to_string(),
		local_path: None,
	};
	let root = tap.local_dir().expect("local dir");
	for file in [
		"agents/dev/default.toml",
		"agents/dev/rust.toml",
		"agents/dev/go.toml",
		"agents/acme/reviewer.toml",
	] {
		fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
		fs::write(root.join(file), "").unwrap();
	}
	fs::write(root.join("agents/dev/notes.md"), "").unwrap();
	fs::write(root.join("agents/flat.toml"), "").unwrap();

	let tags: Vec<String> = tap.agent_manifests().unwrap().into_iter().map(|(tag, _)| tag).collect();
	assert_eq!(tags, ["acme:reviewer", "dev", "dev:go", "dev:rust"]);
}

// Symlinks need elevated rights on Windows, so the alias rule is checked on unix only.
#[cfg(unix)]
#[serial]
#[test]
fn an_alias_tag_resolves_to_the_agent_its_symlink_points_to() {
	let _data = DataDirGuard::new();
	let tap = Tap {
		name: "alice/tools".to_string(),
		local_path: None,
	};
	let namespace_dir = tap.local_dir().expect("local dir").join("agents/app");
	fs::create_dir_all(&namespace_dir).unwrap();
	fs::write(namespace_dir.join("default.toml"), "").unwrap();
	fs::write(namespace_dir.join("editor.toml"), "").unwrap();
	std::os::unix::fs::symlink("default.toml", namespace_dir.join("chief.toml")).unwrap();
	std::os::unix::fs::symlink("editor.toml", namespace_dir.join("writer.toml")).unwrap();
	write_taps(vec![tap.clone()]);

	assert_eq!(resolve_alias_tag("app:chief").unwrap(), "app");
	assert_eq!(resolve_alias_tag("app:writer").unwrap(), "app:editor");
	assert_eq!(alias_tags_of("app").unwrap(), ["app:chief"]);
	assert_eq!(alias_tags_of("app:editor").unwrap(), ["app:writer"]);
	// Not an alias: an agent itself, a tag no tap has, a plain name.
	for tag in ["app", "app:editor", "app:missing", "plain"] {
		assert_eq!(resolve_alias_tag(tag).unwrap(), tag);
	}
	let listed: Vec<String> = tap.agent_manifests().unwrap().into_iter().map(|(tag, _)| tag).collect();
	assert_eq!(listed, ["app", "app:editor"], "an alias is never listed");
}

#[cfg(unix)]
#[serial]
#[test]
fn a_real_manifest_in_an_earlier_tap_is_not_redirected_by_a_later_alias() {
	let _data = DataDirGuard::new();
	let own = Tap {
		name: "alice/own".to_string(),
		local_path: None,
	};
	let own_dir = own.local_dir().expect("local dir").join("agents/app");
	fs::create_dir_all(&own_dir).unwrap();
	fs::write(own_dir.join("chief.toml"), "").unwrap();
	let renamed = Tap {
		name: "bob/renamed".to_string(),
		local_path: None,
	};
	let renamed_dir = renamed.local_dir().expect("local dir").join("agents/app");
	fs::create_dir_all(&renamed_dir).unwrap();
	fs::write(renamed_dir.join("default.toml"), "").unwrap();
	std::os::unix::fs::symlink("default.toml", renamed_dir.join("chief.toml")).unwrap();
	write_taps(vec![own, renamed]);

	assert_eq!(resolve_alias_tag("app:chief").unwrap(), "app:chief");
}

#[serial]
#[test]
fn agent_manifests_leave_out_names_a_run_would_reject() {
	let _data = DataDirGuard::new();
	let tap = Tap {
		name: "alice/tools".to_string(),
		local_path: None,
	};
	let root = tap.local_dir().expect("local dir");
	for file in [
		"agents/dev/default.toml",
		"agents/dev/my_agent.toml",
		"agents/dev/Rust.toml",
		"agents/dev/ok-1.toml",
		"agents/Foo/default.toml",
		"agents/1st/default.toml",
	] {
		fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
		fs::write(root.join(file), "").unwrap();
	}
	let tags: Vec<String> = tap.agent_manifests().unwrap().into_iter().map(|(tag, _)| tag).collect();
	assert_eq!(tags, ["dev", "dev:ok-1"]);
}

#[serial]
#[test]
fn a_path_is_never_built_from_an_invalid_tag() {
	let _data = DataDirGuard::new();
	let tap = Tap {
		name: "alice/tools".to_string(),
		local_path: None,
	};
	for bad in ["../../x", "a/b", "dev:../x", ""] {
		assert!(tap.agent_manifest(bad).is_err(), "{bad}");
		let err = explain_missing_agent(bad).unwrap_err().to_string();
		assert!(err.contains("Invalid agent tag"), "{bad}: {err}");
	}
	assert!(tap
		.agent_manifest("dev:rust@1.0")
		.unwrap()
		.ends_with("agents/dev/rust.toml"));
}

#[serial]
#[test]
fn skill_dirs_lists_a_namespace_once_when_a_longer_name_sorts_between_its_tags() {
	let _data = DataDirGuard::new();
	let tap = Tap {
		name: "alice/tools".to_string(),
		local_path: None,
	};
	let root = tap.local_dir().expect("local dir");
	// By tag the order is `ab`, `ab-c`, `ab:x`, so `ab` is split by `ab-c`.
	for file in ["agents/ab/default.toml", "agents/ab/x.toml", "agents/ab-c/default.toml"] {
		fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
		fs::write(root.join(file), "").unwrap();
	}
	fs::create_dir_all(root.join("agents/ab/skills/one")).unwrap();
	let owned: Vec<(Option<String>, String)> = tap
		.skill_dirs()
		.unwrap()
		.into_iter()
		.map(|skill| {
			(
				skill.owner,
				skill.dir.file_name().unwrap().to_string_lossy().into_owned(),
			)
		})
		.collect();
	assert_eq!(owned, [(Some("ab".to_string()), "one".to_string())]);
}

#[serial]
#[test]
fn skill_dirs_lists_shared_skills_first_then_each_agent_by_name() {
	let _data = DataDirGuard::new();
	let tap = Tap {
		name: "alice/tools".to_string(),
		local_path: None,
	};
	let root = tap.local_dir().expect("local dir");
	for dir in [
		"skills/s-b",
		"skills/s-a",
		"agents/zeta/skills/z-1",
		"agents/alpha/skills/a-2",
		"agents/alpha/skills/a-1",
		"agents/no-manifest/skills/ignored",
	] {
		fs::create_dir_all(root.join(dir)).unwrap();
	}
	fs::write(root.join("skills/not-a-skill.txt"), "").unwrap();
	for agent in ["alpha", "zeta"] {
		fs::write(root.join("agents").join(agent).join("default.toml"), "").unwrap();
	}

	let found: Vec<(Option<String>, String)> = tap
		.skill_dirs()
		.unwrap()
		.into_iter()
		.map(|skill| {
			(
				skill.owner,
				skill.dir.file_name().unwrap().to_string_lossy().into_owned(),
			)
		})
		.collect();
	let owned = |agent: &str, skill: &str| (Some(agent.to_string()), skill.to_string());
	assert_eq!(
		found,
		vec![
			(None, "s-a".to_string()),
			(None, "s-b".to_string()),
			owned("alpha", "a-1"),
			owned("alpha", "a-2"),
			owned("zeta", "z-1"),
		],
		"a directory with no manifest owns nothing"
	);
}

#[serial]
#[test]
fn skill_dirs_of_a_tap_without_skills_is_empty() {
	let _data = DataDirGuard::new();
	let tap = Tap {
		name: "alice/tools".to_string(),
		local_path: None,
	};
	fs::create_dir_all(tap.local_dir().unwrap()).unwrap();
	assert!(tap.skill_dirs().unwrap().is_empty());
}

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
	assert_eq!(
		tap.agent_manifest("helper").unwrap(),
		dir.join("agents/helper/default.toml")
	);
	assert_eq!(
		tap.agent_manifest("helper:default").unwrap(),
		dir.join("agents/helper/default.toml")
	);
	assert_eq!(
		tap.agent_manifest("helper:rust").unwrap(),
		dir.join("agents/helper/rust.toml")
	);
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
		assert!(tap.agents_dir().is_err(), "{name:?} agents dir must fail too");
		assert!(tap.workflows_dir().is_err(), "{name:?} workflows dir must fail too");
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

#[test]
fn parse_tap_arg_rejects_empty_halves() {
	for name in ["foo/", "/repo", "/"] {
		assert!(parse_tap_arg(name).is_err(), "{name:?} must be rejected");
	}
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
	assert_eq!(taps[1].name, default_tap_name());
}

// --- add_tap ----------------------------------------------------------------

#[serial]
#[test]
fn add_tap_rejects_invalid_names_default_and_duplicates() {
	let _data = DataDirGuard::new();

	let err = add_tap("plainname").unwrap_err();
	assert!(err.to_string().contains("user/repo"), "{err}");

	let err = add_tap(&default_tap_name()).unwrap_err();
	assert!(err.to_string().contains("built-in tap"), "{err}");

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
	assert_eq!(listed[0].local_path.as_deref(), Some(src.path().to_str().unwrap()));
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
	let err = remove_tap(&default_tap_name()).unwrap_err();
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
	assert!(dir.symlink_metadata().is_err(), "symlink must be removed with the tap");
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
	fs::create_dir_all(&agents).unwrap();
	for name in ["developer", "assistant"] {
		fs::create_dir_all(agents.join(name)).unwrap();
		fs::write(agents.join(name).join("default.toml"), "").unwrap();
	}
	fs::write(agents.join("notes.txt"), "").unwrap();
	// A namespace without `default.toml` lists its variants only; a flat `<name>.toml` is no agent.
	fs::create_dir_all(agents.join("legacy")).unwrap();
	fs::write(agents.join("legacy").join("chief.toml"), "").unwrap();
	fs::write(agents.join("flat.toml"), "").unwrap();
	add_tap(&format!("alice/tools {}", src.path().display())).unwrap();

	let tags = list_agent_tags().unwrap();
	assert_eq!(tags, vec!["assistant", "developer", "legacy:chief"]);
}

#[serial]
#[test]
fn list_agent_tags_first_tap_wins_on_duplicates() {
	let _data = DataDirGuard::new();
	assert!(list_agent_tags().unwrap().is_empty(), "no taps → no tags");

	let first = tempfile::tempdir().unwrap();
	let second = tempfile::tempdir().unwrap();
	for (src, name) in [(&first, "alice/one"), (&second, "bob/two")] {
		let agent = src.path().join("agents").join("shared");
		fs::create_dir_all(&agent).unwrap();
		fs::write(agent.join("default.toml"), "").unwrap();
		add_tap(&format!("{name} {}", src.path().display())).unwrap();
	}

	let tags = list_agent_tags().unwrap();
	assert_eq!(tags, vec!["shared"], "duplicate tag must appear once");
}

// Symlinks need elevated rights on Windows, so the symlink rule is checked on unix only.
#[cfg(unix)]
#[serial]
#[test]
fn symlinked_agent_directory_is_not_an_agent() {
	let _data = DataDirGuard::new();
	let src = tempfile::tempdir().unwrap();
	write_agent(src.path(), "assistant", PLAIN_HEADER);
	let dir = src.path().join("agents");
	std::os::unix::fs::symlink(dir.join("assistant"), dir.join("old")).unwrap();
	// The tap itself is a symlink here (a local tap): its real manifests must still list.
	add_tap(&format!("alice/tools {}", src.path().display())).unwrap();

	assert_eq!(list_agent_tags().unwrap(), vec!["assistant"]);
	let headers = list_agent_headers().unwrap();
	assert_eq!(headers.len(), 1);
	assert_eq!(headers[0].tag, "assistant");
	assert!(!has_agent("old").unwrap(), "a symlink is not an agent");
	assert!(has_agent("assistant").unwrap());
}

// --- load_taps / fetch_workflow ------------------------------------------------

#[serial]
#[test]
fn load_taps_lists_user_taps_then_default() {
	let _data = DataDirGuard::new();
	create_default_tap();

	let src = tempfile::tempdir().unwrap();
	add_tap(&format!("alice/tools {}", src.path().display())).unwrap();
	// A GitHub tap whose directory already exists is only refreshed in the
	// background (and not at all without a `.git`) — it must still be listed.
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
	assert_eq!(names, vec!["alice/tools", "gh/repo", default_tap_name().as_str()]);
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
	assert_eq!(source, default_tap_name());

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
	let err = git_clone("octomind-no-such-local-repository", dir.path(), None).unwrap_err();
	assert!(err.to_string().contains("Failed to clone tap"), "{err}");
}

#[test]
#[serial]
fn sync_clone_never_touches_an_enclosing_repository() {
	// A tap dir left without `.git` (a failed clone) inside a dotfiles repo.
	let outer = tempfile::tempdir().unwrap();
	fs::write(outer.path().join("notes.txt"), "committed").unwrap();
	git_in(outer.path(), &["init"]);
	commit_all(outer.path(), "init");
	fs::write(outer.path().join("notes.txt"), "uncommitted edit").unwrap();
	let tap_dir = outer.path().join("taps").join("probe");
	fs::create_dir_all(&tap_dir).unwrap();

	let err = sync_clone(&tap_dir, None).unwrap_err();

	assert!(err.to_string().contains("is not a git clone"), "{err:#}");
	assert_eq!(
		fs::read_to_string(outer.path().join("notes.txt")).unwrap(),
		"uncommitted edit"
	);
}

#[test]
#[serial]
fn sync_clone_makes_a_dirty_diverged_clone_match_upstream() {
	// Prod 2026-11-13: an agent edited a manifest inside the default tap clone and
	// every later `pull --ff-only` failed, so the clone never saw a new agent.
	let remote = tempfile::tempdir().expect("remote");
	fs::write(remote.path().join("agent.toml"), "v1").expect("write v1");
	git_in(remote.path(), &["init"]);
	commit_all(remote.path(), "v1");

	let root = tempfile::tempdir().expect("clone root");
	let clone = root.path().join("tap");
	let url = format!("file://{}", remote.path().display());
	git_in(
		root.path(),
		&["clone", "--depth", "1", &url, clone.to_str().expect("utf-8 path")],
	);

	fs::write(remote.path().join("agent.toml"), "v2").expect("write v2");
	commit_all(remote.path(), "v2");

	fs::write(clone.join("committed.toml"), "local").expect("write local file");
	commit_all(&clone, "local commit");
	fs::write(clone.join("agent.toml"), "edited in place").expect("edit tracked file");
	fs::write(clone.join("untracked.toml"), "new").expect("write untracked file");

	sync_clone(&clone, None).expect("sync a dirty, diverged clone");

	assert_eq!(fs::read_to_string(clone.join("agent.toml")).unwrap(), "v2");
	assert!(!clone.join("committed.toml").exists(), "local commit discarded");
	assert!(!clone.join("untracked.toml").exists(), "untracked file removed");
}

#[test]
#[serial]
fn a_clone_follows_the_ref_it_is_given_and_a_missing_ref_removes_it() {
	let remote = tempfile::tempdir().expect("remote");
	fs::write(remote.path().join("agent.toml"), "default branch").expect("write default");
	git_in(remote.path(), &["init"]);
	commit_all(remote.path(), "init");
	git_in(remote.path(), &["checkout", "-b", "flat-agents"]);
	fs::write(remote.path().join("agent.toml"), "flat-agents").expect("write ref");
	commit_all(remote.path(), "flat");
	git_in(remote.path(), &["checkout", "-"]);
	let url = format!("file://{}", remote.path().display());

	let root = tempfile::tempdir().expect("clone root");
	let clone = root.path().join("tap");
	git_clone(&url, &clone, Some("flat-agents")).expect("clone at a ref");
	assert_eq!(fs::read_to_string(clone.join("agent.toml")).unwrap(), "flat-agents");

	// A clone that follows HEAD can be moved to the ref, and back.
	git_in(&clone, &["checkout", "-q", "--detach"]);
	sync_clone(&clone, None).expect("sync to HEAD");
	assert_eq!(fs::read_to_string(clone.join("agent.toml")).unwrap(), "default branch");
	sync_clone(&clone, Some("flat-agents")).expect("sync to the ref");
	assert_eq!(fs::read_to_string(clone.join("agent.toml")).unwrap(), "flat-agents");

	let missing = root.path().join("missing");
	let err = git_clone(&url, &missing, Some("no-such-ref")).unwrap_err();
	assert!(err.to_string().contains("no-such-ref"), "{err:#}");
	assert!(!missing.exists(), "a clone that cannot reach the ref is removed");
}

#[cfg(unix)]
#[test]
#[serial]
fn sync_clone_leaves_a_symlinked_tap_alone() {
	let target = tempfile::tempdir().expect("local tap source");
	let root = tempfile::tempdir().expect("link root");
	let link = root.path().join("tap");
	std::os::unix::fs::symlink(target.path(), &link).expect("symlink tap");

	sync_clone(&link, None).expect("a local tap is never synced");
}

/// Run `git` in `dir` with no user config prompts; panics on failure.
fn git_in(dir: &Path, args: &[&str]) {
	let status = std::process::Command::new("git")
		.args(args)
		.current_dir(dir)
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.status()
		.expect("run git");
	assert!(status.success(), "git {args:?} failed");
}

/// Stage everything in `dir` and commit it as a throwaway identity.
fn commit_all(dir: &Path, message: &str) {
	git_in(dir, &["add", "."]);
	git_in(
		dir,
		&[
			"-c",
			"user.name=test",
			"-c",
			"user.email=test@example.com",
			"-c",
			"commit.gpgsign=false",
			"commit",
			"-m",
			message,
		],
	);
}

// --- load_taps / list_agent_tags / fetch_workflow edge branches -------------

#[cfg(unix)]
fn chmod(path: &std::path::Path, mode: u32) {
	use std::os::unix::fs::PermissionsExt;
	fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set permissions");
}

#[test]
#[serial]
fn load_taps_skips_refresh_of_existing_github_tap_dirs_without_git() {
	let _guard = DataDirGuard::new();
	create_default_tap();
	// A GitHub-format tap (no local_path) whose directory exists but is not a
	// git repo: no refresh starts — load must still succeed.
	let gh_dir = tap_dir_for("probe/github");
	fs::create_dir_all(&gh_dir).expect("create github tap dir");
	write_taps(vec![Tap {
		name: "probe/github".to_string(),
		local_path: None,
	}]);

	let taps = load_taps().expect("load_taps survives a tap that cannot refresh");
	let names: Vec<&str> = taps.iter().map(|t| t.name.as_str()).collect();
	assert_eq!(
		names,
		vec!["probe/github", default_tap_name().as_str()],
		"user taps first, built-in default last"
	);
}

#[test]
#[serial]
#[cfg(unix)]
fn list_agent_tags_skips_unreadable_and_non_utf8_entries() {
	let _guard = DataDirGuard::new();
	let default_tap = create_default_tap();

	// A user tap whose agents dir cannot be read is skipped entirely.
	let locked_tap = tap_dir_for("probe/locked");
	let locked_agents = locked_tap.join("agents");
	fs::create_dir_all(locked_agents.join("hidden")).expect("create locked agents dir");
	fs::write(locked_agents.join("hidden").join("default.toml"), "x").expect("write hidden agent");
	chmod(&locked_agents, 0o000);
	write_taps(vec![Tap {
		name: "probe/locked".to_string(),
		local_path: None,
	}]);

	// Default tap: a good agent, a subdirectory, and a non-UTF-8 file name.
	let agents = default_tap.join("agents");
	fs::create_dir_all(&agents).expect("create agents dir");
	fs::create_dir_all(agents.join("good")).expect("create good agent dir");
	fs::write(agents.join("good").join("default.toml"), "x").expect("write good agent");
	fs::create_dir_all(agents.join("sealed")).expect("create sealed subdirectory");

	// Linux only: APFS rejects non-UTF-8 names with EILSEQ.
	#[cfg(target_os = "linux")]
	{
		use std::os::unix::ffi::OsStringExt;
		let non_utf8_dir = agents.join(std::ffi::OsString::from_vec(vec![0xfe, b'a']));
		fs::create_dir_all(&non_utf8_dir).expect("create non-utf8 agent dir");
		fs::write(non_utf8_dir.join("default.toml"), "x").expect("write non-utf8 agent");
	}

	let tags = list_agent_tags().expect("tag discovery succeeds");
	assert_eq!(
		tags,
		vec!["good".to_string()],
		"unreadable and non-UTF-8 entries are skipped"
	);

	chmod(&locked_agents, 0o755);
}

#[test]
#[serial]
fn fetch_workflow_falls_back_past_a_tap_with_invalid_name() {
	let _guard = DataDirGuard::new();
	let default_tap = create_default_tap();
	let workflows = default_tap.join("workflows");
	fs::create_dir_all(&workflows).expect("create workflows dir");
	fs::write(workflows.join("probe.toml"), "description = \"probe workflow\"\n").expect("write workflow");

	// A tap whose name is not `user/repo` cannot resolve a workflows dir and
	// must be skipped so later taps still answer.
	write_taps(vec![Tap {
		name: "not-a-tap-name".to_string(),
		local_path: None,
	}]);

	let (content, source) = fetch_workflow("probe").expect("workflow found via default tap");
	assert!(content.contains("probe workflow"));
	assert_eq!(source, default_tap_name());
}

// --- add_tap / remove_tap failure and lifecycle branches --------------------

#[test]
#[serial]
fn add_tap_rejects_duplicate_names() {
	let _guard = DataDirGuard::new();
	create_default_tap();
	write_taps(vec![Tap {
		name: "probe/one".to_string(),
		local_path: None,
	}]);

	let err = add_tap("probe/one").expect_err("duplicate tap must fail");
	assert!(err.to_string().contains("already added"), "got: {err:#}");
}

#[test]
#[serial]
fn add_tap_local_errors_when_parent_dir_cannot_be_created() {
	let _guard = DataDirGuard::new();
	create_default_tap();
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	// A file where `<data>/taps/probe` should be makes create_dir_all fail.
	let parent = data_dir.join("taps").join("probe");
	fs::write(&parent, "not a directory").expect("write blocker file");

	let target = tempfile::tempdir().expect("local tap source");
	let err =
		add_tap(&format!("probe/blocked {}", target.path().display())).expect_err("unwritable tap parent must fail");
	assert!(
		err.to_string().contains("Failed to create tap parent dir"),
		"got: {err:#}"
	);
}

#[test]
#[serial]
fn add_tap_local_errors_when_existing_tap_path_cannot_be_removed() {
	let _guard = DataDirGuard::new();
	create_default_tap();
	let tap_dir = tap_dir_for("probe/occupied");
	// A non-empty directory at the tap path: remove_file (the unix link
	// remover) fails with EISDIR.
	fs::create_dir_all(tap_dir.join("payload")).expect("create occupied tap dir");
	fs::write(tap_dir.join("payload").join("keep.txt"), "x").expect("write payload");

	let target = tempfile::tempdir().expect("local tap source");
	let err = add_tap(&format!("probe/occupied {}", target.path().display())).expect_err("occupied tap path must fail");
	assert!(
		err.to_string().contains("Failed to remove existing tap path"),
		"got: {err:#}"
	);
}

#[test]
#[serial]
#[cfg(unix)]
fn add_tap_local_errors_when_symlink_cannot_be_created() {
	let _guard = DataDirGuard::new();
	create_default_tap();
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	// A read-only parent lets create_dir_all succeed (dir exists) but denies
	// the symlink write.
	let parent = data_dir.join("taps").join("probe");
	fs::create_dir_all(&parent).expect("create tap parent");
	chmod(&parent, 0o555);

	let target = tempfile::tempdir().expect("local tap source");
	let err =
		add_tap(&format!("probe/readonly {}", target.path().display())).expect_err("read-only tap parent must fail");
	assert!(err.to_string().contains("Failed to create symlink"), "got: {err:#}");

	chmod(&parent, 0o755);
}

#[test]
#[serial]
fn add_tap_github_clone_failure_surfaces_git_error() {
	let _guard = DataDirGuard::new();
	create_default_tap();
	let _prompt = EnvVarGuard::set("GIT_TERMINAL_PROMPT", "0");
	let _count = EnvVarGuard::set("GIT_CONFIG_COUNT", "1");
	let _key = EnvVarGuard::set("GIT_CONFIG_KEY_0", "url.file:///octomind-test-missing-root/.insteadOf");
	let _value = EnvVarGuard::set("GIT_CONFIG_VALUE_0", "https://github.com/");

	// Rewrite GitHub to a guaranteed-missing local root: this exercises the
	// clone-error branch without network or an interactive credential prompt.
	let err = add_tap("octomind-probe/nonexistent-tap-repo").expect_err("clone of a nonexistent repo must fail");
	assert!(err.to_string().contains("Failed to clone tap from"), "got: {err:#}");
}

#[test]
#[serial]
fn add_tap_github_existing_dir_pulls_and_succeeds_on_non_repo() {
	let _guard = DataDirGuard::new();
	create_default_tap();
	let tap_dir = tap_dir_for("probe/stale");
	fs::create_dir_all(&tap_dir).expect("pre-create tap dir");

	// An existing directory that is not a git repo: the update path runs, the
	// sync failure is logged, and the tap is still registered.
	add_tap("probe/stale").expect("tap added despite failed pull");
	let taps = load_taps().expect("reload taps");
	assert!(
		taps.iter().any(|t| t.name == "probe/stale"),
		"tap persisted to taps.toml"
	);
}

#[test]
#[serial]
fn remove_tap_deletes_the_local_tap_symlink() {
	let _guard = DataDirGuard::new();
	create_default_tap();
	let target = tempfile::tempdir().expect("local tap source");
	fs::write(target.path().join("marker.txt"), "x").expect("write marker");

	add_tap(&format!("probe/local {}", target.path().display())).expect("local tap added");
	let tap_dir = tap_dir_for("probe/local");
	assert!(tap_dir.is_symlink(), "local tap is a symlink");

	remove_tap("probe/local").expect("tap removed");
	assert!(!tap_dir.symlink_metadata().is_ok(), "symlink removed with the tap");
	assert!(
		target.path().join("marker.txt").exists(),
		"the local target directory is untouched"
	);
}

// --- background refresh --------------------------------------------------------

fn wait_for(path: &std::path::Path) {
	for _ in 0..200 {
		if path.exists() {
			return;
		}
		std::thread::sleep(Duration::from_millis(50));
	}
	panic!("{} never appeared", path.display());
}

/// A `git` first on PATH that records each call and blocks until released, so a
/// test can see `load_taps` return while the pull is still running.
#[cfg(unix)]
struct FakeGit {
	dir: tempfile::TempDir,
	_path: EnvVarGuard,
}

#[cfg(unix)]
impl FakeGit {
	fn install() -> Self {
		let dir = tempfile::tempdir().expect("fake git dir");
		let script = format!(
			"#!/bin/sh\necho \"$@ prompt=$GIT_TERMINAL_PROMPT\" >> '{dir}/calls'\nwhile [ ! -e '{dir}/release' ]; do sleep 0.05; done\ntouch '{dir}/done'\n",
			dir = dir.path().display()
		);
		let path = dir.path().join("git");
		fs::write(&path, script).expect("write fake git");
		chmod(&path, 0o755);
		let search_path = format!("{}:{}", dir.path().display(), std::env::var("PATH").unwrap_or_default());
		Self {
			dir,
			_path: EnvVarGuard::set("PATH", search_path),
		}
	}

	fn calls_file(&self) -> PathBuf {
		self.dir.path().join("calls")
	}

	fn done_file(&self) -> PathBuf {
		self.dir.path().join("done")
	}

	fn release_and_wait(&self) {
		fs::write(self.dir.path().join("release"), "").expect("release fake git");
		wait_for(&self.done_file());
	}
}

/// A GitHub tap that is a git checkout (has `.git`) and has never fetched, so
/// its refresh is due; returns its `.git` dir.
fn write_due_github_tap(name: &str) -> PathBuf {
	let git_dir = tap_dir_for(name).join(".git");
	fs::create_dir_all(&git_dir).expect("create .git");
	write_taps(vec![Tap {
		name: name.to_string(),
		local_path: None,
	}]);
	git_dir
}

#[cfg(unix)]
#[test]
#[serial]
fn load_taps_returns_while_the_pull_is_still_running() {
	let _guard = DataDirGuard::new();
	create_default_tap();
	let git_dir = write_due_github_tap("probe/slow");
	let fake_git = FakeGit::install();

	load_taps().expect("load taps");

	assert!(git_dir.join(REFRESH_STAMP).exists(), "stamp is written before the sync");
	wait_for(&fake_git.calls_file());
	assert!(!fake_git.done_file().exists(), "load_taps must not wait for the sync");
	assert_eq!(
		fs::read_to_string(fake_git.calls_file()).unwrap().trim(),
		"fetch --depth 1 origin HEAD prompt=0"
	);
	fake_git.release_and_wait();
	assert!(wait_for_started_refreshes(), "the sync ends");
	assert_eq!(
		fs::read_to_string(fake_git.calls_file()).unwrap(),
		SYNC_CALLS,
		"fetch, then reset to it, then clean"
	);
}

/// The git calls of one [`sync_clone`], as [`FakeGit`] records them.
#[cfg(unix)]
const SYNC_CALLS: &str = "fetch --depth 1 origin HEAD prompt=0\nreset --hard FETCH_HEAD prompt=0\nclean -fd prompt=0\n";

#[cfg(unix)]
#[test]
#[serial]
fn back_to_back_load_taps_start_one_refresh() {
	let _guard = DataDirGuard::new();
	create_default_tap();
	write_due_github_tap("probe/once");
	let fake_git = FakeGit::install();

	load_taps().expect("first load");
	load_taps().expect("second load");

	wait_for(&fake_git.calls_file());
	fake_git.release_and_wait();
	assert!(wait_for_started_refreshes(), "the sync ends");
	assert_eq!(
		fs::read_to_string(fake_git.calls_file()).unwrap(),
		SYNC_CALLS,
		"the stamp must keep the second load from syncing again"
	);
}

#[test]
#[serial]
fn missing_default_tap_is_cloned_at_the_configured_ref_before_load_taps_returns() {
	let _guard = DataDirGuard::new();
	let _ref = BuiltInRefGuard::set("flat-agents");
	let remote_root = tempfile::tempdir().expect("remote root");
	let remote = remote_root.path().join("muvon").join("octomind-tap");
	fs::create_dir_all(&remote).expect("create remote");
	fs::write(remote.join("marker.txt"), "default branch").expect("write marker");
	git_in(&remote, &["init"]);
	commit_all(&remote, "init");
	// The configured ref is another branch, with other files.
	git_in(&remote, &["checkout", "-b", "flat-agents"]);
	fs::write(remote.join("marker.txt"), "flat-agents").expect("write ref marker");
	commit_all(&remote, "flat");
	git_in(&remote, &["checkout", "-"]);

	let _prompt = EnvVarGuard::set("GIT_TERMINAL_PROMPT", "0");
	let _count = EnvVarGuard::set("GIT_CONFIG_COUNT", "1");
	let _key = EnvVarGuard::set(
		"GIT_CONFIG_KEY_0",
		format!("url.file://{}/.insteadOf", remote_root.path().display()),
	);
	let _value = EnvVarGuard::set("GIT_CONFIG_VALUE_0", "https://github.com/");

	load_taps().expect("load taps clones the missing default tap");

	let default_dir = tap_dir_for(&default_tap_name());
	assert_eq!(
		fs::read_to_string(default_dir.join("marker.txt")).expect("the first run has the clone on disk"),
		"flat-agents",
		"the first clone ends on the configured ref, not on the default branch"
	);
}

// --- agent headers and lookup ----------------------------------------------

fn write_agent(tap_root: &Path, name: &str, header: &str) {
	let dir = tap_root.join("agents").join(name);
	fs::create_dir_all(&dir).expect("create agent dir");
	fs::write(dir.join("default.toml"), format!("{header}\n[[roles]]\n")).expect("write agent");
}

const PLAIN_HEADER: &str = "# Title: T\n# Description: D\n";

#[serial]
#[test]
fn list_agent_headers_reads_fields_sorted_and_first_tap_wins() {
	let _data = DataDirGuard::new();
	let first = tempfile::tempdir().unwrap();
	let second = tempfile::tempdir().unwrap();
	write_agent(first.path(), "shared", "# Title: First\n");
	write_agent(first.path(), "beta", PLAIN_HEADER);
	write_agent(second.path(), "shared", "# Title: Second\n");
	write_agent(second.path(), "alpha", PLAIN_HEADER);
	add_tap(&format!("alice/one {}", first.path().display())).unwrap();
	add_tap(&format!("bob/two {}", second.path().display())).unwrap();

	let agents = list_agent_headers().unwrap();
	let tags: Vec<&str> = agents.iter().map(|a| a.tag.as_str()).collect();
	assert_eq!(tags, vec!["alpha", "beta", "shared"]);

	let shared = &agents[2];
	assert_eq!(shared.header.title.as_deref(), Some("First"));
	assert!(shared.header.description.is_none());
	assert_eq!(agents[0].header.description.as_deref(), Some("D"));
}

#[serial]
#[test]
fn has_agent_finds_an_agent_by_its_flat_name() {
	let _data = DataDirGuard::new();
	let tap = tempfile::tempdir().unwrap();
	write_agent(tap.path(), "developer", PLAIN_HEADER);
	add_tap(&format!("alice/tools {}", tap.path().display())).unwrap();

	assert!(has_agent("developer").unwrap());
	assert!(!has_agent("nowhere").unwrap());
	assert!(
		!has_agent("developer:general").unwrap(),
		"a two-part tag names no agent"
	);
}

#[serial]
#[test]
fn user_tap_agent_wins_over_builtin_tap_agent() {
	let _data = DataDirGuard::new();
	write_agent(&create_default_tap(), "developer", "# Title: Builtin\n");
	let tap = tempfile::tempdir().unwrap();
	write_agent(tap.path(), "developer", "# Title: User\n");
	add_tap(&format!("alice/tools {}", tap.path().display())).unwrap();

	let agent = list_agent_headers()
		.unwrap()
		.into_iter()
		.find(|a| a.tag == "developer")
		.unwrap();
	assert_eq!(agent.header.title.as_deref(), Some("User"));
}

#[test]
#[serial]
fn concurrent_waiters_both_observe_one_in_flight_refresh() {
	let _guard = DataDirGuard::new();
	// Simulate a refresh in flight without git: the guard counts it until dropped.
	let refresh = crate::agent::taps::RefreshEndGuard::start();

	let waiter_a = std::thread::spawn(crate::agent::taps::wait_for_started_refreshes);
	let waiter_b = std::thread::spawn(crate::agent::taps::wait_for_started_refreshes);
	// Give both waiters time to park: neither may return while the refresh runs.
	std::thread::sleep(Duration::from_millis(200));
	assert!(
		!waiter_a.is_finished() && !waiter_b.is_finished(),
		"waiters returned while the refresh was still running"
	);
	drop(refresh);

	assert!(waiter_a.join().unwrap(), "first waiter must see the refresh end");
	assert!(waiter_b.join().unwrap(), "second waiter must see the refresh end");
}

#[test]
#[serial]
fn a_refresh_that_ended_before_the_wait_still_earns_a_retry() {
	// The lookup can read the clone just before the pull lands; the miss must still be retried.
	drop(crate::agent::taps::RefreshEndGuard::start());

	assert!(crate::agent::taps::wait_for_started_refreshes());
}
