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

//! Direct render tests for the CLI display arms the dispatch tests cannot
//! reach (they need live data from network-backed commands: usage, login,
//! share, agents, learning, skills, mcp health…). Each renderer is fed a
//! representative payload and must render without panicking — the padding
//! and truncation code here is exactly where malformed width math bites.

use super::*;
use crate::session::chat::session::commands::UsageWindow;
use serde_json::json;

fn test_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

#[test]
fn test_fmt_tokens_scales() {
	assert_eq!(fmt_tokens(850), "850");
	assert_eq!(fmt_tokens(12_400), "12.4k");
	assert_eq!(fmt_tokens(3_000_000), "3.0M");
}

#[test]
fn test_mcp_health_display_known_and_unknown() {
	for health in ["healthy", "dead", "degraded", "unknown", "bogus"] {
		assert!(!mcp_health_display(health).is_empty());
	}
}

#[test]
fn test_bar_handles_zero_and_over_limit() {
	// Zero limit must not divide by zero; over-limit must not over-index
	assert!(!money_bar(1.0, 0.0).is_empty());
	assert!(!money_bar(15.0, 10.0).is_empty());
	assert!(!gb_bar(0.0, 100.0).is_empty());
	assert!(!gb_bar(120.0, 100.0).is_empty());
}

#[test]
fn test_render_done_all_flags() {
	display_done(&CommandOutput::Done {
		done: true,
		memorized: Some(true),
		summarized: Some(true),
		saved: Some(true),
	});
	display_done(&CommandOutput::Done {
		done: false,
		memorized: None,
		summarized: None,
		saved: None,
	});
}

#[test]
fn test_render_run_list_and_execute() {
	let config = test_config();
	display_run(
		&CommandOutput::Run {
			command_executed: "list".to_string(),
			data: json!({"action": "list", "commands": ["estimate", "review"]}),
		},
		&config,
		"assistant",
	);
	display_run(
		&CommandOutput::Run {
			command_executed: "list".to_string(),
			data: json!({"action": "list", "commands": []}),
		},
		&config,
		"assistant",
	);
	display_run(
		&CommandOutput::Run {
			command_executed: "estimate".to_string(),
			data: json!({"action": "execute", "success": true, "result": "# Done\nAll good."}),
		},
		&config,
		"assistant",
	);
	display_run(
		&CommandOutput::Run {
			command_executed: "bogus".to_string(),
			data: json!({
				"action": "execute",
				"success": false,
				"error": "unknown command",
				"available_commands": ["estimate"]
			}),
		},
		&config,
		"assistant",
	);
}

#[test]
fn test_render_mcp_health_variants() {
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "health".to_string(),
		data: json!({
			"subcommand": "health",
			"monitor_running": true,
			"servers": [
				{"name": "developer", "health": "healthy", "last_checked_secs_ago": 12},
				{"name": "flaky", "health": "dead", "restart_count": 3, "consecutive_failures": 2}
			]
		}),
	});
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "health".to_string(),
		data: json!({"subcommand": "health", "message": "MCP disabled for this role"}),
	});
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "health".to_string(),
		data: json!({"subcommand": "health", "monitor_running": false, "error": "monitor not reachable"}),
	});
}

#[test]
fn test_render_skill_variants() {
	display_skill(&CommandOutput::Skill {
		data: json!({
			"subcommand": "list",
			"total": 2,
			"active_count": 1,
			"page": 1,
			"total_pages": 2,
			"pattern": "",
			"skills": [
				{
					"name": "code-review",
					"description": "Reviews code with a very long description that should be truncated because it goes far past the eighty character display ceiling for a single line",
					"active": true,
					"capabilities": ["review"],
					"domains": ["rust"],
					"scripts": ["lint.sh"]
				},
				{"name": "bare", "description": "", "active": false}
			]
		}),
	});
	display_skill(&CommandOutput::Skill {
		data: json!({"subcommand": "list", "total": 0, "active_count": 0, "skills": []}),
	});
	display_skill(&CommandOutput::Skill {
		data: json!({"subcommand": "use", "name": "code-review"}),
	});
	display_skill(&CommandOutput::Skill {
		data: json!({"subcommand": "forget", "name": "code-review"}),
	});
	display_skill(&CommandOutput::Skill {
		data: json!({"subcommand": "error", "message": "no such skill"}),
	});
}

#[test]
fn test_render_learning_variants() {
	display_learning(&CommandOutput::Learning {
		data: json!({
			"subcommand": "list",
			"role": "assistant",
			"project": "octomind",
			"total": 2,
			"page": 1,
			"total_pages": 2,
			"lessons": [
				{
					"index": 1,
					"content": "Run the tests on the box, never locally — the local toolchain drifts and the CI image is the only truth for this repository's builds",
					"importance": 0.9,
					"confidence": "high",
					"scope": "global",
					"tags": ["testing"],
					"created": "2026-08-18T10:00:00Z"
				},
				{"index": 2, "content": "short one", "importance": 0.2}
			]
		}),
	});
	display_learning(&CommandOutput::Learning {
		data: json!({"subcommand": "list", "role": "a", "project": "p", "total": 0, "lessons": []}),
	});
	display_learning(&CommandOutput::Learning {
		data: json!({"subcommand": "delete", "index": 1, "content_preview": "Run the tests"}),
	});
	display_learning(&CommandOutput::Learning {
		data: json!({"subcommand": "clear", "deleted": 3, "errors": ["locked.md"]}),
	});
	display_learning(&CommandOutput::Learning {
		data: json!({"subcommand": "clear", "deleted": 0}),
	});
	display_learning(&CommandOutput::Learning {
		data: json!({"subcommand": "error", "message": "store unreadable"}),
	});
}

#[test]
fn test_render_usage_signed_in_and_out() {
	display_usage(&CommandOutput::Usage {
		signed_in: true,
		account: Some("dev@example.com".to_string()),
		windows: vec![
			UsageWindow {
				label: "5h".to_string(),
				spent_usd: 2.5,
				reserved_usd: Some(0.75),
				cap_usd: 10.0,
				resets_at: "2026-08-18T12:00:00Z".to_string(),
			},
			UsageWindow {
				label: "week".to_string(),
				spent_usd: 40.0,
				reserved_usd: None,
				cap_usd: 50.0,
				resets_at: "2026-08-24T00:00:00Z".to_string(),
			},
		],
		balance_usd: 12.34,
		storage_gb: 1.5,
		storage_quota_gb: 10.0,
		network_used_gb: 120.0,
		network_included_gb: 100.0,
	});
	display_usage(&CommandOutput::Usage {
		signed_in: false,
		account: None,
		windows: Vec::new(),
		balance_usd: 0.0,
		storage_gb: 0.0,
		storage_quota_gb: 0.0,
		network_used_gb: 0.0,
		network_included_gb: 0.0,
	});
}

#[test]
fn test_render_login_share_analyze() {
	display_login(&CommandOutput::Login {
		already_signed_in: true,
		account: Some("dev@example.com".to_string()),
		verification_url: None,
		user_code: None,
	});
	display_login(&CommandOutput::Login {
		already_signed_in: false,
		account: None,
		verification_url: Some("https://octomind.run/verify".to_string()),
		user_code: Some("ABCD-1234".to_string()),
	});
	display_share(&CommandOutput::Share {
		id: "sh-1".to_string(),
		url: "https://octomind.run/s/sh-1".to_string(),
	});
	display_share(&CommandOutput::Share {
		id: "sh-2".to_string(),
		url: "http://localhost:8080/s/sh-2".to_string(),
	});
	display_analyze(&CommandOutput::Analyze {
		url: "http://127.0.0.1:4321/?token=tok".to_string(),
		port: 4321,
		token: "tok".to_string(),
	});
}

#[test]
fn test_render_agents_list_and_detail() {
	display_agents(&CommandOutput::Agents {
		running: vec![json!({
			"id": "run-1",
			"role": "developer",
			"elapsed_secs": 95,
			"tokens_input": 12_400,
			"tokens_output": 850,
			"cost": 0.0421,
			"last_action": "editing src/main.rs"
		})],
		finished: vec![json!({
			"id": "run-0",
			"role": "researcher",
			"status": "done",
			"elapsed_secs": 30,
			"finished_secs_ago": 600,
			"tokens_input": 3_000_000,
			"tokens_output": 1_000,
			"cost": 1.25
		})],
		detail: None,
		total: 2,
	});
	display_agents(&CommandOutput::Agents {
		running: Vec::new(),
		finished: Vec::new(),
		detail: None,
		total: 0,
	});
	display_agents(&CommandOutput::Agents {
		running: Vec::new(),
		finished: Vec::new(),
		detail: Some(json!({
			"id": "run-1",
			"role": "developer",
			"status": "failed",
			"elapsed_secs": 120,
			"workdir": "/tmp/w",
			"model": "ollama:fake-model",
			"tokens_input": 500,
			"tokens_output": 200,
			"cost": 0.01,
			"last_action": "cargo build failed"
		})),
		total: 1,
	});
}

#[test]
fn test_render_report_and_list_tables() {
	let config = test_config();
	display_report(
		&CommandOutput::Report {
			entries: vec![json!({
				"user_request": "a multi\nline request\twith tabs that is quite long and will be truncated in the table cell",
				"cost": "$0.0421",
				"tool_calls": 7,
				"task_time": "1.2s",
				"ai_time": "800ms",
				"processing_time": "400ms"
			})],
			totals: json!({
				"cost": "$0.0421",
				"tool_calls": 7,
				"task_time": "1.2s",
				"ai_time": "800ms",
				"processing_time": "400ms"
			}),
		},
		&config,
	);
	display_report(
		&CommandOutput::Report {
			entries: Vec::new(),
			totals: json!({}),
		},
		&config,
	);

	display_list(
		&CommandOutput::List {
			sessions: vec![
				json!({
					"name": "a-session-with-an-extremely-long-name-that-truncates",
					"title": "investigating the coverage gap in the websocket layer",
					"created": "2026-08-18 10:00",
					"model": "openrouter:anthropic/claude-sonnet-5",
					"tokens": 123_456,
					"cost": 1.2345,
					"is_current": true
				}),
				json!({"name": "bare", "model": "plain-model", "tokens": 10, "cost": 0.0}),
			],
			total_sessions: 2,
			page: 1,
			total_pages: 1,
			plain_text: None,
		},
		&config,
	);
	display_list(
		&CommandOutput::List {
			sessions: Vec::new(),
			total_sessions: 0,
			page: 1,
			total_pages: 1,
			plain_text: None,
		},
		&config,
	);
}

#[test]
fn test_render_schedule_and_monitor_arms() {
	display_schedule(&CommandOutput::Schedule {
		data: json!({"subcommand": "help"}),
	});
	display_schedule(&CommandOutput::Schedule {
		data: json!({"subcommand": "error", "message": "bad when= expression"}),
	});
	display_monitor(&CommandOutput::Monitor {
		data: json!({"subcommand": "error", "message": "no such monitor"}),
	});
	display_monitor(&CommandOutput::Monitor {
		data: json!({"subcommand": "list", "is_error": false, "message": "No running monitors"}),
	});
}

#[test]
fn test_render_mcp_list_full_validate_payloads() {
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "list".to_string(),
		data: json!({"subcommand": "list", "servers": {
			"developer": ["shell", "text_editor"],
			"orchestration": ["schedule", "monitor"]
		}}),
	});
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "list".to_string(),
		data: json!({"subcommand": "list", "servers": {}}),
	});
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "full".to_string(),
		data: json!({
			"subcommand": "full",
			"servers": [
				{"name": "developer", "health": "healthy", "connection_type": "builtin",
				 "tools": ["shell", "text_editor"]},
				{"name": "flaky", "health": "dead", "connection_type": "stdio",
				 "restart_count": 2, "consecutive_failures": 1}
			],
			"tools": {"developer": [{
				"name": "shell",
				"description": "Run a command",
				"parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}}
			}]}
		}),
	});
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "validate".to_string(),
		data: json!({
			"subcommand": "validate",
			"all_valid": false,
			"tools": [
				{"name": "good_tool", "valid": true},
				{"name": "bad_tool", "valid": false, "issues": ["missing description"]}
			]
		}),
	});
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "validate".to_string(),
		data: json!({"subcommand": "validate", "all_valid": true, "tools": [{"name": "good_tool", "valid": true}]}),
	});
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "info".to_string(),
		data: json!({
			"subcommand": "info",
			"servers": [
				{"name": "developer", "health": "healthy", "connection_type": "builtin",
				 "tools": [{"name": "shell", "description": "Run a command"}]},
				{"name": "flaky", "health": "dead", "connection_type": "stdio",
				 "restart_count": 3, "consecutive_failures": 2,
				 "tools": [{"name": "probe", "description": "Probe things"}]}
			]
		}),
	});
	// Unknown subcommand falls through to the generic message arm
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "other".to_string(),
		data: json!({"subcommand": "??", "message": "unknown mcp subcommand"}),
	});
}

#[test]
fn test_render_plan_variants() {
	display_plan(&CommandOutput::Plan {
		has_plan: true,
		plan: Some(json!({"title": "ship it"})),
		display: Some("Task 1: build\nTask 2: test".to_string()),
		knowledge: vec![
			"first knowledge entry\nwith a continuation line".to_string(),
			"second entry".to_string(),
		],
	});
	display_plan(&CommandOutput::Plan {
		has_plan: false,
		plan: None,
		display: None,
		knowledge: Vec::new(),
	});
}

#[test]
fn test_render_image_video_variants() {
	display_image(&CommandOutput::Image {
		image_attached: true,
		path: Some("shot.png".to_string()),
		error: None,
	});
	display_image(&CommandOutput::Image {
		image_attached: false,
		path: None,
		error: Some("no such file".to_string()),
	});
	display_video(&CommandOutput::Video {
		video_attached: true,
		path: Some("clip.mp4".to_string()),
		error: None,
	});
	display_video(&CommandOutput::Video {
		video_attached: false,
		path: None,
		error: Some("unsupported codec".to_string()),
	});
}

#[test]
fn test_render_info_full_and_minimal() {
	let mut cstats = crate::session::CompressionStats::default();
	cstats.conversation_compressions = 2;
	cstats.total_messages_removed = 14;
	cstats.total_tokens_saved = 9_000;
	cstats.input_tokens = 1_200;

	display_info(&CommandOutput::Info {
		session_name: "info-render".to_string(),
		model: "ollama:fake-model".to_string(),
		role: "assistant".to_string(),
		tokens_input: 120_000,
		tokens_output: 8_500,
		tokens_used: 128_500,
		tokens_cached: 90_000,
		tokens_cache_write: 4_000,
		tokens_reasoning: 1_500,
		total_cost: 1.2345,
		cache_savings: 0.42,
		tokens_per_second: 55.5,
		avg_tokens_per_compression: 4_500.0,
		avg_tokens_per_tool: 800.0,
		avg_tokens_per_response: 1_200.0,
		avg_input_tokens: 20_000.0,
		compression_stats: Some(cstats),
		cache_markers_system: 1,
		cache_markers_tool: 2,
		cache_markers_content: 2,
		cache_non_cached_tokens: 12_000,
		agents_stats: Some(json!({
			"total": 3, "running": 1, "done": 1, "failed": 1,
			"tokens_input": 50_000, "tokens_output": 4_000,
			"tokens_cached": 30_000, "total_cost": 0.55
		})),
		supervisor_stats: Some(json!({
			"calls": 12, "recall_calls": 2, "gate_calls": 4, "resolve_calls": 1,
			"distill_calls": 1, "condense_calls": 2, "delegate_calls": 1,
			"fidelity_calls": 1, "delegate_runs": 1, "delegate_blocks": 0,
			"condensed_results": 3, "condense_saved_tokens": 4_000,
			"input_tokens": 30_000, "output_tokens": 2_000,
			"tokens_per_second": 80.0, "cost": 0.12,
			"gate_runs": 4, "gate_pass": 3, "gate_fail": 1,
			"steers": 1, "steer_signals": {"drift": 1}
		})),
	});

	// Minimal: fresh session, nothing optional present
	display_info(&CommandOutput::Info {
		session_name: "empty".to_string(),
		model: "m".to_string(),
		role: "assistant".to_string(),
		tokens_input: 0,
		tokens_output: 0,
		tokens_used: 0,
		tokens_cached: 0,
		tokens_cache_write: 0,
		tokens_reasoning: 0,
		total_cost: 0.0,
		cache_savings: 0.0,
		tokens_per_second: 0.0,
		avg_tokens_per_compression: 0.0,
		avg_tokens_per_tool: 0.0,
		avg_tokens_per_response: 0.0,
		avg_input_tokens: 0.0,
		compression_stats: None,
		cache_markers_system: 0,
		cache_markers_tool: 0,
		cache_markers_content: 0,
		cache_non_cached_tokens: 0,
		agents_stats: None,
		supervisor_stats: None,
	});
}

#[test]
fn test_render_change_variants_model_effort_role() {
	display_model(&CommandOutput::Model {
		old_model: Some("ollama:old".to_string()),
		new_model: "ollama:new".to_string(),
		changed: true,
		saved: Some(false),
		save_error: Some("config file is read-only".to_string()),
	});
	display_model(&CommandOutput::Model {
		old_model: None,
		new_model: "ollama:new".to_string(),
		changed: true,
		saved: None,
		save_error: None,
	});
	display_effort(&CommandOutput::Effort {
		old_effort: Some("low".to_string()),
		new_effort: "high".to_string(),
		changed: true,
		saved: Some(false),
		save_error: Some("could not persist".to_string()),
	});
	display_role(&CommandOutput::Role {
		old_role: Some("assistant".to_string()),
		new_role: "task_refiner".to_string(),
		current_role: None,
		available_roles: None,
		changed: true,
		saved: Some(false),
		save_error: Some("could not persist".to_string()),
	});
	display_role(&CommandOutput::Role {
		old_role: None,
		new_role: String::new(),
		current_role: Some("assistant".to_string()),
		available_roles: Some(vec!["assistant".to_string(), "reduce".to_string()]),
		changed: false,
		saved: None,
		save_error: None,
	});
}

#[test]
fn test_render_prompt_arms() {
	display_prompt(&CommandOutput::Prompt {
		data: json!({"action": "list", "prompts": [
			{"name": "estimate", "description": "estimate a task"},
			{"name": "review", "description": "review a diff"}
		]}),
	});
	display_prompt(&CommandOutput::Prompt {
		data: json!({"action": "list", "prompts": []}),
	});
	display_prompt(&CommandOutput::Prompt {
		data: json!({"action": "execute", "success": true, "prompt_name": "estimate"}),
	});
	display_prompt(&CommandOutput::Prompt {
		data: json!({
			"action": "execute",
			"success": false,
			"error": "no such template",
			"available_prompts": ["estimate", "review"]
		}),
	});
}

#[test]
fn test_render_schedule_and_monitor_list_arms() {
	// Empty list → onboarding examples
	display_schedule(&CommandOutput::Schedule {
		data: json!({"subcommand": "list", "is_error": false, "message": "No scheduled entries."}),
	});
	// Populated list → raw rows
	display_schedule(&CommandOutput::Schedule {
		data: json!({"subcommand": "list", "is_error": false,
			"message": "1: in 5m — check build\n2: 9am — run tests"}),
	});
	// Failed list
	display_schedule(&CommandOutput::Schedule {
		data: json!({"subcommand": "list", "is_error": true, "message": "scheduler unavailable"}),
	});
	// Any other subcommand falls through to the generic message arm
	display_schedule(&CommandOutput::Schedule {
		data: json!({"subcommand": "add", "is_error": false, "message": "scheduled #3"}),
	});
	display_monitor(&CommandOutput::Monitor {
		data: json!({"subcommand": "list", "is_error": false,
			"message": "mon-1 running: watching the build log"}),
	});
}
