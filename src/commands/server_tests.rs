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

//! Tests for `octomind server`: argument parsing, tag resolution failures,
//! and a full startup against an ephemeral port with a sandboxed data dir.

use super::*;
use clap::Parser;
use octomind::Config;
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

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

#[derive(clap::Parser)]
struct Cli {
	#[command(flatten)]
	args: ServerArgs,
}

#[test]
fn server_args_default_host_port_and_parse_overrides() {
	let cli = Cli::try_parse_from(["octomind"]).expect("bare server parses");
	assert_eq!(cli.args.tag, None);
	assert_eq!(cli.args.host, "127.0.0.1");
	assert_eq!(cli.args.port, 8080);
	assert!(!cli.args.sandbox);
	assert!(cli.args.allow_origin.is_empty());

	let cli = Cli::try_parse_from([
		"octomind",
		"developer:general",
		"--host",
		"0.0.0.0",
		"-p",
		"9000",
		"--sandbox",
		"--allow-origin",
		"http://localhost:3000",
		"--allow-origin",
		"https://panel.example",
	])
	.expect("full flag set parses");
	assert_eq!(cli.args.tag.as_deref(), Some("developer:general"));
	assert_eq!(cli.args.host, "0.0.0.0");
	assert_eq!(cli.args.port, 9000);
	assert!(cli.args.sandbox);
	assert_eq!(
		cli.args.allow_origin,
		vec![
			"http://localhost:3000".to_string(),
			"https://panel.example".to_string()
		]
	);
}

#[tokio::test]
#[serial]
async fn execute_rejects_an_unknown_tap_tag() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = tempfile::tempdir().expect("sandbox dir");
	std::env::set_var(DATA_DIR_KEY, dir.path());
	let default_tap = octomind::agent::taps::Tap {
		name: octomind::agent::taps::DEFAULT_TAP.to_string(),
		local_path: None,
	};
	std::fs::create_dir_all(default_tap.local_dir().expect("default tap dir"))
		.expect("create empty default tap");

	let config = template_config();
	let args = ServerArgs {
		tag: Some("no-such-category:missing-variant".to_string()),
		host: "127.0.0.1".to_string(),
		port: 0,
		sandbox: false,
		allow_origin: Vec::new(),
	};
	let err = execute(&args, &config)
		.await
		.expect_err("unknown tap tag fails resolution");
	assert!(
		err.to_string().contains("Failed to fetch agent manifest"),
		"{err}"
	);
}

#[tokio::test]
#[serial]
async fn execute_starts_the_websocket_server_on_an_ephemeral_port() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let dir = std::env::temp_dir().join(format!("octomind-server-{}", std::process::id()));
	std::fs::create_dir_all(&dir).expect("sandbox dir");
	std::env::set_var(DATA_DIR_KEY, &dir);

	let config = template_config();
	// A plain role skips tap manifest fetching; port 0 binds an ephemeral
	// port so parallel runs never collide.
	let args = ServerArgs {
		tag: Some("assistant".to_string()),
		host: "127.0.0.1".to_string(),
		port: 0,
		sandbox: false,
		allow_origin: vec!["http://localhost:3000".to_string()],
	};
	execute(&args, &config)
		.await
		.expect("server starts and returns");
}
