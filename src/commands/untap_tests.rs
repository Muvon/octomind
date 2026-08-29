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

//! Tests for `octomind untap`: argument parsing plus removal flows against a
//! sandboxed data dir, seeded through the same `taps::add_tap` the CLI uses.

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
	let dir = std::env::temp_dir().join(format!("octomind-untap-{tag}-{}", std::process::id()));
	if dir.exists() {
		std::fs::remove_dir_all(&dir).expect("clear stale sandbox data dir");
	}
	std::fs::create_dir_all(&dir).expect("create sandbox data dir");
	dir
}

#[derive(clap::Parser)]
struct Cli {
	#[command(flatten)]
	args: UntapArgs,
}

#[test]
fn untap_args_require_a_name() {
	let cli = Cli::try_parse_from(["octomind", "myorg/repo"]).expect("name parses");
	assert_eq!(cli.args.name, "myorg/repo");

	assert!(
		Cli::try_parse_from(["octomind"]).is_err(),
		"the tap name is required"
	);
}

#[test]
#[serial]
fn execute_removes_an_added_tap_and_its_symlink() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("remove");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let local = tempfile::tempdir().expect("local tap dir");
	taps::add_tap(&format!("testorg/probe {}", local.path().to_string_lossy())).expect("seed tap");
	let tap_dir = dir.join("taps").join("testorg").join("octomind-probe");
	assert!(tap_dir.symlink_metadata().is_ok(), "symlink seeded");

	execute(&UntapArgs {
		name: "testorg/probe".to_string(),
	})
	.expect("untap succeeds");

	assert!(
		taps::list_taps().expect("list taps").is_empty(),
		"tap list drained"
	);
	assert!(
		tap_dir.symlink_metadata().is_err(),
		"local tap symlink removed with the entry"
	);
}

#[test]
#[serial]
fn execute_rejects_unknown_taps() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("unknown");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let err = execute(&UntapArgs {
		name: "ghost/org".to_string(),
	})
	.expect_err("unknown tap refused");
	assert!(err.to_string().contains("not in your tap list"), "{err}");
}

#[test]
#[serial]
fn execute_rejects_the_builtin_default_tap() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("default");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let err = execute(&UntapArgs {
		name: taps::DEFAULT_TAP.to_string(),
	})
	.expect_err("default tap is protected");
	assert!(err.to_string().contains("cannot be removed"), "{err}");
}
