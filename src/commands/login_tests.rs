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

//! Tests for `octomind login`: argument parsing plus the full device flow run
//! against a local HTTP stub speaking the kisscore `[err, data]` envelope.
//! Env-touching tests are `#[serial]` because env vars are process-global.

use super::*;
use clap::Parser;
use octomind::account::{self, Session};
use serial_test::serial;
use std::collections::VecDeque;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
	let dir = std::env::temp_dir().join(format!("octomind-login-{tag}-{}", std::process::id()));
	std::fs::create_dir_all(&dir).expect("create sandbox data dir");
	dir
}

fn env_ok(data: serde_json::Value) -> String {
	serde_json::json!([null, data]).to_string()
}

fn env_err(code: &str) -> String {
	serde_json::json!([code, null]).to_string()
}

/// One-shot-per-connection HTTP stub serving raw bodies in order.
async fn spawn_api(bodies: Vec<String>) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind stub");
	let addr = listener.local_addr().expect("stub addr");
	let queue = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::from(bodies)));

	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			let queue = queue.clone();
			tokio::spawn(async move {
				let mut buf = Vec::new();
				let mut tmp = [0u8; 8192];
				let header_end = loop {
					let n = sock.read(&mut tmp).await.unwrap_or(0);
					if n == 0 {
						return;
					}
					buf.extend_from_slice(&tmp[..n]);
					if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
						break pos + 4;
					}
				};
				let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
				let content_length: usize = headers
					.lines()
					.find_map(|l| l.strip_prefix("content-length:"))
					.and_then(|v| v.trim().parse().ok())
					.unwrap_or(0);
				while buf.len() < header_end + content_length {
					let n = sock.read(&mut tmp).await.unwrap_or(0);
					if n == 0 {
						break;
					}
					buf.extend_from_slice(&tmp[..n]);
				}
				let body = queue
					.lock()
					.expect("stub queue")
					.pop_front()
					.unwrap_or_else(|| "[null, null]".to_string());
				let response = format!(
					"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
					body.len(),
					body
				);
				let _ = sock.write_all(response.as_bytes()).await;
				let _ = sock.shutdown().await;
			});
		}
	});

	format!("http://{addr}")
}

#[derive(clap::Parser)]
struct Cli {
	#[command(flatten)]
	args: LoginArgs,
}

#[test]
fn login_args_parse_flags_and_default_to_off() {
	let cli = Cli::try_parse_from(["octomind"]).expect("bare login parses");
	assert!(!cli.args.force);
	assert!(!cli.args.no_browser);

	let cli = Cli::try_parse_from(["octomind", "--force", "--no-browser"]).expect("flags parse");
	assert!(cli.args.force);
	assert!(cli.args.no_browser);
}

#[tokio::test]
#[serial]
async fn execute_reports_an_existing_session_without_minting_new_credentials() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY, account::API_URL_ENV, account::HUB_KEY_ENV]);
	let dir = sandbox("already");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(account::HUB_KEY_ENV);
	let url = spawn_api(vec![env_ok(
		serde_json::json!({"email": "dev@example.com", "plan": "pro"}),
	)])
	.await;
	std::env::set_var(account::API_URL_ENV, &url);
	account::save_session(&Session {
		jwt: "j1".into(),
		refresh_token: "r1".into(),
		api_url: url.clone(),
	})
	.expect("seed session");

	execute(&LoginArgs {
		force: false,
		no_browser: false,
	})
	.await
	.expect("already-signed-in is a clean exit");

	let config_dir = octomind::directories::get_config_dir().expect("config dir");
	assert!(
		!config_dir.join(".env").exists(),
		"early return must not touch the hub key"
	);
}

#[tokio::test]
#[serial]
async fn execute_completes_the_device_flow_and_stores_credentials() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY, account::API_URL_ENV, account::HUB_KEY_ENV]);
	let dir = sandbox("full-flow");
	std::env::set_var(DATA_DIR_KEY, &dir);
	std::env::remove_var(account::HUB_KEY_ENV);
	let url = spawn_api(vec![
		env_ok(serde_json::json!({
			"device_code": "dc-1",
			"user_code": "AB12-CD34",
			"verification_url": "https://octomind.run/app/login/cli?code=AB12-CD34",
			"verification_url_complete": "https://octomind.run/app/login/cli?code=AB12-CD34&complete",
			"interval": 1
		})),
		env_ok(serde_json::json!({
			"api_key": "hk-1",
			"jwt": "jwt-1",
			"refresh_token": "r-1",
			"key_name": "octomind-cli-x1"
		})),
		env_ok(serde_json::json!({"email": "dev@example.com", "plan": "pro"})),
	])
	.await;
	std::env::set_var(account::API_URL_ENV, &url);

	execute(&LoginArgs {
		force: true,
		no_browser: true,
	})
	.await
	.expect("device flow completes against the stub");

	let config_dir = octomind::directories::get_config_dir().expect("config dir");
	let env_body = std::fs::read_to_string(config_dir.join(".env")).expect(".env written");
	assert!(env_body.contains("OCTOHUB_API_KEY=hk-1"), "{env_body}");
	let s = account::session().expect("panel session stored");
	assert_eq!(s.jwt, "jwt-1");
	assert_eq!(s.refresh_token, "r-1");
}

#[tokio::test]
#[serial]
async fn execute_surfaces_a_failed_login_start() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY, account::API_URL_ENV]);
	let dir = sandbox("start-fail");
	std::env::set_var(DATA_DIR_KEY, &dir);
	let url = spawn_api(vec![env_err("boom")]).await;
	std::env::set_var(account::API_URL_ENV, &url);

	let err = execute(&LoginArgs {
		force: true,
		no_browser: true,
	})
	.await
	.expect_err("start failure propagates");
	assert!(
		err.to_string().contains("could not start login: boom"),
		"{err}"
	);
}
