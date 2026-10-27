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
fn test_effort_levels_start_with_none() {
	// `/effort` accepts `none`; the levels it prints have to say so.
	assert!(effort_levels().starts_with("none"));
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
			"storage": {
				"hot_items": 2, "hot_tokens": 120,
				"cold_items": 1, "cold_tokens": 40,
				"scoped_hot": 1, "global_hot": 1,
				"scoped_cold": 1, "global_cold": 0,
				"by_type": {"learning": {"hot": 2, "cold": 1}}
			},
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
		windows: vec![UsageWindow {
			label: "billing period".to_string(),
			spent_usd: 16.0,
			reserved_usd: Some(0.75),
			allowance_usd: 20.0,
			resets_at: "2026-08-24T00:00:00Z".to_string(),
		}],
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
	display_status(&CommandOutput::Status {
		data: json!({
			"view": "agents",
			"running": [json!({
				"id": "run-1",
				"role": "developer",
				"elapsed_secs": 95,
				"tokens_input": 12_400,
				"tokens_output": 850,
				"cost": 0.0421,
				"last_action": "editing src/main.rs"
			})],
			"finished": [json!({
				"id": "run-0",
				"role": "researcher",
				"status": "done",
				"elapsed_secs": 30,
				"finished_secs_ago": 600,
				"tokens_input": 3_000_000,
				"tokens_output": 1_000,
				"cost": 1.25
			})],
			"detail": null,
			"total": 2,
		}),
	});
	display_status(&CommandOutput::Status {
		data: json!({
			"view": "agents", "running": [], "finished": [], "detail": null, "total": 0,
		}),
	});
	display_status(&CommandOutput::Status {
		data: json!({
			"view": "agents", "running": [], "finished": [],
			"detail": json!({
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
			}),
			"total": 1,
		}),
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
fn test_render_schedule_and_status_arms() {
	display_schedule(&CommandOutput::Schedule {
		data: json!({"subcommand": "help"}),
	});
	display_schedule(&CommandOutput::Schedule {
		data: json!({"subcommand": "error", "message": "bad when= expression"}),
	});
	display_status(&CommandOutput::Status {
		data: json!({"view": "error", "message": "no such status item"}),
	});
	display_status(&CommandOutput::Status {
		data: json!({"view": "overview", "active": 0, "agents": [], "jobs": [], "monitors": []}),
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
	let cstats = crate::session::CompressionStats {
		conversation_compressions: 2,
		total_messages_removed: 14,
		total_tokens_saved: 9_000,
		input_tokens: 1_200,
		..Default::default()
	};

	display_info(&CommandOutput::Info {
		session_name: "info-render".to_string(),
		model: "ollama:fake-model".to_string(),
		role: "assistant".to_string(),
		tokens_input: 120_000,
		tokens_output: 8_500,
		tokens_used: 224_000,
		tokens_cached: 90_000,
		tokens_cache_write: 4_000,
		tokens_reasoning: 1_500,
		total_cost: 1.2345,
		cache_savings: 0.42,
		tokens_per_second: 55.5,
		timing: crate::session::chat::session::commands::InfoTiming {
			model_time_ms: 180_000,
			turn_model_time_ms: 180_000,
			split_turns: 3,
			split_time_ms: 240_000,
			supervisor_blocking_time_ms: 20_000,
			tools_runtime_time_ms: 40_000,
			requests: 12,
			avg_request_time_ms: 15_000,
			completed_turns: 3,
			total_turn_time_ms: 240_000,
			avg_turn_time_ms: 80_000,
			last_turn_time_ms: 65_000,
		},
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
		supervisor_accounting: {
			let mut info = crate::session::SessionInfo::default();
			info.supervisor_accounting.kinds.insert(
				crate::supervisor::stats::CallKind::Gate,
				crate::supervisor::accounting::Usage {
					calls: 2,
					input_tokens: 50,
					output_tokens: 10,
					reasoning_tokens: 5,
					api_time_ms: 100,
					blocking_wall_time_ms: 250,
					cost: 0.02,
					..Default::default()
				},
			);
			info.supervisor_accounting.seams.insert(
				crate::supervisor::evaluate::Seam::Gate,
				crate::supervisor::accounting::Usage {
					calls: 1,
					background_wall_time_ms: 100,
					..Default::default()
				},
			);
			info.supervisor_accounting.turn_time = crate::supervisor::accounting::TurnTime {
				completed: 3,
				total_time_ms: 240_000,
				main_api_time_ms: 180_000,
				blocking_supervisor_time_ms: 20_000,
			};
			crate::supervisor::report::SupervisorReport::for_session(&info)
		},
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
		timing: Default::default(),
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
		supervisor_accounting: Default::default(),
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
	display_status(&CommandOutput::Status {
		data: json!({"view": "monitors", "active": 1, "monitors": [{
			"id": "mon-1", "description": "watching the build log", "command": "tail -f build.log",
			"workdir": "/tmp", "elapsed_secs": 2, "flush_interval_secs": 5,
			"max_batch_bytes": 4096, "timeout_ms": null
		}]}),
	});
}

#[test]
fn test_render_learning_show_and_evolution_arms() {
	// Show: full metadata, related links, evidence provenance
	display_learning(&CommandOutput::Learning {
		data: json!({
			"subcommand": "show",
			"index": 3,
			"memory_type": "experience",
			"title": "coverage workflow",
			"content": "line one\nline two",
			"outcome": "verified",
			"confidence": "high",
			"scope": "project",
			"path": "lessons/testing.md",
			"related": ["L1", "L2"],
			"evidence": ["session://abc/message/4"]
		}),
	});
	// Show: bare record without optional metadata
	display_learning(&CommandOutput::Learning {
		data: json!({"subcommand": "show", "index": 1, "content": "only content"}),
	});
	// Evolution list: populated and empty
	display_learning(&CommandOutput::Learning {
		data: json!({
			"subcommand": "evolution_list",
			"project": "octomind",
			"domain": "testing",
			"records": [
				{"id": "evo-1", "name": "run-tests-on-box", "kind": "skill", "state": "trial"},
				{"id": "evo-2", "name": "verify-before-done", "kind": "rule", "state": "shadow"}
			]
		}),
	});
	display_learning(&CommandOutput::Learning {
		data: json!({"subcommand": "evolution_list", "records": []}),
	});
	// Evolution show: with and without the native artifact
	display_learning(&CommandOutput::Learning {
		data: json!({
			"subcommand": "evolution_show",
			"record": {"name": "run-tests-on-box", "state": "trial"},
			"native_artifact": "# SKILL.md\nRun tests on the box."
		}),
	});
	display_learning(&CommandOutput::Learning {
		data: json!({"subcommand": "evolution_show", "record": {"name": "bare", "state": "shadow"}}),
	});
	// Evolution action outcomes
	for action in ["approve", "reject", "rollback"] {
		display_learning(&CommandOutput::Learning {
			data: json!({
				"subcommand": "evolution_action",
				"action": action,
				"record": {"name": "run-tests-on-box", "state": "trial"}
			}),
		});
	}
}

#[test]
fn test_render_mcp_info_string_tools_and_empty() {
	// String tool lists (configured, not discovered) and an empty-tools server
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "info".to_string(),
		data: json!({
			"subcommand": "info",
			"servers": [
				{"name": "configured", "health": "healthy", "connection_type": "stdio",
				 "tools": ["alpha", "beta"]},
				{"name": "bare", "health": "healthy", "connection_type": "http", "tools": []}
			]
		}),
	});
	// Empty-config message arm
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "info".to_string(),
		data: json!({"subcommand": "info", "servers": [], "message": "No MCP servers configured"}),
	});
}

#[test]
fn test_render_mcp_full_schema_edge_cases() {
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "full".to_string(),
		data: json!({
			"subcommand": "full",
			"servers": [
				{"name": "cov", "health": "healthy", "connection_type": "builtin",
				 "tools": ["t1", "t2", "t3"]},
				{"name": "empty", "health": "healthy", "connection_type": "builtin", "tools": []}
			],
			"tools": {
				"cov": [
					// Enum + default + required parameter rendering
					{"name": "t1", "description": "",
					 "parameters": {"type": "object",
						"properties": {"mode": {"type": "string", "enum": ["fast", "slow"],
							"default": "fast", "description": "How to run"}},
						"required": ["mode"]}},
					// Non-object schema falls back to raw display
					{"name": "t2", "description": "Non-object schema",
					 "parameters": {"type": "string"}},
					// Plain object schema, no enum/default
					{"name": "t3", "description": "Plain",
					 "parameters": {"type": "object", "properties": {"n": {"type": "number"}}}}
				],
				"empty": []
			}
		}),
	});
}

#[test]
fn test_render_mcp_health_zero_extras() {
	// A server row with only name+health: no restart counters, never checked
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "health".to_string(),
		data: json!({
			"subcommand": "health",
			"monitor_running": true,
			"servers": [
				{"name": "z", "health": "running", "last_checked_secs_ago": null}
			]
		}),
	});
}

#[test]
fn test_render_skill_pattern_and_mid_page() {
	display_skill(&CommandOutput::Skill {
		data: json!({
			"subcommand": "list",
			"total": 40,
			"active_count": 1,
			"page": 2,
			"total_pages": 3,
			"pattern": "cov*",
			"skills": [
				{"name": "coverage", "description": "Measures coverage", "active": true,
				 "scripts": ["activate", "validate"]},
				{"name": "cov-report", "description": "", "active": false}
			]
		}),
	});
}

#[test]
fn test_render_list_mid_page_nav_and_plain_text() {
	let config = test_config();
	display_list(
		&CommandOutput::List {
			sessions: vec![json!({
				"name": "mid-page",
				"title": "a session on page two",
				"created": "2026-08-19 11:00",
				"model": "ollama:fake-model",
				"tokens": 500,
				"cost": 0.5,
				"is_current": false
			})],
			total_sessions: 40,
			page: 2,
			total_pages: 3,
			plain_text: None,
		},
		&config,
	);
	// plain_text overrides the table rendering entirely
	display_list(
		&CommandOutput::List {
			sessions: Vec::new(),
			total_sessions: 1,
			page: 1,
			total_pages: 1,
			plain_text: Some("just-a-session-name".to_string()),
		},
		&config,
	);
}

#[test]
fn test_render_status_agents_and_minimal_detail() {
	// Finished rows carry a status; running rows may lack usage entirely
	display_status(&CommandOutput::Status {
		data: json!({
			"view": "agents", "running": [json!({
				"id": "r-min",
				"role": "researcher",
				"elapsed_secs": 3
			})], "finished": [json!({
				"id": "f-cancel",
				"role": "developer",
				"status": "cancelled",
				"ago_secs": 45
			})], "detail": null, "total": 2,
		}),
	});
	// Minimal detail card: no model/tokens/cost/last_action → placeholder text
	display_status(&CommandOutput::Status {
		data: json!({
			"view": "agents", "running": [], "finished": [], "detail": json!({
				"id": "d-min",
				"role": "developer",
				"status": "running",
				"elapsed_secs": 7
			}), "total": 1,
		}),
	});
}

#[test]
fn test_render_loglevel_direct_arms() {
	// changed + old → from/to/note rows
	display_loglevel(&CommandOutput::Loglevel {
		old_level: Some("info".to_string()),
		new_level: Some("debug".to_string()),
		current_level: None,
		available_levels: vec!["none".to_string(), "info".to_string(), "debug".to_string()],
		changed: true,
	});
	// changed but no new level → bare "changed" close
	display_loglevel(&CommandOutput::Loglevel {
		old_level: None,
		new_level: None,
		current_level: None,
		available_levels: Vec::new(),
		changed: true,
	});
	// unchanged + current → current/available rows
	display_loglevel(&CommandOutput::Loglevel {
		old_level: None,
		new_level: None,
		current_level: Some("debug".to_string()),
		available_levels: vec!["none".to_string(), "info".to_string(), "debug".to_string()],
		changed: false,
	});
	// unchanged + no current → fallback arm
	display_loglevel(&CommandOutput::Loglevel {
		old_level: None,
		new_level: None,
		current_level: None,
		available_levels: Vec::new(),
		changed: false,
	});
}

#[test]
fn test_help_rows_mention_aliases() {
	use crate::session::chat::commands::*;

	let rows = help_builtin_rows();
	assert!(
		rows.iter()
			.any(|(cmd, desc)| *cmd == HELP_COMMAND && desc.contains(HELP_COMMAND_ALT)),
		"help row must mention the {} alias",
		HELP_COMMAND_ALT
	);
	assert!(
		rows.iter()
			.any(|(cmd, desc)| *cmd == EXIT_COMMAND && desc.contains(QUIT_COMMAND)),
		"exit row must mention the {} alias",
		QUIT_COMMAND
	);
}

#[test]
fn test_render_help_with_custom_commands() {
	use crate::session::layers::layer_trait::{InputMode, LayerConfig, OutputMode, OutputRole};

	let mut config = test_config();
	config.commands = Some(vec![LayerConfig {
		name: "estimate".to_string(),
		description: "estimate a task".to_string(),
		command: "task-estimator".to_string(),
		workdir: ".".to_string(),
		input_mode: InputMode::Last,
		output_mode: OutputMode::None,
		output_role: OutputRole::Assistant,
	}]);
	display_help(&CommandOutput::Help { commands: Vec::new() }, &config);

	// No custom commands configured → builtin listing only
	let config = test_config();
	display_help(&CommandOutput::Help { commands: Vec::new() }, &config);
}

#[test]
fn test_render_report_sparse_rows() {
	let config = test_config();
	// Rows missing optional cells must fall back to placeholders, not panic
	display_report(
		&CommandOutput::Report {
			entries: vec![
				json!({"user_request": "sparse row"}),
				json!({"user_request": "only cost", "cost": "$0.10"}),
			],
			totals: json!({"cost": "$0.10"}),
		},
		&config,
	);
}

#[test]
fn test_render_info_partial_sections() {
	// Some sections present, others absent; failed learning outcome
	display_info(&CommandOutput::Info {
		session_name: "partial".to_string(),
		model: "ollama:fake-model".to_string(),
		role: "assistant".to_string(),
		tokens_input: 10,
		tokens_output: 5,
		tokens_used: 15,
		tokens_cached: 0,
		tokens_cache_write: 0,
		tokens_reasoning: 0,
		total_cost: 0.01,
		cache_savings: 0.0,
		tokens_per_second: 0.0,
		timing: Default::default(),
		avg_tokens_per_compression: 0.0,
		avg_tokens_per_tool: 0.0,
		avg_tokens_per_response: 0.0,
		avg_input_tokens: 0.0,
		compression_stats: Some(crate::session::CompressionStats::default()),
		cache_markers_system: 0,
		cache_markers_tool: 0,
		cache_markers_content: 0,
		cache_non_cached_tokens: 0,
		agents_stats: Some(json!({
			"total": 1, "running": 0, "done": 1, "failed": 0,
			"tokens_input": 10, "tokens_output": 2, "tokens_cached": 0, "total_cost": 0.01
		})),
		supervisor_accounting: Default::default(),
	});
}

#[test]
fn test_render_mcp_dump_variants() {
	// Populated dump: numbered sections with pretty JSON bodies
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "dump".to_string(),
		data: json!({"subcommand": "dump", "tools": [
			{"name": "shell", "description": "Run a command",
			 "parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}}}
		]}),
	});
	// Empty tool list
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "dump".to_string(),
		data: json!({"subcommand": "dump", "tools": []}),
	});
	// tools key missing entirely
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "dump".to_string(),
		data: json!({"subcommand": "dump"}),
	});
}

#[test]
fn test_render_mcp_invalid_validate_empty_and_bare_list() {
	// Explicit invalid-subcommand payload → subcommand menu
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "bogus".to_string(),
		data: json!({"subcommand": "invalid"}),
	});
	// validate with no tools → early empty close
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "validate".to_string(),
		data: json!({"subcommand": "validate", "all_valid": true, "tools": []}),
	});
	// list without a servers key at all
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "list".to_string(),
		data: json!({"subcommand": "list"}),
	});
}

#[test]
fn test_render_mcp_info_empty_tools_map_and_health_stopped() {
	// tools present but an empty map → "No tools available." section
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "info".to_string(),
		data: json!({"subcommand": "info", "servers": [], "tools": {}}),
	});
	// Monitor stopped, no error, no servers → zero-count close
	display_mcp(&CommandOutput::Mcp {
		mcp_command: "health".to_string(),
		data: json!({"subcommand": "health", "monitor_running": false}),
	});
}

#[test]
fn test_render_learning_list_rich_rows_and_nav() {
	display_learning(&CommandOutput::Learning {
		data: json!({
			"subcommand": "list",
			"role": "assistant",
			"project": "octomind",
			"total": 30,
			"page": 2,
			"total_pages": 3,
			"pattern": "cov*",
			"lessons": [
				{
				"index": 4,
				"memory_type": "experience",
				"title": "an experience title long enough to exceed the eighty character display ceiling for the source line so it must be truncated",
				"content": "backing content",
				"importance": 0.5,
				"confidence": "medium",
				"scope": "project",
				"tags": ["coverage", "ci"],
				"created": "2026-08-20T08:15:30Z",
				"outcome": "verified",
				"related": ["L1", "L2"],
				"evidence": ["session://x/message/9"]
				},
				{"index": 5, "content": "plain lesson", "importance": 0.1}
			]
		}),
	});
	// Unknown learning subcommand renders nothing
	display_learning(&CommandOutput::Learning {
		data: json!({"subcommand": "bogus"}),
	});
}

#[test]
fn test_render_image_video_usage_and_clipboard() {
	// clipboard path renders the friendly label instead of a path
	display_image(&CommandOutput::Image {
		image_attached: true,
		path: Some("clipboard".to_string()),
		error: None,
	});
	// not attached, no error → usage/examples arm
	display_image(&CommandOutput::Image {
		image_attached: false,
		path: None,
		error: None,
	});
	display_video(&CommandOutput::Video {
		video_attached: false,
		path: None,
		error: None,
	});
}

#[test]
fn test_gb_bar_zero_limit_is_unlimited() {
	assert!(!gb_bar(1.0, 0.0).is_empty());
}

#[test]
fn test_render_schedule_generic_error_close() {
	display_schedule(&CommandOutput::Schedule {
		data: json!({"subcommand": "remove", "is_error": true, "message": "no such entry"}),
	});
}

#[test]
fn supervisor_table_merges_seams_hides_zero_rows_and_only_shows_real_background() {
	use crate::supervisor::{
		accounting::{TurnTime, Usage},
		evaluate::Seam,
		report::{Mechanic, Outcome, SupervisorReport},
		stats::CallKind,
	};
	let mut info = crate::session::SessionInfo::default();
	info.supervisor_accounting.kinds.insert(
		CallKind::Condense,
		Usage {
			blocking_wall_time_ms: 1_400,
			outcome: [(Outcome::Condensed, 2), (Outcome::SavedTokens, 5_700)].into(),
			..Default::default()
		},
	);
	let decision = Usage {
		calls: 2,
		input_tokens: 31_900,
		output_tokens: 2_300,
		api_time_ms: 1_300,
		cost: 0.0013,
		blocking_wall_time_ms: 1_300,
		..Default::default()
	};
	info.supervisor_accounting
		.kinds
		.insert(CallKind::Evaluate, decision.clone());
	info.supervisor_accounting.seams.insert(Seam::Condense, decision);
	info.supervisor_accounting
		.kinds
		.insert(CallKind::Resolve, Usage::default());
	info.supervisor_accounting.turn_time = TurnTime {
		completed: 1,
		total_time_ms: 2_000,
		main_api_time_ms: 100,
		blocking_supervisor_time_ms: 1_400,
	};
	info.supervisor_accounting.blocking_wall_time_ms = 1_400;
	let report = SupervisorReport::for_session(&info);
	let table = plain_info_render(&supervisor_table(&report, 120));
	assert!(table.contains("chat"));
	assert!(table.contains("decision"));
	assert!(table.contains("1s 400ms"));
	assert!(table.contains("1s 300ms"));
	assert!(table.contains("2 condensed · saved 5.7K tok"));
	assert!(!table.contains("evaluate"));
	assert!(!table.contains("resolve"));
	assert!(!table.contains("background"));
	assert_eq!(table.lines().filter(|line| line.starts_with("condense")).count(), 1);
	assert_eq!(report.mechanics[&Mechanic::Condense].chat_calls, 0);
	assert_eq!(report.mechanics[&Mechanic::Condense].decision_calls, 2);
	let timing_rows = info_timing_rows(&super::super::InfoTiming {
		split_time_ms: report.turn_time.total_time_ms,
		turn_model_time_ms: report.turn_time.main_api_time_ms,
		supervisor_blocking_time_ms: report.turn_time.blocking_supervisor_time_ms,
		tools_runtime_time_ms: 500,
		..Default::default()
	});
	assert_eq!(plain_info_render(&timing_rows[0].1), "100ms");
	assert_eq!(
		plain_info_render(&timing_rows[1].1),
		"1s 400ms blocking · 70% of turn time"
	);
	assert_eq!(plain_info_render(&timing_rows[2].1), "500ms");
	info.supervisor_accounting.kinds.insert(
		CallKind::Distill,
		Usage {
			calls: 1,
			background_wall_time_ms: 500,
			..Default::default()
		},
	);
	info.supervisor_accounting.background_wall_time_ms = Some(500);
	assert!(supervisor_table(&SupervisorReport::for_session(&info), 120).contains("background"));
}

#[test]
fn supervisor_wall_only_and_local_rows_preserve_usage_without_duplicate_outcomes() {
	use crate::supervisor::{accounting::Usage, report::SupervisorReport, stats::CallKind};
	let mut info = crate::session::SessionInfo::default();
	info.supervisor_accounting.kinds.insert(
		CallKind::Condense,
		Usage {
			blocking_wall_time_ms: 1_400,
			..Default::default()
		},
	);
	info.supervisor_accounting.kinds.insert(
		CallKind::Hindsight,
		Usage {
			calls: 2,
			blocking_wall_time_ms: 800,
			..Default::default()
		},
	);
	let table = plain_info_render(&supervisor_table(&SupervisorReport::for_session(&info), 120));
	assert!(table
		.lines()
		.any(|line| line.starts_with("condense") && line.contains("1s 400ms")));
	assert!(!table.contains("no model usage"));
	assert!(table
		.lines()
		.any(|line| line.starts_with("hindsight") && line.contains("local") && line.contains("800ms")));
}

#[test]
fn supervisor_sample_renders_one_session_table_and_one_timing_split() {
	use crate::supervisor::{
		accounting::{TurnTime, Usage},
		evaluate::Seam,
		report::{Outcome, SupervisorReport},
		stats::CallKind,
	};
	let mut info = crate::session::SessionInfo::default();
	for (kind, calls, input, output, wall, api, cost) in [
		(CallKind::Gate, 4, 24_000, 1_600, 17_600, 12_000, 0.006),
		(CallKind::Authorize, 1, 8_000, 200, 9_400, 2_400, 0.004),
		(CallKind::Condense, 0, 0, 0, 1_400, 0, 0.0),
		(CallKind::Recall, 1, 6_000, 300, 2_200, 1_000, 0.0003),
		(CallKind::Resolve, 1, 200, 100, 1_300, 1_300, 0.0002),
		(CallKind::Hindsight, 2, 0, 0, 800, 0, 0.0),
	] {
		info.supervisor_accounting.kinds.insert(
			kind,
			Usage {
				calls,
				input_tokens: input,
				output_tokens: output,
				blocking_wall_time_ms: wall,
				api_time_ms: api,
				cost,
				..Default::default()
			},
		);
	}
	for (seam, calls, input, output, api, cost) in [
		(Seam::Gate, 2, 4_400, 400, 2_900, 0.0007),
		(Seam::Authorizer, 8, 37_100, 600, 3_200, 0.0012),
		(Seam::Condense, 2, 31_900, 2_300, 1_300, 0.0013),
		(Seam::Recall, 1, 2_000, 100, 200, 0.0001),
		(Seam::Capabilities, 1, 500, 100, 1_600, 0.0),
	] {
		info.supervisor_accounting.seams.insert(
			seam,
			Usage {
				calls,
				input_tokens: input,
				output_tokens: output,
				api_time_ms: api,
				blocking_wall_time_ms: api,
				cost,
				..Default::default()
			},
		);
	}
	info.supervisor_accounting
		.kinds
		.get_mut(&CallKind::Gate)
		.unwrap()
		.outcome = [(Outcome::Runs, 2), (Outcome::Fail, 1), (Outcome::RefutedGaps, 2)].into();
	info.supervisor_accounting
		.kinds
		.get_mut(&CallKind::Authorize)
		.unwrap()
		.outcome
		.insert(Outcome::SkippedJudge, 7);
	info.supervisor_accounting
		.kinds
		.get_mut(&CallKind::Condense)
		.unwrap()
		.outcome = [(Outcome::Condensed, 2), (Outcome::SavedTokens, 5_700)].into();
	info.supervisor_accounting
		.kinds
		.get_mut(&CallKind::Recall)
		.unwrap()
		.outcome
		.insert(Outcome::Items, 9);
	info.authorization.checked = 11;
	info.supervisor_accounting.blocking_wall_time_ms = 34_300;
	info.supervisor_accounting.turn_time = TurnTime {
		completed: 1,
		total_time_ms: 130_000,
		main_api_time_ms: 90_000,
		blocking_supervisor_time_ms: 34_300,
	};
	let report = SupervisorReport::for_session(&info);
	assert_eq!(
		(
			report.totals.chat_calls,
			report.totals.decision_calls,
			report.totals.api_ms
		),
		(9, 14, 25_900)
	);
	assert_eq!(
		(report.totals.input_tokens, report.totals.output_tokens),
		(114_100, 5_700)
	);
	let table = plain_info_render(&supervisor_table(&report, 120));
	assert!(!table.contains("evaluate"));
	assert!(!table.contains("background"));
	assert!(table.contains("$0.01380"));
	println!("supervisor · this session");
	println!("{}", table);
}

fn plain_info_render(text: &str) -> String {
	regex::Regex::new(r"\x1b\[[0-9;]*m")
		.expect("ANSI color pattern")
		.replace_all(text, "")
		.into_owned()
}

fn owner_info_timing() -> super::super::InfoTiming {
	super::super::InfoTiming {
		requests: 17,
		avg_request_time_ms: 6_688,
		completed_turns: 1,
		total_turn_time_ms: 135_799,
		avg_turn_time_ms: 135_799,
		last_turn_time_ms: 135_799,
		model_time_ms: 113_700,
		turn_model_time_ms: 113_700,
		split_turns: 1,
		split_time_ms: 135_799,
		supervisor_blocking_time_ms: 21_200,
		tools_runtime_time_ms: 936,
	}
}

fn owner_supervisor_report() -> crate::supervisor::report::SupervisorReport {
	use crate::supervisor::report::{Mechanic, MechanicUsage, Outcome, SupervisorReport, Totals};
	let mut report = SupervisorReport::default();
	for (mechanic, chat, decision, wall, api, input, output, cost) in [
		(Mechanic::Gate, 2, 0, 3_500, 3_500, 10_000, 268, 0.0018),
		(Mechanic::Authorize, 1, 16, 9_800, 9_700, 67_000, 1_100, 0.0032),
		(Mechanic::Condense, 0, 5, 2_900, 2_800, 66_000, 4_700, 0.0028),
		(Mechanic::Recall, 1, 1, 1_500, 1_500, 8_001, 371, 0.0004),
		(Mechanic::Resolve, 1, 0, 1_400, 1_400, 239, 80, 0.0001),
		(Mechanic::Hindsight, 1, 0, 994, 0, 0, 0, 0.0),
		(Mechanic::Capabilities, 0, 1, 1_100, 1_100, 527, 67, 0.0),
	] {
		report.mechanics.insert(
			mechanic,
			MechanicUsage {
				chat_calls: chat,
				decision_calls: decision,
				wall_blocking_ms: wall,
				api_ms: api,
				input_tokens: input,
				output_tokens: output,
				cost,
				..Default::default()
			},
		);
	}
	for (mechanic, outcomes) in [
		(Mechanic::Gate, vec![(Outcome::Runs, 1), (Outcome::Pass, 1)]),
		(
			Mechanic::Authorize,
			vec![
				(Outcome::Checked, 17),
				(Outcome::SkippedJudge, 16),
				(Outcome::Applied, 15),
				(Outcome::Replaced, 15),
			],
		),
		(
			Mechanic::Condense,
			vec![
				(Outcome::Condensed, 5),
				(Outcome::SavedTokens, 19_000),
				(Outcome::Applied, 5),
				(Outcome::Replaced, 5),
			],
		),
		(Mechanic::Recall, vec![(Outcome::Items, 10), (Outcome::Applied, 9)]),
	] {
		report.mechanics.get_mut(&mechanic).expect("sample mechanic").outcome = outcomes.into_iter().collect();
	}
	report.totals = Totals {
		chat_calls: 6,
		decision_calls: 23,
		wall_blocking_ms: 21_200,
		api_ms: 19_900,
		input_tokens: 153_000,
		output_tokens: 6_500,
		cost: 0.0083,
		..Default::default()
	};
	report.turn_time = crate::supervisor::accounting::TurnTime {
		completed: 1,
		total_time_ms: 135_799,
		main_api_time_ms: 113_700,
		blocking_supervisor_time_ms: 21_200,
	};
	report
}

#[test]
fn info_timing_split_uses_shared_durations_and_matched_turn_measurements() {
	let mut timing = owner_info_timing();
	timing.model_time_ms = 999_999;
	let before = serde_json::to_value(&timing).expect("timing JSON");
	let rows = info_timing_rows(&timing);
	assert_eq!(
		rows.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
		[
			"requests",
			"turns",
			"avg turn",
			"main API",
			"supervisor",
			"tools + runtime",
		]
	);
	assert_eq!(plain_info_render(&rows[0].1), "17 · 6s 688ms avg");
	assert_eq!(plain_info_render(&rows[1].1), "1 completed · 2m 15s 799ms active");
	assert_eq!(plain_info_render(&rows[2].1), "2m 15s 799ms · 2m 15s 799ms last");
	assert_eq!(plain_info_render(&rows[3].1), "1m 53s 700ms");
	assert_eq!(plain_info_render(&rows[4].1), "21s 200ms blocking · 16% of turn time");
	assert_eq!(plain_info_render(&rows[5].1), "936ms");
	assert_eq!(serde_json::to_value(&timing).expect("timing JSON"), before);

	timing.split_time_ms = 0;
	timing.split_turns = 0;
	assert_eq!(info_timing_rows(&timing).len(), 3);
	assert!(info_timing_rows(&Default::default()).is_empty());
}

#[test]
fn info_supervisor_owner_sample_fits_widths_preserves_payload_and_styles_values() {
	let report = owner_supervisor_report();
	let before = serde_json::to_value(&report).expect("supervisor JSON");
	for width in [120, 100, 80] {
		let styled = supervisor_table(&report, width);
		let table = plain_info_render(&styled);
		for line in table.lines() {
			assert!(
				line.chars().count() + "│   ".chars().count() <= width,
				"{width} columns: {line}"
			);
		}
		assert_eq!(table.lines().filter(|line| line.starts_with("total")).count(), 1);
		assert_eq!(table.lines().filter(|line| line.starts_with("authorize")).count(), 1);
		assert_eq!(table.lines().next().expect("header").contains("outcome"), width == 120);
		assert!(table.contains("1 run · 1 pass"));
		assert!(table.contains("17 checked · 16 skipped judge"));
		assert!(table.contains("5 condensed · saved 19K tok"));
		assert!(table.contains("10 items · 9 filtered"));
		assert!(table.contains("$0.00830"));
		assert!(!table.contains("decision applied"));
		assert!(!table.contains("chat calls replaced"));
		assert!(!table.contains("1 runs"));
		assert!(!table.contains('-'));
		let total = table
			.lines()
			.position(|line| line.starts_with("total"))
			.expect("total row");
		assert!(table
			.lines()
			.nth(total - 1)
			.expect("total rule")
			.chars()
			.all(|c| c == '─'));
		if width == 80 {
			let mut lines = table.lines();
			let gate = lines.find(|line| line.starts_with("gate")).expect("gate row");
			assert!(!gate.contains("run"));
			assert_eq!(lines.next(), Some("  1 run · 1 pass"));
			assert!(styled.contains(&"  1 run · 1 pass".dimmed().to_string()));
		}
		assert!(styled.contains(&"gate        ".bright_black().to_string()));
		assert!(styled.contains(&"    10K".bright_blue().to_string()));
		assert!(styled.contains(&"268".bright_green().to_string()));
		assert!(styled.contains(&"$0.00180".bright_yellow().to_string()));
	}
	assert_eq!(serde_json::to_value(&report).expect("supervisor JSON"), before);
}

#[test]
fn info_supervisor_background_long_durations_and_outcomes_fit_without_losing_values() {
	use crate::supervisor::report::{Mechanic, Outcome};
	let mut report = owner_supervisor_report();
	let gate = report.mechanics.get_mut(&Mechanic::Gate).expect("gate");
	gate.wall_background_ms = 3_661_500;
	gate.outcome = [
		(Outcome::Runs, 1),
		(Outcome::Pass, 2),
		(Outcome::Fail, 3),
		(Outcome::RefutedGaps, 4),
		(Outcome::Unavailable, 5),
		(Outcome::DecisionUnavailable, 6),
	]
	.into();
	for width in [120, 100, 80] {
		let table = plain_info_render(&supervisor_table(&report, width));
		assert!(table.contains("background"));
		assert!(table.contains(&format_duration(3_661_500)));
		assert!(table.contains("6 decision unavailable"));
		assert!(table.contains("1 run · 2 passes · 3 fails"));
		assert!(table.contains('–'));
		for line in table.lines() {
			assert!(
				line.chars().count() + "│   ".chars().count() <= width,
				"{width} columns: {line}"
			);
		}
	}
}

#[test]
fn info_supervisor_outcomes_pluralize_counts_and_keep_mechanic_results() {
	use crate::supervisor::report::{Mechanic, MechanicUsage, Outcome};
	for (outcome, singular, plural) in [
		(Outcome::Runs, "run", "runs"),
		(Outcome::Pass, "pass", "passes"),
		(Outcome::Fail, "fail", "fails"),
		(Outcome::RefutedGaps, "gap refuted", "gaps refuted"),
		(Outcome::Items, "item", "items"),
		(Outcome::Lessons, "lesson", "lessons"),
		(Outcome::Experiences, "experience", "experiences"),
		(Outcome::PlanCalls, "planner call", "planner calls"),
		(Outcome::Skips, "skip", "skips"),
	] {
		for (count, label) in [(1, singular), (2, plural)] {
			let usage = MechanicUsage {
				outcome: [(outcome, count)].into(),
				..Default::default()
			};
			assert_eq!(supervisor_outcome(Mechanic::Gate, &usage), format!("{count} {label}"));
		}
	}
	for (mechanic, label) in [
		(Mechanic::Skills, "activated"),
		(Mechanic::Capabilities, "activated"),
		(Mechanic::Recall, "filtered"),
		(Mechanic::Compression, "demoted"),
	] {
		let usage = MechanicUsage {
			outcome: [(Outcome::Applied, 1)].into(),
			..Default::default()
		};
		assert_eq!(supervisor_outcome(mechanic, &usage), format!("1 {label}"));
	}
}

#[test]
fn info_restyle_render_samples() {
	let timing = owner_info_timing();
	let report = owner_supervisor_report();
	for width in [120, 80] {
		println!("INFO SAMPLE {width}");
		block_section("timing");
		let rows = info_timing_rows(&timing);
		let kw = key_width(rows.iter().map(|(key, _)| *key));
		for (key, value) in &rows {
			block_row(key, value, kw);
		}
		block_section_with("supervisor", "this session");
		for line in supervisor_table(&report, width).lines() {
			block_row_text(line);
		}
		println!("END INFO SAMPLE");
	}
	println!("INFO SAMPLE zero activity");
	let mut empty_report = crate::supervisor::report::SupervisorReport::default();
	empty_report.turn_time.completed = 1;
	assert!(supervisor_table(&empty_report, 80).is_empty());
	display_info(&CommandOutput::Info {
		session_name: "zero-supervisor".into(),
		model: "m".into(),
		role: "assistant".into(),
		tokens_input: 0,
		tokens_output: 0,
		tokens_used: 0,
		tokens_cached: 0,
		tokens_cache_write: 0,
		tokens_reasoning: 0,
		total_cost: 0.0,
		cache_savings: 0.0,
		tokens_per_second: 0.0,
		timing: Default::default(),
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
		supervisor_accounting: empty_report,
	});
	println!("END INFO SAMPLE");
}
