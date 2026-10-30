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

//! Dispatch-layer tests for the `/schedule` session command: subcommand
//! routing, key=value parsing through the raw input string, and the full
//! add/list/edit/remove lifecycle against the real schedule tool inside a
//! throwaway session scope. Tokenizer unit tests live in the inline mod tests.

use super::*;

/// Mimic how `process_command` splits raw input into params.
fn params_of(input: &str) -> Vec<&str> {
	input.split_whitespace().skip(1).collect()
}

/// Run one `/schedule <input>` invocation and return its JSON payload.
async fn run(input: &str) -> serde_json::Value {
	let params = params_of(input);
	let result = handle_schedule(input, &params)
		.await
		.unwrap_or_else(|e| panic!("schedule {input:?} errored: {e}"));
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	let CommandOutput::Schedule { data } = *output else {
		panic!("expected Schedule output");
	};
	data
}

/// Extract the `[id]` from an add-command success message.
fn extract_id(text: &str) -> String {
	let start = text.find('[').expect("id bracket in add response") + 1;
	let end = text[start..].find(']').expect("closing bracket") + start;
	text[start..end].to_string()
}

#[tokio::test]
async fn test_bare_command_lists_empty_schedule() {
	crate::session::context::with_session_id("sched-cmd-bare".to_string(), async {
		let data = run("/schedule").await;
		assert_eq!(data["subcommand"], "list");
		assert_eq!(data["is_error"], false);
		assert_eq!(data["message"], "No scheduled entries.");
	})
	.await;
}

#[tokio::test]
async fn test_unknown_subcommand() {
	let data = run("/schedule frobnicate").await;
	assert_eq!(data["subcommand"], "error");
	let message = data["message"].as_str().expect("message");
	assert!(
		message.contains("unknown subcommand 'frobnicate'"),
		"message: {message}"
	);
}

#[tokio::test]
async fn test_help_subcommand() {
	let data = run("/schedule help").await;
	assert_eq!(data["subcommand"], "help");
}

#[tokio::test]
async fn test_remove_without_id_shows_usage() {
	let data = run("/schedule remove").await;
	assert_eq!(data["subcommand"], "error");
	assert_eq!(data["message"], "usage: /schedule remove <id>");
}

#[tokio::test]
async fn test_add_rejects_bare_token() {
	let data = run("/schedule add bareword").await;
	assert_eq!(data["subcommand"], "error");
	let message = data["message"].as_str().expect("message");
	assert!(message.contains("parse error:"), "message: {message}");
	assert!(message.contains("expected key=value"), "message: {message}");
}

#[tokio::test]
async fn test_add_rejects_unterminated_quote() {
	let data = run("/schedule add when=\"in 5m").await;
	assert_eq!(data["subcommand"], "error");
	let message = data["message"].as_str().expect("message");
	assert!(message.contains("unterminated quote"), "message: {message}");
}

#[tokio::test]
async fn test_add_requires_message() {
	crate::session::context::with_session_id("sched-cmd-no-msg".to_string(), async {
		let data = run("/schedule add when=\"in 5m\"").await;
		assert_eq!(data["subcommand"], "add");
		assert_eq!(data["is_error"], true);
		let message = data["message"].as_str().expect("message");
		assert!(message.contains("message"), "message: {message}");
	})
	.await;
}

#[tokio::test]
async fn test_user_command_cannot_add_or_replace_messages_with_tool_calls() {
	let id = format!("sched-cmd-message-only-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(id.clone(), async {
		let mut config: crate::config::Config = toml::from_str(include_str!(concat!(
			env!("CARGO_MANIFEST_DIR"),
			"/config-templates/default.toml"
		)))
		.expect("default config");
		config.mcp.servers = vec![crate::config::McpServerConfig::builtin(
			"orchestration",
			30,
			vec!["schedule".to_string()],
		)];
		crate::mcp::tool_map::isolate_session_tool_map(&id);
		crate::mcp::tool_map::initialize_tool_map(&config).await.unwrap();
		assert!(crate::mcp::tool_map::get_server_for_tool("schedule").is_some());
		let payload = r#"tool_call='{"name":"schedule","arguments":{"command":"list"}}'"#;
		let added = run(&format!("/schedule add {payload}")).await;
		assert_eq!(added["is_error"], true, "data: {added}");
		assert_eq!(run("/schedule list").await["message"], "No scheduled entries.");

		let message = run("/schedule add message='message only'").await;
		assert_eq!(message["is_error"], false);
		let entry = extract_id(message["message"].as_str().unwrap());
		let edited = run(&format!("/schedule edit {entry} {payload}")).await;
		assert_eq!(edited["is_error"], true, "data: {edited}");
		let listing = run("/schedule list").await;
		assert!(listing["message"].as_str().unwrap().contains("message only"));
	})
	.await;
	crate::session::context::cleanup_session(&id);
}

#[tokio::test]
async fn test_add_rejects_invalid_when() {
	crate::session::context::with_session_id("sched-cmd-bad-when".to_string(), async {
		let data = run("/schedule add when=\"not-a-time\" message=\"hi\"").await;
		assert_eq!(data["is_error"], true, "data: {data}");
	})
	.await;
}

#[tokio::test]
async fn test_implicit_add_without_subcommand() {
	crate::session::context::with_session_id("sched-cmd-implicit".to_string(), async {
		let data = run("/schedule when=\"in 5m\" message=\"implicit add works\"").await;
		assert_eq!(data["subcommand"], "add");
		assert_eq!(data["is_error"], false, "data: {data}");
	})
	.await;
}

#[tokio::test]
async fn test_add_list_edit_remove_lifecycle() {
	crate::session::context::with_session_id("sched-cmd-lifecycle".to_string(), async {
		// add with quoted multi-word values
		let added = run("/schedule add when=\"in 5m\" message=\"check the build\" description=\"ci poll\"").await;
		assert_eq!(added["is_error"], false, "add failed: {added}");
		let message = added["message"].as_str().expect("message");
		assert!(message.contains("✅ Scheduled ["), "message: {message}");
		let id = extract_id(message);

		// list shows the entry
		let listed = run("/schedule list").await;
		assert_eq!(listed["is_error"], false);
		let listing = listed["message"].as_str().expect("message");
		assert!(listing.contains(&id), "listing: {listing}");
		assert!(listing.contains("ci poll"), "listing: {listing}");

		// edit with positional id: message + repeat interval change
		let edited = run(&format!(
			"/schedule edit {id} message=\"check the deploy\" every=\"10m\""
		))
		.await;
		assert_eq!(edited["is_error"], false, "edit failed: {edited}");
		let after_edit = run("/schedule").await;
		let listing = after_edit["message"].as_str().expect("message");
		assert!(listing.contains("10m"), "listing after edit: {listing}");
		assert!(listing.contains("check the deploy"), "listing after edit: {listing}");

		// remove, then the schedule is empty again
		let removed = run(&format!("/schedule remove {id}")).await;
		assert_eq!(removed["is_error"], false, "remove failed: {removed}");
		let emptied = run("/schedule list").await;
		assert_eq!(emptied["message"], "No scheduled entries.");
	})
	.await;
}

#[tokio::test]
async fn test_edit_nonexistent_id_errors() {
	crate::session::context::with_session_id("sched-cmd-edit-missing".to_string(), async {
		let data = run("/schedule edit no-such-id when=\"in 1m\"").await;
		assert_eq!(data["is_error"], true, "data: {data}");
	})
	.await;
}

#[tokio::test]
async fn test_remove_nonexistent_id_errors() {
	crate::session::context::with_session_id("sched-cmd-rm-missing".to_string(), async {
		let data = run("/schedule remove no-such-id").await;
		assert_eq!(data["is_error"], true, "data: {data}");
	})
	.await;
}
