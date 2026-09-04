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

fn entry(user_request: &str, tools_used: &str) -> ReportEntry {
	ReportEntry {
		user_request: user_request.to_string(),
		cost: "0.01000".to_string(),
		tool_calls: 2,
		tools_used: tools_used.to_string(),
		task_time: "1.0s".to_string(),
		ai_time: "0.5s".to_string(),
		processing_time: "0.5s".to_string(),
	}
}

fn report(entries: Vec<ReportEntry>) -> SessionReport {
	let total_requests = entries.len() as u32;
	SessionReport {
		entries,
		totals: ReportTotals {
			total_cost: 0.02,
			total_tool_calls: 4,
			total_task_time_ms: 2_000,
			total_ai_time_ms: 1_000,
			total_processing_time_ms: 1_000,
			total_requests,
		},
	}
}

#[test]
fn tools_used_is_sorted_and_dash_when_empty() {
	assert_eq!(SessionReport::format_tools_used(&HashMap::new()), "-");

	let tools = HashMap::from([
		("shell".to_string(), 3u32),
		("read".to_string(), 1u32),
		("write".to_string(), 2u32),
	]);
	// Sorted so the same tool set always renders identically.
	assert_eq!(
		SessionReport::format_tools_used(&tools),
		"read(1), shell(3), write(2)"
	);
}

#[test]
fn truncate_request_respects_the_cap_in_chars() {
	assert_eq!(SessionReport::truncate_request("short", 35), "short");
	// Exactly at the cap is untouched.
	let exact = "x".repeat(35);
	assert_eq!(SessionReport::truncate_request(&exact, 35), exact);

	let long = "y".repeat(100);
	let out = SessionReport::truncate_request(&long, 35);
	assert_eq!(out.chars().count(), 35);
	assert!(out.ends_with("..."));
}

#[test]
fn truncate_request_does_not_split_multibyte_chars() {
	// A byte-based slice would panic here.
	let long = "日".repeat(100);
	let out = SessionReport::truncate_request(&long, 35);
	assert_eq!(out.chars().count(), 35);
	assert!(out.starts_with('日'));
}

#[test]
fn escape_markdown_protects_table_cells() {
	let r = report(vec![]);
	// A raw pipe or newline in a cell would break the table layout.
	assert_eq!(r.escape_markdown("a|b"), "a\\|b");
	assert_eq!(r.escape_markdown("line1\nline2"), "line1 line2");
	assert_eq!(r.escape_markdown("crlf\r\n"), "crlf ");
}

#[test]
fn markdown_table_escapes_every_cell_it_renders() {
	let r = report(vec![entry("fix a|b bug", "shell|read(1)")]);
	let table = r.generate_markdown_table();
	let row = table
		.lines()
		.find(|l| l.contains("fix a"))
		.expect("entry row present");
	// The row keeps exactly the 8 pipes of a 7-column table — the two
	// pipes coming from the data are escaped, not counted as separators.
	assert_eq!(row.matches("\\|").count(), 2);
	assert_eq!(row.replace("\\|", "").matches('|').count(), 8);
	assert!(table.contains("**TOTAL**"));
}

#[test]
fn markdown_table_has_header_separator_and_one_row_per_entry() {
	let r = report(vec![entry("first", "read(1)"), entry("second", "-")]);
	let table = r.generate_markdown_table();
	let lines: Vec<&str> = table.lines().collect();
	// header + separator + 2 entries + totals
	assert_eq!(lines.len(), 5);
	assert!(lines[1].starts_with("|---"));
	assert!(lines[2].contains("first"));
	assert!(lines[3].contains("second"));
	assert!(lines[4].contains("**TOTAL**"));
}

#[test]
fn json_report_mirrors_entries_and_totals() {
	let r = report(vec![entry("do a thing", "read(1)")]);
	let json = r.to_json();
	assert_eq!(json["entries"].as_array().unwrap().len(), 1);
	assert_eq!(json["entries"][0]["user_request"], "do a thing");
	assert_eq!(json["totals"]["total_requests"], 1);
	assert_eq!(json["totals"]["total_tool_calls"], 4);
}

#[test]
fn plain_string_drops_markdown_markers() {
	let r = report(vec![entry("do a thing", "read(1)")]);
	let plain = r.to_plain_string();
	assert!(!plain.contains("**"));
	assert!(!plain.contains('|'));
	assert!(plain.contains("Session Usage Report"));
	assert!(plain.contains("do a thing"));
}

#[test]
fn empty_report_still_renders_a_table_and_summary() {
	let mut r = report(vec![]);
	r.totals.total_requests = 0;
	let md = r.to_markdown_string();
	assert!(md.contains("| **TOTAL** |"));
	assert!(md.contains("**0** requests"));
}

#[test]
fn generate_from_log_tracks_messages_commands_tools_cost_and_time() {
	let dir = tempfile::tempdir().expect("temp dir");
	let path = dir.path().join("report.jsonl.zst");
	let entries = [
		serde_json::json!({"role":"user","content":"build the report","timestamp":100}),
		serde_json::json!({
			"role":"assistant",
			"content":"working",
			"timestamp":102,
			"tool_calls":[{"name":"shell"},{"name":"view"}]
		}),
		serde_json::json!({
			"type":"STATS","timestamp":103,"total_cost":0.5,
			"total_api_time_ms":100,"total_tool_time_ms":30
		}),
		serde_json::json!({"type":"COMMAND","command":"/info","timestamp":110}),
		serde_json::json!({"type":"TOOL_CALL","tool_name":"schedule","timestamp":111}),
		serde_json::json!({
			"type":"SUMMARY","timestamp":112,
			"session_info":{
				"total_cost":1.0,"total_api_time_ms":160,"total_tool_time_ms":50
			}
		}),
		serde_json::json!({"role":"assistant","content":"done","timestamp":114}),
	];
	for entry in entries {
		crate::session::append_to_session_file(&path, &entry.to_string())
			.expect("append report frame");
	}

	let report = SessionReport::generate_from_log(path.to_str().unwrap()).expect("report");
	assert_eq!(report.entries.len(), 2);
	assert_eq!(report.entries[0].user_request, "build the report");
	assert_eq!(report.entries[0].tool_calls, 2);
	assert_eq!(report.entries[0].tools_used, "shell(1), view(1)");
	assert_eq!(report.entries[0].cost, "0.50000");
	assert_eq!(report.entries[1].user_request, "/info");
	assert_eq!(report.entries[1].tools_used, "schedule(1)");
	assert_eq!(report.totals.total_requests, 2);
	assert_eq!(report.totals.total_tool_calls, 3);
	assert!((report.totals.total_cost - 1.0).abs() < f64::EPSILON);
	assert_eq!(report.totals.total_ai_time_ms, 160);
	assert_eq!(report.totals.total_processing_time_ms, 50);
	assert_eq!(report.totals.total_task_time_ms, 7_000);
}

#[test]
fn generate_from_log_rejects_missing_or_invalid_zstd_files() {
	let dir = tempfile::tempdir().expect("temp dir");
	let missing = dir.path().join("missing.zst");
	assert!(SessionReport::generate_from_log(missing.to_str().unwrap()).is_err());

	let invalid = dir.path().join("invalid.zst");
	std::fs::write(&invalid, b"not zstd").unwrap();
	assert!(SessionReport::generate_from_log(invalid.to_str().unwrap()).is_err());
}

#[test]
fn generate_from_log_ignores_injected_user_turns_and_relogged_messages() {
	let dir = tempfile::tempdir().expect("temp dir");
	let path = dir.path().join("report.jsonl.zst");
	let real_turn = serde_json::json!({"role":"user","content":"fix the parser","timestamp":100});
	let assistant = serde_json::json!({
		"role":"assistant",
		"content":"on it",
		"timestamp":101,
		"tool_calls":[{"name":"shell"}]
	});
	let entries = [
		serde_json::json!({"role":"user","content":"<skill name=\"git\">body</skill>","timestamp":90}),
		real_turn.clone(),
		assistant.clone(),
		serde_json::json!({"role":"user","content":"<pay-attention>steer</pay-attention>","timestamp":102}),
		serde_json::json!({"role":"user","content":"<system-note>job done</system-note>","timestamp":103}),
		serde_json::json!({"role":"user","content":"<continuation>resume</continuation>","timestamp":104}),
		serde_json::json!({"type":"COMPRESSION_POINT","timestamp":105}),
		// Compression re-appends the surviving messages verbatim.
		real_turn,
		assistant,
	];
	for entry in entries {
		crate::session::append_to_session_file(&path, &entry.to_string())
			.expect("append report frame");
	}

	let report = SessionReport::generate_from_log(path.to_str().unwrap()).expect("report");
	assert_eq!(report.entries.len(), 1);
	assert_eq!(report.entries[0].user_request, "fix the parser");
	assert_eq!(report.entries[0].tool_calls, 1);
	assert_eq!(report.totals.total_requests, 1);
}
