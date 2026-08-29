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

//! `octomind complete` command coverage: each subcommand arm runs against a
//! sandboxed `OCTOMIND_DATA_DIR` (tap enumeration is local-only, no network)
//! and the template config's roles.

use super::*;
use serial_test::serial;
use std::path::PathBuf;

const DATA_DIR_KEY: &str = "OCTOMIND_DATA_DIR";

/// Snapshot env vars and restore them on drop — a failed assert must not leak
/// a sandboxed data dir into the next test.
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

/// A fresh per-test data dir under the system temp dir.
fn sandbox(tag: &str) -> PathBuf {
	let dir = std::env::temp_dir().join(format!("octomind-cmp-{tag}-{}", std::process::id()));
	std::fs::create_dir_all(&dir).expect("create sandbox data dir");
	dir
}

fn template_config() -> Config {
	let config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config
}

fn complete_args(subcommand: &str) -> CompleteArgs {
	CompleteArgs {
		subcommand: subcommand.to_string(),
	}
}

#[test]
#[serial]
fn run_subcommand_lists_tap_tags_and_config_roles() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("run");
	std::env::set_var(DATA_DIR_KEY, &dir);

	// One agent tag from the default tap's agents tree.
	let agents = dir
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("agents")
		.join("dev");
	std::fs::create_dir_all(&agents).expect("agents dir");
	std::fs::write(agents.join("general.toml"), "name = \"general\"\n").expect("agent manifest");

	let config = template_config();
	assert!(!config.roles.is_empty(), "template ships roles");
	execute(&complete_args("run"), &config).expect("run completion is read-only");
}

#[test]
#[serial]
fn workflow_subcommand_lists_tap_workflows() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = sandbox("workflow");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let workflows = dir
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("workflows");
	std::fs::create_dir_all(&workflows).expect("workflows dir");
	std::fs::write(workflows.join("alpha.toml"), "description = \"a\"\n").expect("workflow");

	execute(&complete_args("workflow"), &template_config())
		.expect("workflow completion is read-only");
}

#[test]
fn unknown_subcommand_is_silent_success() {
	execute(&complete_args("bogus"), &template_config())
		.expect("unknown subcommands fall back to file completion");
}
