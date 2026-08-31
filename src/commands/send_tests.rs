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

//! Tests for `octomind send`: argument parsing, response decoding, and the
//! Unix-socket delivery path against a real local `UnixListener` bound at the
//! same path `session_socket_path` derives.

use super::*;
use clap::Parser;
#[cfg(unix)]
use serial_test::serial;

#[cfg(unix)]
const DATA_DIR_KEY: &str = "OCTOMIND_DATA_DIR";

/// Snapshot env vars and restore them on drop.
#[cfg(unix)]
struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

#[cfg(unix)]
impl EnvGuard {
	fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}

#[cfg(unix)]
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

#[derive(clap::Parser)]
struct Cli {
	#[command(flatten)]
	args: SendArgs,
}

#[test]
fn send_args_parse_name_short_flag_and_message() {
	let cli = Cli::try_parse_from(["octomind", "-n", "work", "hello there"])
		.expect("name + message parse");
	assert_eq!(cli.args.name, "work");
	assert_eq!(cli.args.message.as_deref(), Some("hello there"));

	let cli = Cli::try_parse_from(["octomind", "--name", "work"]).expect("message optional");
	assert_eq!(cli.args.name, "work");
	assert!(cli.args.message.is_none());

	assert!(
		Cli::try_parse_from(["octomind", "hello"]).is_err(),
		"name is required"
	);
}

#[tokio::test]
async fn read_response_accepts_ok_and_rejects_anything_else() {
	let mut ok: &[u8] = b"ok";
	read_response(&mut ok, "sess").await.expect("ok is success");

	let mut padded: &[u8] = b"  ok\n\n";
	read_response(&mut padded, "sess")
		.await
		.expect("whitespace around ok still ok");

	let mut busy: &[u8] = b"busy";
	let err = read_response(&mut busy, "sess")
		.await
		.expect_err("non-ok surfaces the reply");
	assert!(
		err.to_string().contains("session 'sess' returned: busy"),
		"{err}"
	);
}

#[tokio::test]
async fn execute_rejects_an_empty_message_argument() {
	let err = execute(&SendArgs {
		name: "work".into(),
		message: Some("   ".into()),
	})
	.await
	.expect_err("blank message bails");
	assert!(
		err.to_string().contains("message must not be empty"),
		"{err}"
	);
}

#[tokio::test]
async fn execute_rejects_a_missing_message_without_stdin_data() {
	// Whether stdin is a terminal (bails before reading) or an empty pipe
	// (reads EOF, still empty), the outcome is the same refusal.
	let err = execute(&SendArgs {
		name: "work".into(),
		message: None,
	})
	.await
	.expect_err("no message bails");
	assert!(
		err.to_string().contains("message must not be empty"),
		"{err}"
	);
}

/// Bind a one-shot Unix-socket peer at the session's inject path and echo
/// `reply` back. Returns the received message via the join handle.
#[cfg(unix)]
async fn socket_peer(
	name: &str,
	reply: &'static str,
) -> (std::path::PathBuf, tokio::task::JoinHandle<String>) {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	let path = octomind::directories::session_socket_path(name).expect("socket path");
	let listener = tokio::net::UnixListener::bind(&path).expect("bind socket");
	let task = tokio::spawn(async move {
		let (mut sock, _) = listener.accept().await.expect("accept");
		let mut received = String::new();
		sock.read_to_string(&mut received)
			.await
			.expect("read message");
		sock.write_all(reply.as_bytes()).await.expect("write reply");
		let _ = sock.shutdown().await;
		received
	});
	(path, task)
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn execute_delivers_the_message_over_the_socket() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let name = format!("octo-send-ok-{}", std::process::id());
	let (path, peer) = socket_peer(&name, "ok\n").await;

	execute(&SendArgs {
		name: name.clone(),
		message: Some("hello socket".into()),
	})
	.await
	.expect("send succeeds when the session answers ok");

	assert_eq!(peer.await.expect("peer finished"), "hello socket");
	let _ = std::fs::remove_file(&path);
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn execute_surfaces_a_non_ok_session_reply() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let name = format!("octo-send-busy-{}", std::process::id());
	let (path, peer) = socket_peer(&name, "busy\n").await;

	let err = execute(&SendArgs {
		name: name.clone(),
		message: Some("any".into()),
	})
	.await
	.expect_err("non-ok reply fails the send");
	assert!(err.to_string().contains("returned: busy"), "{err}");

	let _ = peer.await;
	let _ = std::fs::remove_file(&path);
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn execute_fails_when_no_session_socket_exists() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let name = format!("octo-send-none-{}", std::process::id());

	let err = execute(&SendArgs {
		name: name.clone(),
		message: Some("hello".into()),
	})
	.await
	.expect_err("missing socket bails");
	assert!(
		err.to_string()
			.contains(&format!("no running session named '{name}'")),
		"{err}"
	);
}
