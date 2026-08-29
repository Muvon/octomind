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

//! Tests for `octomind tap`: argument parsing plus add/list flows against a
//! sandboxed data dir. Local-path taps avoid git entirely; the GitHub-clone
//! branch needs the network and is deliberately not exercised here.

use super::*;
use clap::Parser;
use octomind::agent::taps;
use serial_test::serial;

const DATA_DIR_KEY: &str = "OCTOMIND_DATA_DIR";

/// Snapshot env vars and restore them on drop.
struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
	fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}

impl Drop for EnvGuard {
	fn drop(&mut self) {
		for (key, saved) in &self.0 {
			match saved {
				Some(v) => std::env::set_var(key, v),
				None => std::env::remove_var(key),
			}
		}
	}
}

fn sandbox(tag: &str) -> std::path::PathBuf {
	let dir = std::env::temp_dir().join(format!("octomind-tap-{tag}-{}", std::process::id()));
	if dir.exists() {
		std::fs::remove_dir_all(&dir).expect("clear stale sandbox data dir");
	}
	std::fs::create_dir_all(&dir).expect("create sandbox data dir");
	dir
}

#[derive(clap::Parser)]
struct Cli {
	#[command(flatten)]
	args: TapArgs,
}

#[test]
fn tap_args_parse_optional_positionals() {
	let cli = Cli::try_parse_from(["octomind"]).expect("bare tap lists");
	assert!(cli.args.tap.is_none());
	assert!(cli.args.local_path.is_none());

	let cli = Cli::try_parse_from(["octomind", "myorg/repo"]).expect("tap only");
	assert_eq!(cli.args.tap.as_deref(), Some("myorg/repo"));
	assert!(cli.args.local_path.is_none());

	let cli =
		Cli::try_parse_from(["octomind", "myorg/repo", "./local"]).expect("tap with local path");
	assert_eq!(cli.args.tap.as_deref(), Some("myorg/repo"));
	assert_eq!(cli.args.local_path.as_deref(), Some("./local"));
}

#[test]
#[serial]
fn execute_lists_taps_when_none_is_given() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("list-empty");
	std::env::set_var(DATA_DIR_KEY, &dir);

	execute(&TapArgs {
		tap: None,
		local_path: None,
	})
	.expect("empty listing renders the built-in tap row");
}

#[test]
#[serial]
fn execute_adds_a_local_tap_and_symlinks_it() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("add-local");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let local = tempfile::tempdir().expect("local tap dir");
	let local_str = local.path().to_string_lossy().to_string();

	execute(&TapArgs {
		tap: Some("testorg/probe".to_string()),
		local_path: Some(local_str.clone()),
	})
	.expect("local tap add succeeds");

	let listed = taps::list_taps().expect("list taps");
	assert_eq!(listed.len(), 1, "{listed:?}");
	assert_eq!(listed[0].name, "testorg/probe");
	assert_eq!(listed[0].local_path.as_deref(), Some(local_str.as_str()));

	// The tap dir is a symlink to the live local directory.
	let tap_dir = dir.join("taps").join("testorg").join("octomind-probe");
	assert!(
		tap_dir.symlink_metadata().is_ok(),
		"symlink at {}",
		tap_dir.display()
	);
	assert_eq!(tap_dir.read_link().expect("symlink target"), local.path());
}

#[test]
#[serial]
fn execute_lists_an_added_tap_with_its_local_suffix() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("list-added");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let local = tempfile::tempdir().expect("local tap dir");

	execute(&TapArgs {
		tap: Some("testorg/probe".to_string()),
		local_path: Some(local.path().to_string_lossy().to_string()),
	})
	.expect("add tap");

	// Non-empty listing path: user section rows plus the built-in row.
	execute(&TapArgs {
		tap: None,
		local_path: None,
	})
	.expect("listing renders added taps");
}

#[test]
#[serial]
fn execute_rejects_the_builtin_default_tap() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("add-default");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let err = execute(&TapArgs {
		tap: Some(taps::DEFAULT_TAP.to_string()),
		local_path: None,
	})
	.expect_err("default tap cannot be re-added");
	assert!(err.to_string().contains("built-in default tap"), "{err}");
}

#[test]
#[serial]
fn execute_rejects_malformed_and_duplicate_taps() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("add-bad");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let err = execute(&TapArgs {
		tap: Some("plain".to_string()),
		local_path: None,
	})
	.expect_err("malformed tap refused")
	.to_string();
	assert!(err.contains("user/repo"), "{err}");

	let local = tempfile::tempdir().expect("local tap dir");
	execute(&TapArgs {
		tap: Some("testorg/probe".to_string()),
		local_path: Some(local.path().to_string_lossy().to_string()),
	})
	.expect("first add succeeds");

	let err = execute(&TapArgs {
		tap: Some("testorg/probe".to_string()),
		local_path: Some(local.path().to_string_lossy().to_string()),
	})
	.expect_err("duplicate add refused");
	assert!(err.to_string().contains("already added"), "{}", err);
}
