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
use crate::config::HookConfig;
use crate::session::inbox;
use std::io::Write;
use tempfile::TempDir;

/// A temporary script file whose write fd is closed before returning.
/// On Linux, executing a file with an open write fd causes ETXTBSY (error 26).
struct TempScript {
	path: PathBuf,
	_dir: TempDir, // keeps the directory (and file) alive
}

impl TempScript {
	fn path(&self) -> &std::path::Path {
		&self.path
	}
}

fn make_script(code: &str) -> TempScript {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("script.sh");
	{
		// Write and close the fd inside this block so the file is not open when executed.
		let mut f = std::fs::File::create(&path).unwrap();
		writeln!(f, "#!/bin/bash").unwrap();
		writeln!(f, "{}", code).unwrap();
	}
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
	}
	TempScript { path, _dir: dir }
}

#[test]
fn validate_hook_rejects_missing_script() {
	let hook = HookConfig {
		name: "test".into(),
		bind: "127.0.0.1:0".into(),
		script: "/nonexistent/script.sh".into(),
		timeout: 30,
	};
	let err = validate_hook(&hook).unwrap_err();
	assert!(err.to_string().contains("does not exist"), "{}", err);
}

#[test]
fn validate_hook_rejects_bad_bind() {
	let script = make_script("echo ok");
	let hook = HookConfig {
		name: "test".into(),
		bind: "not-an-address".into(),
		script: script.path().to_string_lossy().into(),
		timeout: 30,
	};
	let err = validate_hook(&hook).unwrap_err();
	assert!(err.to_string().contains("invalid bind address"), "{}", err);
}

#[test]
fn validate_hook_accepts_valid_config() {
	let script = make_script("echo ok");
	let hook = HookConfig {
		name: "test".into(),
		bind: "127.0.0.1:0".into(),
		script: script.path().to_string_lossy().into(),
		timeout: 30,
	};
	let (addr, path) = validate_hook(&hook).unwrap();
	assert_eq!(addr.port(), 0);
	assert_eq!(path, script.path());
}

#[cfg(unix)]
#[tokio::test]
async fn webhook_listener_processes_post() {
	// Script that echoes stdin with a prefix
	let script = make_script(r#"read -r payload; echo "got: $payload""#);

	let hook = HookConfig {
		name: "smoke".into(),
		bind: "127.0.0.1:0".into(),
		script: script.path().to_string_lossy().into(),
		timeout: 10,
	};

	let (_, script_path) = validate_hook(&hook).unwrap();

	// Initialize inbox for our test session
	let session_name = "webhook-test-session";
	crate::session::context::with_session_id(session_name.to_string(), async move {
		inbox::init_inbox_for_session();

		// Start the listener (bind to port 0 = OS picks a free port)
		let guard = start_webhook_listener(
			session_name,
			&hook,
			"127.0.0.1:0".parse().unwrap(),
			script_path,
		)
		.await
		.unwrap();

		// We need the actual bound port. The listener binds inside start_webhook_listener.
		// Since we bind to port 0, we need a different approach — let's bind ourselves.
		drop(guard);

		// Instead, bind explicitly to get the port
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();

		let hook_name = hook.name.clone();
		let script_for_task = hook.script.clone();
		let timeout = hook.timeout;

		let handle = tokio::spawn(async move {
			let _ = run_listener(
				listener,
				session_name,
				&hook_name,
				&PathBuf::from(&script_for_task),
				timeout,
			)
			.await;
		});

		// Give the listener a moment to start accepting
		tokio::time::sleep(Duration::from_millis(50)).await;

		// Send a POST request
		let client = reqwest::Client::new();
		let resp = client
			.post(format!("http://{}", addr))
			.body("hello-webhook")
			.send()
			.await
			.unwrap();

		assert_eq!(resp.status(), 200);
		let body = resp.text().await.unwrap();
		assert_eq!(body.trim(), "ok");

		// Check inbox got the message
		let msg = inbox::try_pop_inbox_message();
		assert!(msg.is_some(), "Expected a message in the inbox");
		let msg = msg.unwrap();
		assert!(
			msg.content.contains("got: hello-webhook"),
			"Unexpected content: {}",
			msg.content
		);
		match &msg.source {
			InboxSource::Webhook { hook } => assert_eq!(hook, "smoke"),
			other => panic!("Expected Webhook source, got {:?}", other),
		}

		// GET should be rejected
		let resp = client.get(format!("http://{}", addr)).send().await.unwrap();
		assert_eq!(resp.status(), 405);

		handle.abort();
	})
	.await;
}

#[cfg(unix)]
#[tokio::test]
async fn webhook_listener_handles_script_failure() {
	let script = make_script("exit 1");

	let hook = HookConfig {
		name: "fail-hook".into(),
		bind: "127.0.0.1:0".into(),
		script: script.path().to_string_lossy().into(),
		timeout: 10,
	};

	let session_name = "webhook-fail-test";
	crate::session::context::with_session_id(session_name.to_string(), async move {
		inbox::init_inbox_for_session();

		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();

		let script_path = PathBuf::from(&hook.script);
		let handle = tokio::spawn(async move {
			let _ = run_listener(listener, session_name, "fail-hook", &script_path, 10).await;
		});

		tokio::time::sleep(Duration::from_millis(50)).await;

		let client = reqwest::Client::new();
		let resp = client
			.post(format!("http://{}", addr))
			.body("test")
			.send()
			.await
			.unwrap();

		assert_eq!(resp.status(), 500);

		// Inbox should be empty (script failed)
		assert!(inbox::try_pop_inbox_message().is_none());

		handle.abort();
	})
	.await;
}
