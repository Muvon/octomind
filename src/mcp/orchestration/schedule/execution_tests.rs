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

//! Scheduled calls exercise the real Layer executor without creating a parent model turn.

use super::*;
use crate::config::McpServerConfig;
use crate::mcp::orchestration::schedule::storage::ScheduledToolCall;
use crate::mcp::orchestration::schedule::{flush_due_to_inbox, has_pending_schedules, is_session_idle};
use crate::session::context::{cleanup_session, get_schedule_storage, init_session_services, with_session_id};
use crate::session::inbox::try_pop_inbox_message;
use serde_json::{json, Value};
use std::future::Future;
use std::time::Duration;

const WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn test_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml")).expect("parse default configuration");
	config.supervisor.enabled = false;
	config.supervisor.authorizer.enabled = false;
	config.capabilities.clear();
	config.mcp.servers = vec![McpServerConfig::builtin(
		"orchestration",
		30,
		vec!["schedule".to_string(), "tap".to_string()],
	)];
	config
}

struct TestSession {
	id: SessionId,
	dir: tempfile::TempDir,
	tool: String,
}

impl TestSession {
	fn new() -> Self {
		let suffix = uuid::Uuid::new_v4().simple().to_string();
		Self {
			id: format!("schedule-execution-{suffix}"),
			dir: tempfile::tempdir().expect("temporary project"),
			tool: format!("schedule_probe_{suffix}"),
		}
	}

	async fn run(&self, config: &Config, future: impl Future<Output = ()>) {
		with_session_id(self.id.clone(), async {
			crate::mcp::set_session_working_directory(self.dir.path().to_path_buf());
			crate::mcp::tool_map::isolate_session_tool_map(&self.id);
			// In particular, this loads project guardrails and initializes authorizer,
			// inbox, scheduled executions, tap registry and background job manager.
			init_session_services("assistant");
			crate::mcp::tool_map::initialize_tool_map(config)
				.await
				.expect("initialize isolated real tool map");
			future.await;
		})
		.await;
	}

	fn entry(&self, name: &str, arguments: Value) -> ScheduleEntry {
		ScheduleEntry::new(
			format!("scheduled execution fixture {}", self.tool),
			ScheduleAction::ToolCall {
				tool_call: ScheduledToolCall {
					name: name.to_string(),
					arguments,
				},
				workdir: self.dir.path().to_path_buf(),
			},
			chrono::Local::now() - chrono::Duration::seconds(1),
			None,
		)
	}

	fn enqueue(&self, entry: ScheduleEntry) {
		get_schedule_storage(&self.id).lock().unwrap().add(entry);
	}
}

impl Drop for TestSession {
	fn drop(&mut self) {
		cleanup_session(&self.id);
		crate::config::runtime_overlay::clear_capability_extras(&self.id);
	}
}

async fn wait_until(reason: &str, mut ready: impl FnMut() -> bool) {
	tokio::time::timeout(WAIT_TIMEOUT, async {
		while !ready() {
			tokio::time::sleep(POLL_INTERVAL).await;
		}
	})
	.await
	.unwrap_or_else(|_| panic!("timed out waiting for {reason}"));
}

async fn completed(id: &str) -> InboxMessage {
	wait_until("scheduled execution to finish", || !has_running()).await;
	let message = try_pop_inbox_message().expect("execution must publish its result before becoming idle");
	assert!(matches!(&message.source, InboxSource::Schedule { id: actual } if actual == id));
	assert!(
		try_pop_inbox_message().is_none(),
		"exactly one result, not a model-dispatch prompt"
	);
	message
}

fn assert_status(message: &InboxMessage, tool: &str, id: &str, status: &str) {
	assert!(
		message
			.content
			.starts_with(&format!("[Scheduled tool '{tool}' {status} ({id})]\n\n")),
		"{}",
		message.content
	);
}

#[tokio::test]
async fn due_builtin_schedule_list_executes_and_delivers_listing_not_a_prompt() {
	let session = TestSession::new();
	let config = test_config();
	session
		.run(&config, async {
			let entry = session.entry("schedule", json!({"command": "list"}));
			let id = entry.id.clone();
			session.enqueue(entry);
			flush_due_to_inbox(&config);
			assert!(has_pending_schedules());
			let message = completed(&id).await;
			assert_status(&message, "schedule", &id, "completed");
			assert!(
				message.content.ends_with("No scheduled entries."),
				"{}",
				message.content
			);
			assert!(!has_pending_schedules());
			assert!(is_session_idle());
			flush_due_to_inbox(&config);
			assert!(try_pop_inbox_message().is_none());

			{
				let registry = executions().lock().unwrap();
				let previous = &registry[&session.id][&id];
				assert!(!previous.running);
				assert!(previous.tap_run.is_none());
			}
			let next = session.entry("schedule", json!({"command": "list"}));
			let next_id = next.id.clone();
			dispatch(next, &config);
			{
				let registry = executions().lock().unwrap();
				let jobs = &registry[&session.id];
				assert!(
					!jobs.contains_key(&id),
					"next dispatch must prune the completed one-shot"
				);
				assert!(jobs[&next_id].running);
				assert_eq!(jobs.len(), 1);
			}
			assert_status(&completed(&next_id).await, "schedule", &next_id, "completed");
		})
		.await;
}

#[tokio::test]
async fn due_builtin_tap_invalid_run_delivers_error_without_registering_a_job() {
	let session = TestSession::new();
	let config = test_config();
	session
		.run(&config, async {
			let entry = session.entry("tap", json!({"action": "run"}));
			let id = entry.id.clone();
			session.enqueue(entry);
			flush_due_to_inbox(&config);
			let message = completed(&id).await;
			assert_status(&message, "tap", &id, "failed");
			assert!(
				message.content.contains("Missing required parameter 'prompt'"),
				"{}",
				message.content
			);
			assert!(!crate::session::tap_runs::has_running_jobs());
			assert!(crate::session::tap_runs::list_jobs().is_empty());
			assert!(executions().lock().unwrap()[&session.id][&id].tap_run.is_none());
			assert!(is_session_idle());
		})
		.await;
}

#[tokio::test]
async fn disabled_builtin_at_firing_delivers_unavailable_error_without_mutating_store() {
	let session = TestSession::new();
	let config = test_config();
	session
		.run(&config, async {
			let entry = session.entry(
				"schedule",
				json!({
					"command": "add", "when": "in 1h", "message": "must never be added"
				}),
			);
			let id = entry.id.clone();
			session.enqueue(entry);
			assert!(crate::mcp::get_available_functions(&config)
				.await
				.iter()
				.any(|f| f.name == "schedule"));
			let server = config.mcp.servers[0].clone();
			crate::session::context::register_dynamic_server_for_session(&session.id, server);
			assert!(crate::session::context::enable_dynamic_server_for_session(
				&session.id,
				"orchestration",
				Vec::new()
			));
			assert!(crate::session::context::disable_dynamic_server_for_session(
				&session.id,
				"orchestration"
			));
			flush_due_to_inbox(&config);
			let message = completed(&id).await;
			assert_status(&message, "schedule", &id, "failed");
			assert!(message
				.content
				.contains("tool 'schedule' is no longer available in this session"));
			assert!(get_schedule_storage(&session.id).lock().unwrap().is_empty());
			assert!(crate::session::guardrails::get_call_log(&session.id).is_empty());
		})
		.await;
}

#[tokio::test]
async fn due_tick_skips_stored_running_tap_then_dispatches_after_tap_finishes() {
	use crate::session::tap_runs::{register_job, TapJob, TapJobStatus};
	use std::sync::{Arc, RwLock};

	let session = TestSession::new();
	let config = test_config();
	session
		.run(&config, async {
			let mut entry = session.entry("schedule", json!({"command": "list"}));
			entry.interval_secs = Some(3600);
			let id = entry.id.clone();
			session.enqueue(entry);
			flush_due_to_inbox(&config);
			assert_status(&completed(&id).await, "schedule", &id, "completed");

			// Only the child process is replaced: use the real session-owned TapJob
			// registry and the same stored run id as a successful tap(run) result.
			let tap_id = format!("tap-{}", uuid::Uuid::new_v4());
			let status = Arc::new(RwLock::new(TapJobStatus::Running));
			let (cancel_tx, _cancel_rx) = watch::channel(false);
			register_job(TapJob {
				id: tap_id.clone(),
				role: "developer:general".to_string(),
				workdir: session.dir.path().display().to_string(),
				started_at: std::time::SystemTime::now(),
				status: Arc::clone(&status),
				error: None,
				cancel_tx,
				live: Arc::new(RwLock::new(Default::default())),
			});
			executions()
				.lock()
				.unwrap()
				.get_mut(&session.id)
				.unwrap()
				.get_mut(&id)
				.unwrap()
				.tap_run = Some(tap_id);
			force_due(&session.id, &id);
			flush_due_to_inbox(&config);
			assert!(!has_running(), "a busy tap must not launch another scheduled execution");
			assert!(try_pop_inbox_message().is_none());
			assert_eq!(crate::session::guardrails::get_call_log(&session.id).len(), 1);
			assert!(!is_session_idle());

			*status.write().unwrap() = TapJobStatus::Done;
			force_due(&session.id, &id);
			flush_due_to_inbox(&config);
			assert!(has_running());
			assert_status(&completed(&id).await, "schedule", &id, "completed");
			assert_eq!(crate::session::guardrails::get_call_log(&session.id).len(), 2);
			assert!(is_session_idle());
		})
		.await;
}

fn force_due(session: &SessionId, id: &str) {
	assert!(get_schedule_storage(session).lock().unwrap().edit(
		id,
		None,
		None,
		Some(chrono::Local::now() - chrono::Duration::seconds(1)),
		None,
	));
}

#[tokio::test]
async fn execute_with_prefired_cancellation_preserves_call_identity_and_returns_error() {
	let session = TestSession::new();
	let config = test_config();
	session
		.run(&config, async {
			let call = McpToolCall {
				tool_name: "schedule".to_string(),
				tool_id: "cancelled-scheduled-call".to_string(),
				parameters: json!({"command": "add", "when": "in 1h", "message": "must not be added"}),
			};
			let (_cancel, rx) = watch::channel(true);
			let (result, tap_run) = execute(&call, &config, rx).await;
			assert!(tap_run.is_none());
			assert!(result.is_error());
			assert_eq!(result.tool_name, call.tool_name);
			assert_eq!(result.tool_id, call.tool_id);
			assert!(result.extract_content().to_lowercase().contains("cancel"));
			assert!(get_schedule_storage(&session.id).lock().unwrap().is_empty());
			assert!(try_pop_inbox_message().is_none());
		})
		.await;
}

// These integration fixtures execute shebang scripts and assert Unix process
// cancellation. Windows cannot execute these files directly; builtin tests above
// remain cross-platform, as in the existing local-tool execution tests.
#[cfg(unix)]
mod unix {
	use super::*;
	use crate::mcp::orchestration::schedule::storage::TriggerMode;
	use crate::mcp::orchestration::schedule::{execute_schedule_tool, flush_idle_to_inbox};
	use crate::session::inbox::get_inbox_notify;
	use std::os::unix::fs::PermissionsExt;
	use std::path::Path;

	fn write_script(root: &Path, relative: &str, body: &str) {
		let path = root.join(relative);
		std::fs::create_dir_all(path.parent().expect("script parent")).expect("script directory");
		std::fs::write(&path, body).expect("write executable script");
		std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod +x fixture");
	}

	fn install_probe(session: &TestSession) {
		write_script(
			session.dir.path(),
			&format!(".agents/tools/{}", session.tool),
			r#"#!/bin/sh
# @description Record a scheduled call and optionally wait for explicit release.
# @param token string required Payload to echo.
# @param wait boolean Wait for release before completing.
cat > params.json
printf '%s\n' "$OCTOMIND_PARAM_TOKEN" >> calls
printf '%s\n' "$OCTOMIND_WORKDIR" > workdir
printf '%s\n' "$$" > pid
: > started
if [ "$OCTOMIND_PARAM_WAIT" = true ]; then
	i=0
	while [ ! -f release ]; do
		i=$((i + 1))
		[ "$i" -lt 1000 ] || exit 23
		sleep 0.01
	done
fi
: > finished
printf 'RESULT:%s\n' "$OCTOMIND_PARAM_TOKEN"
"#,
		);
	}

	fn install_hook(session: &TestSession, guards: &str) {
		write_script(
			session.dir.path(),
			"hooks/record",
			"#!/bin/sh\ncat > hook.json\n: > hook-ran\n",
		);
		std::fs::write(
			session.dir.path().join(".agents/guardrails.toml"),
			format!("{guards}\n[[hook]]\nscript = 'hooks/record'\n"),
		)
		.expect("project guardrails");
	}

	fn read(root: &Path, relative: &str) -> String {
		std::fs::read_to_string(root.join(relative)).expect("tool side effect")
	}

	fn release(session: &TestSession) {
		std::fs::write(session.dir.path().join("release"), "").expect("release running script");
	}

	async fn started(session: &TestSession) {
		wait_until("real script to start", || session.dir.path().join("started").exists()).await;
	}

	#[tokio::test]
	async fn due_local_call_executes_once_with_arguments_and_post_hook_without_parent_turn() {
		let session = TestSession::new();
		install_probe(&session);
		install_hook(&session, "");
		let config = test_config();
		session
			.run(&config, async {
				let arguments = json!({"token": "雪 🦀\nkeep whitespace ", "wait": false});
				let entry = session.entry(&session.tool, arguments.clone());
				let id = entry.id.clone();
				session.enqueue(entry);
				flush_due_to_inbox(&config);
				flush_due_to_inbox(&config);
				let message = completed(&id).await;
				assert_status(&message, &session.tool, &id, "completed");
				assert!(message.content.ends_with("RESULT:雪 🦀\nkeep whitespace \n"));
				assert_eq!(read(session.dir.path(), "calls"), "雪 🦀\nkeep whitespace \n");
				assert_eq!(
					serde_json::from_str::<Value>(&read(session.dir.path(), "params.json")).unwrap(),
					arguments
				);
				let hook: Value = serde_json::from_str(&read(session.dir.path(), "hook.json")).unwrap();
				assert_eq!(hook["tool"], session.tool);
				assert_eq!(hook["params"], arguments);
				assert_eq!(hook["success"], true);
				assert_eq!(hook["result"], "RESULT:雪 🦀\nkeep whitespace \n");
				assert!(hook["tool_id"]
					.as_str()
					.unwrap()
					.starts_with(&format!("schedule-{id}-")));
				assert_eq!(crate::session::guardrails::get_call_log(&session.id).len(), 1);
				assert!(!has_pending_schedules());
				flush_due_to_inbox(&config);
				assert!(try_pop_inbox_message().is_none());
			})
			.await;
	}

	#[tokio::test]
	async fn idle_local_call_executes_once_and_keeps_session_busy_until_result_delivery() {
		let session = TestSession::new();
		install_probe(&session);
		let config = test_config();
		session
			.run(&config, async {
				let mut entry = session.entry(&session.tool, json!({"token": "idle", "wait": true}));
				entry.trigger_mode = TriggerMode::Idle;
				let id = entry.id.clone();
				session.enqueue(entry);
				assert!(is_session_idle());
				flush_idle_to_inbox(&config);
				started(&session).await;
				assert!(get_schedule_storage(&session.id).lock().unwrap().is_empty());
				assert!(has_running());
				assert!(has_pending_schedules());
				assert!(!is_session_idle());
				assert!(try_pop_inbox_message().is_none());
				flush_idle_to_inbox(&config);
				release(&session);
				let message = completed(&id).await;
				assert_status(&message, &session.tool, &id, "completed");
				assert!(message.content.ends_with("RESULT:idle\n"));
				assert_eq!(read(session.dir.path(), "calls"), "idle\n");
				assert!(!has_pending_schedules());
				assert!(is_session_idle());
				flush_idle_to_inbox(&config);
				assert!(try_pop_inbox_message().is_none());
			})
			.await;
	}

	#[tokio::test]
	async fn due_local_call_keeps_one_shot_pending_and_session_nonidle_while_running() {
		let session = TestSession::new();
		install_probe(&session);
		let config = test_config();
		session
			.run(&config, async {
				let entry = session.entry(&session.tool, json!({"token": "due", "wait": true}));
				let id = entry.id.clone();
				session.enqueue(entry);
				flush_due_to_inbox(&config);
				started(&session).await;
				assert!(get_schedule_storage(&session.id).lock().unwrap().is_empty());
				assert!(has_running());
				assert!(has_pending_schedules());
				assert!(!is_session_idle());
				assert!(try_pop_inbox_message().is_none());
				release(&session);
				assert_status(&completed(&id).await, &session.tool, &id, "completed");
				assert_eq!(read(session.dir.path(), "calls"), "due\n");
				assert!(!has_pending_schedules());
				assert!(is_session_idle());
			})
			.await;
	}

	#[tokio::test]
	async fn idle_call_waits_for_running_due_call_then_executes_once() {
		let session = TestSession::new();
		install_probe(&session);
		let config = test_config();
		session
			.run(&config, async {
				let due = session.entry(&session.tool, json!({"token": "due first", "wait": true}));
				let due_id = due.id.clone();
				session.enqueue(due);
				flush_due_to_inbox(&config);
				started(&session).await;
				let mut idle = session.entry(&session.tool, json!({"token": "idle second"}));
				idle.trigger_mode = TriggerMode::Idle;
				let idle_id = idle.id.clone();
				session.enqueue(idle);
				flush_idle_to_inbox(&config);
				assert_eq!(get_schedule_storage(&session.id).lock().unwrap().entries().len(), 1);
				assert_eq!(read(session.dir.path(), "calls"), "due first\n");
				assert_eq!(crate::session::guardrails::get_call_log(&session.id).len(), 1);
				assert!(try_pop_inbox_message().is_none());
				release(&session);
				assert_status(&completed(&due_id).await, &session.tool, &due_id, "completed");
				flush_idle_to_inbox(&config);
				assert_status(&completed(&idle_id).await, &session.tool, &idle_id, "completed");
				assert_eq!(read(session.dir.path(), "calls"), "due first\nidle second\n");
				assert!(!has_pending_schedules());
				assert!(is_session_idle());
				flush_due_to_inbox(&config);
				flush_idle_to_inbox(&config);
				assert!(try_pop_inbox_message().is_none());
			})
			.await;
	}

	#[tokio::test]
	async fn repeating_due_ticks_skip_running_execution_then_allow_next_run() {
		let session = TestSession::new();
		install_probe(&session);
		let config = test_config();
		session
			.run(&config, async {
				let mut entry = session.entry(&session.tool, json!({"token": "repeat", "wait": true}));
				entry.interval_secs = Some(3600);
				let id = entry.id.clone();
				session.enqueue(entry);
				flush_due_to_inbox(&config);
				started(&session).await;
				let original_cancel = executions().lock().unwrap()[&session.id][&id].cancel.subscribe();
				for _ in 0..3 {
					force_due(&session.id, &id);
					flush_due_to_inbox(&config);
					assert!(
						original_cancel.has_changed().is_ok(),
						"busy tick replaced the original execution"
					);
					assert!(has_running());
					assert!(try_pop_inbox_message().is_none());
				}
				release(&session);
				assert_status(&completed(&id).await, &session.tool, &id, "completed");
				assert_eq!(read(session.dir.path(), "calls"), "repeat\n");
				assert_eq!(crate::session::guardrails::get_call_log(&session.id).len(), 1);
				force_due(&session.id, &id);
				flush_due_to_inbox(&config);
				assert_status(&completed(&id).await, &session.tool, &id, "completed");
				assert_eq!(read(session.dir.path(), "calls"), "repeat\nrepeat\n");
				assert_eq!(crate::session::guardrails::get_call_log(&session.id).len(), 2);
			})
			.await;
	}

	#[tokio::test]
	async fn removing_recurrence_does_not_cancel_already_launched_local_call() {
		let session = TestSession::new();
		install_probe(&session);
		let config = test_config();
		session
			.run(&config, async {
				let mut entry = session.entry(&session.tool, json!({"token": "survives removal", "wait": true}));
				entry.interval_secs = Some(3600);
				let id = entry.id.clone();
				session.enqueue(entry);
				flush_due_to_inbox(&config);
				started(&session).await;
				let removal = execute_schedule_tool(&McpToolCall {
					tool_name: "schedule".to_string(),
					tool_id: "remove-recurrence".to_string(),
					parameters: json!({"command": "remove", "id": id}),
				})
				.await
				.expect("remove is a tool result");
				assert!(!removal.is_error(), "{}", removal.extract_content());
				assert!(get_schedule_storage(&session.id).lock().unwrap().is_empty());
				assert!(has_pending_schedules());
				assert!(!*executions().lock().unwrap()[&session.id][&id].cancel.borrow());
				release(&session);
				let message = completed(&id).await;
				assert_status(&message, &session.tool, &id, "completed");
				assert!(message.content.ends_with("RESULT:survives removal\n"));
				assert!(session.dir.path().join("finished").exists());
				assert_eq!(read(session.dir.path(), "calls"), "survives removal\n");
				assert!(!has_pending_schedules());
				flush_due_to_inbox(&config);
				assert!(try_pop_inbox_message().is_none());
			})
			.await;
	}

	#[tokio::test]
	async fn removed_local_tool_at_firing_delivers_error_without_script_or_post_hook() {
		let session = TestSession::new();
		install_probe(&session);
		install_hook(&session, "");
		let config = test_config();
		session
			.run(&config, async {
				let entry = session.entry(&session.tool, json!({"token": "removed"}));
				let id = entry.id.clone();
				session.enqueue(entry);
				assert!(crate::mcp::get_available_functions(&config)
					.await
					.iter()
					.any(|f| f.name == session.tool));
				std::fs::remove_file(session.dir.path().join(".agents/tools").join(&session.tool)).unwrap();
				flush_due_to_inbox(&config);
				let message = completed(&id).await;
				assert_status(&message, &session.tool, &id, "failed");
				assert!(message.content.contains(&format!(
					"tool '{}' is no longer available in this session",
					session.tool
				)));
				assert!(!session.dir.path().join("calls").exists());
				assert!(!session.dir.path().join("hook-ran").exists());
				assert!(crate::session::guardrails::get_call_log(&session.id).is_empty());
			})
			.await;
	}

	#[tokio::test]
	async fn guardrail_denies_local_call_without_side_effect_post_hook_or_history_credit() {
		let session = TestSession::new();
		install_probe(&session);
		install_hook(
			&session,
			&format!(
				"[[guard]]\nmatch = '{}(token=^blocked$)'\nmessage = 'scheduled writes forbidden'\n",
				session.id
			),
		);
		crate::config::runtime_overlay::set_capability_extras(
			&session.id,
			HashMap::from([("local".to_string(), vec![session.tool.clone()])]),
		);
		let config = test_config();
		session
			.run(&config, async {
				assert_eq!(
					crate::session::guardrails::get_rules(&session.id).unwrap().guards.len(),
					1
				);
				let entry = session.entry(&session.tool, json!({"token": "blocked"}));
				let id = entry.id.clone();
				dispatch(entry, &config);
				let message = completed(&id).await;
				assert_status(&message, &session.tool, &id, "failed");
				assert!(message.content.contains("[guardrail]"), "{}", message.content);
				assert!(message.content.contains("scheduled writes forbidden"));
				assert!(!session.dir.path().join("calls").exists());
				assert!(!session.dir.path().join("hook-ran").exists());
				assert!(crate::session::guardrails::get_call_log(&session.id).is_empty());

				// Positive control: the same loaded hook and guard allow another payload.
				let allowed = session.entry(&session.tool, json!({"token": "allowed"}));
				let allowed_id = allowed.id.clone();
				dispatch(allowed, &config);
				assert_status(&completed(&allowed_id).await, &session.tool, &allowed_id, "completed");
				assert_eq!(read(session.dir.path(), "calls"), "allowed\n");
				assert!(session.dir.path().join("hook-ran").exists());
				assert_eq!(crate::session::guardrails::get_call_log(&session.id).len(), 1);
			})
			.await;
	}

	#[tokio::test]
	async fn local_result_is_capped_after_post_hook_receives_full_output() {
		let session = TestSession::new();
		install_probe(&session);
		install_hook(&session, "");
		let mut config = test_config();
		config.mcp_response_tokens_threshold = 64;
		session
			.run(&config, async {
				let token = "unbounded_payload_0123456789\n".repeat(1000);
				let raw = format!("RESULT:{token}\n");
				let entry = session.entry(&session.tool, json!({"token": token}));
				let id = entry.id.clone();
				dispatch(entry, &config);
				let message = completed(&id).await;
				assert_status(&message, &session.tool, &id, "completed");
				assert!(message
					.content
					.contains(crate::utils::truncation::TRUNCATION_NOTICE_TAG));
				assert!(message.content.len() < raw.len() / 2);
				let hook: Value = serde_json::from_str(&read(session.dir.path(), "hook.json")).unwrap();
				assert_eq!(hook["result"], raw);
				assert_eq!(hook["success"], true);
			})
			.await;
	}

	#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
	async fn captured_cwd_reaches_spawned_tool_without_moving_parent_active_directory_or_anchor() {
		let session = TestSession::new();
		install_probe(&session);
		install_hook(&session, "");
		let parent = tempfile::tempdir().expect("new parent cwd");
		let process_cwd = std::env::current_dir().unwrap();
		let config = test_config();
		session
			.run(&config, async {
				let added = execute_schedule_tool(&McpToolCall {
					tool_name: "schedule".to_string(),
					tool_id: "capture-cwd".to_string(),
					parameters: json!({"command": "add", "when": "in 1h",
					"tool_call": {"name": session.tool, "arguments": {"token": "captured", "wait": true}}}),
				})
				.await
				.expect("schedule captures cwd through its public handler");
				assert!(!added.is_error(), "{}", added.extract_content());
				let entry = get_schedule_storage(&session.id).lock().unwrap().entries()[0].clone();
				let id = entry.id.clone();
				assert!(
					matches!(&entry.action, ScheduleAction::ToolCall { workdir, .. } if workdir == session.dir.path())
				);
				crate::mcp::set_thread_working_directory(parent.path().to_path_buf());
				force_due(&session.id, &id);
				flush_due_to_inbox(&config);
				started(&session).await;
				assert_eq!(crate::mcp::get_thread_working_directory(), parent.path());
				assert_eq!(crate::mcp::get_thread_original_working_directory(), session.dir.path());
				assert_eq!(
					crate::session::context::get_current_workdir(&session.id).as_deref(),
					Some(parent.path())
				);
				assert_eq!(
					read(session.dir.path(), "workdir").trim_end(),
					session.dir.path().to_str().unwrap()
				);
				assert!(!parent.path().join("started").exists());
				release(&session);
				assert_status(&completed(&id).await, &session.tool, &id, "completed");
				assert_eq!(read(session.dir.path(), "calls"), "captured\n");
				assert!(
					session.dir.path().join("hook-ran").exists(),
					"hook must run in captured cwd A"
				);
				assert!(
					!parent.path().join("hook-ran").exists(),
					"hook must not run in parent cwd B"
				);
				assert!(!parent.path().join("hook.json").exists());
				let hook: Value = serde_json::from_str(&read(session.dir.path(), "hook.json")).unwrap();
				assert_eq!(hook["params"]["token"], "captured");
				assert_eq!(crate::mcp::get_thread_working_directory(), parent.path());
				assert_eq!(crate::mcp::get_thread_original_working_directory(), session.dir.path());
				assert_eq!(std::env::current_dir().unwrap(), process_cwd);
			})
			.await;
	}

	#[tokio::test]
	async fn equal_schedule_ids_in_two_sessions_isolate_running_state_permissions_results_and_cwd() {
		let first = TestSession::new();
		let second = TestSession::new();
		install_probe(&first);
		install_probe(&second);
		let config = test_config();
		first
			.run(&config, async {
				let mut entry = first.entry(&first.tool, json!({"token": "first", "wait": true}));
				entry.id = "same-schedule-id".to_string();
				dispatch(entry, &config);
				started(&first).await;
				second
					.run(&config, async {
						assert!(!has_running());
						assert!(!has_pending_schedules());
						assert!(is_session_idle());
						let mut entry = second.entry(&second.tool, json!({"token": "second"}));
						entry.id = "same-schedule-id".to_string();
						dispatch(entry, &config);
						let message = completed("same-schedule-id").await;
						assert_status(&message, &second.tool, "same-schedule-id", "completed");
						assert!(message.content.ends_with("RESULT:second\n"));
						let forbidden = second.entry(&first.tool, json!({"token": "cross-session"}));
						let forbidden_id = forbidden.id.clone();
						dispatch(forbidden, &config);
						let error = completed(&forbidden_id).await;
						assert_status(&error, &first.tool, &forbidden_id, "failed");
						assert!(error.content.contains("no longer available in this session"));
						assert_eq!(read(second.dir.path(), "calls"), "second\n");
						assert_eq!(crate::mcp::get_thread_working_directory(), second.dir.path());
					})
					.await;
				assert!(has_running());
				assert!(has_pending_schedules());
				assert!(!is_session_idle());
				assert!(try_pop_inbox_message().is_none());
				assert_eq!(crate::mcp::get_thread_working_directory(), first.dir.path());
				release(&first);
				let message = completed("same-schedule-id").await;
				assert_status(&message, &first.tool, "same-schedule-id", "completed");
				assert!(message.content.ends_with("RESULT:first\n"));
				assert_eq!(read(first.dir.path(), "calls"), "first\n");
			})
			.await;
	}

	#[tokio::test]
	async fn cleanup_cancels_running_process_and_discards_delivery_to_destroyed_inbox() {
		let session = TestSession::new();
		install_probe(&session);
		let config = test_config();
		session
			.run(&config, async {
				let entry = session.entry(&session.tool, json!({"token": "cancel me", "wait": true}));
				let id = entry.id.clone();
				dispatch(entry, &config);
				started(&session).await;
				let pid: libc::pid_t = read(session.dir.path(), "pid").trim().parse().unwrap();
				let (cancel, task) = {
					let mut registry = executions().lock().unwrap();
					let job = registry.get_mut(&session.id).unwrap().get_mut(&id).unwrap();
					// Retain a join handle only to await real teardown, not to replace execution.
					(
						job.cancel.subscribe(),
						std::mem::replace(&mut job.task, tokio::spawn(async {})),
					)
				};
				assert!(get_inbox_notify().is_some());
				cleanup_session(&session.id);
				assert!(*cancel.borrow(), "central cleanup must signal execution cancellation");
				assert!(!executions().lock().unwrap().contains_key(&session.id));
				tokio::time::timeout(WAIT_TIMEOUT, task)
					.await
					.expect("execution teardown")
					.expect("executor task");
				wait_until("cancelled tool process to be reaped", || {
					// The fixture records its own live PID before the start barrier.
					(unsafe { libc::kill(pid, 0) == -1 })
						&& std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
				})
				.await;
				release(&session);
				assert!(!session.dir.path().join("finished").exists());
				assert_eq!(read(session.dir.path(), "calls"), "cancel me\n");
				assert!(
					get_inbox_notify().is_none(),
					"late result must not recreate a gone inbox"
				);
				assert!(try_pop_inbox_message().is_none());
				assert!(crate::session::inbox::peek_inbox_preview(&session.id).is_none());
				assert!(!has_running());
			})
			.await;
	}
}
