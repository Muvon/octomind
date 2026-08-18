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

//! End-to-end guardrail-hook tests: a real `.agents/guardrails.toml` in a
//! tempdir workdir, real hook scripts spawned as processes. The contract
//! under test: a hook surfaces an inbox message ONLY when its script exits
//! non-zero with non-empty stdout, and `on = "error"` hooks stay silent for
//! successful tool results.

use super::*;
use std::os::unix::fs::PermissionsExt;

fn write_script(dir: &std::path::Path, rel: &str, body: &str) {
	let path = dir.join(rel);
	std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
	std::fs::write(&path, body).expect("write script");
	std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn hook_workdir() -> tempfile::TempDir {
	let tmp = tempfile::tempdir().expect("tempdir");
	std::fs::create_dir_all(tmp.path().join(".agents")).expect(".agents");
	std::fs::write(
		tmp.path().join(".agents/guardrails.toml"),
		concat!(
			"[[hook]]\n",
			"script = \"hooks/notify.sh\"\n",
			"\n",
			"[[hook]]\n",
			"on = \"error\"\n",
			"script = \"hooks/on_error.sh\"\n",
		),
	)
	.expect("write guardrails.toml");
	write_script(
		tmp.path(),
		"hooks/notify.sh",
		"#!/bin/sh\necho \"HOOK-FIRED: inspect the result\"\nexit 1\n",
	);
	write_script(
		tmp.path(),
		"hooks/on_error.sh",
		"#!/bin/sh\necho \"ERROR-HOOK-FIRED\"\nexit 1\n",
	);
	tmp
}

#[tokio::test]
async fn test_hook_fires_into_inbox_and_error_hook_stays_silent() {
	let sid = "__hooks_test_fire".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let tmp = hook_workdir();
		crate::session::context::set_session_workdir(&sid, tmp.path().to_path_buf());
		crate::session::guardrails::init_for_session();
		crate::session::inbox::init_inbox_for_session();

		let config: crate::config::Config =
			toml::from_str(include_str!("../../config-templates/default.toml"))
				.expect("parse default config template");

		let call = crate::mcp::McpToolCall {
			tool_name: "shell".to_string(),
			parameters: serde_json::json!({"cmd": "make build"}),
			tool_id: "t-hook".to_string(),
		};
		let result = crate::mcp::McpToolResult::success(
			"shell".to_string(),
			"t-hook".to_string(),
			"build finished".to_string(),
		);
		crate::session::hooks::run_hooks(&sid, &config, &[call], &[result], &[false]).await;

		let msg = crate::session::inbox::try_pop_inbox_message()
			.expect("firing hook must push an inbox message");
		assert!(
			msg.content.contains("HOOK-FIRED"),
			"hook stdout missing: {}",
			msg.content
		);
		// The on=error hook must NOT have fired for a successful result
		assert!(
			crate::session::inbox::try_pop_inbox_message().is_none(),
			"error hook fired on success"
		);

		crate::session::inbox::clear_inbox_for_session(&sid);
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
async fn test_blocked_and_hookless_calls_push_nothing() {
	let sid = "__hooks_test_blocked".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let tmp = hook_workdir();
		crate::session::context::set_session_workdir(&sid, tmp.path().to_path_buf());
		crate::session::guardrails::init_for_session();
		crate::session::inbox::init_inbox_for_session();

		let config: crate::config::Config =
			toml::from_str(include_str!("../../config-templates/default.toml"))
				.expect("parse default config template");

		let call = crate::mcp::McpToolCall {
			tool_name: "shell".to_string(),
			parameters: serde_json::json!({"cmd": "make build"}),
			tool_id: "t-hook".to_string(),
		};
		let result = crate::mcp::McpToolResult::success(
			"shell".to_string(),
			"t-hook".to_string(),
			"ok".to_string(),
		);
		// The call is marked blocked: hooks must not run for it
		crate::session::hooks::run_hooks(&sid, &config, &[call], &[result], &[true]).await;
		assert!(
			crate::session::inbox::try_pop_inbox_message().is_none(),
			"hooks ran for a blocked call"
		);

		crate::session::inbox::clear_inbox_for_session(&sid);
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}
