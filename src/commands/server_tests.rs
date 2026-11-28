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
use crate::commands::test_support::EnvGuard;
use clap::Parser;
use octomind::Config;
use serial_test::serial;

fn template_config() -> Config {
	let mut config: Config =
		toml::from_str(&crate::commands::template_with_assistant_role::template_with_assistant_role())
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
		"developer",
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
	assert_eq!(cli.args.tag.as_deref(), Some("developer"));
	assert_eq!(cli.args.host, "0.0.0.0");
	assert_eq!(cli.args.port, 9000);
	assert!(cli.args.sandbox);
	assert_eq!(
		cli.args.allow_origin,
		vec!["http://localhost:3000".to_string(), "https://panel.example".to_string()]
	);
}

#[tokio::test]
#[serial]
async fn execute_fails_for_a_variant_tag_that_no_tap_has() {
	let _env = EnvGuard::new(&["OCTOMIND_DATA_DIR"]);
	let data = tempfile::tempdir().expect("data dir");
	// An existing built-in tap directory keeps the lookup offline.
	std::fs::create_dir_all(data.path().join("taps/muvon/octomind-tap")).expect("tap dir");
	std::env::set_var("OCTOMIND_DATA_DIR", data.path());
	let config = template_config();
	octomind::agent::taps::set_built_in_tap(&config.registry);
	let args = ServerArgs {
		tag: Some("developer:general".to_string()),
		host: "127.0.0.1".to_string(),
		port: 0,
		sandbox: false,
		allow_origin: Vec::new(),
	};
	let err = execute(&args, &config)
		.await
		.expect_err("a variant of no namespace fails resolution");
	assert!(err.to_string().contains("not in any installed tap"), "{err}");
}

#[tokio::test]
async fn websocket_server_on_ephemeral_port_keeps_serving() {
	let server = octomind::websocket::WebSocketServer::new(
		"127.0.0.1",
		0,
		template_config(),
		"assistant".to_string(),
		vec!["http://localhost:3000".to_string()],
	)
	.expect("server configuration is valid");

	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(100), server.start())
			.await
			.is_err(),
		"a started server should keep serving until its owner cancels it"
	);
}
