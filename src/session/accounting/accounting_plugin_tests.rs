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
use crate::session::{load_session, SessionInfo};

fn session(name: &str) -> Session {
	Session {
		info: SessionInfo {
			name: name.into(),
			..Default::default()
		},
		messages: Vec::new(),
		session_file: None,
	}
}

fn spent(label: &str, calls: u64, cost: f64, wall_ms: u64, blocking: bool) -> protocol::Usage {
	protocol::Usage {
		label: label.into(),
		calls,
		input_tokens: 10 * calls,
		output_tokens: 2 * calls,
		reasoning_tokens: 0,
		cost,
		wall_ms,
		blocking,
	}
}

#[test]
fn entries_with_one_label_sum_up() {
	let mut session = session("plugin-usage-sum");
	record_plugin_usage(&mut session, "a", &spent("judge", 2, 0.5, 100, false));
	record_plugin_usage(&mut session, "b", &spent("judge", 1, 0.25, 50, false));
	record_plugin_usage(&mut session, "a", &spent("recall", 4, 1.0, 10, false));

	let books = &session.info.supervisor_accounting.plugin_usage;
	assert_eq!(books.len(), 2);
	let judge = &books["judge"];
	assert_eq!((judge.calls, judge.input_tokens, judge.output_tokens), (3, 30, 6));
	assert_eq!((judge.cost, judge.background_wall_time_ms), (0.75, 150));
	assert_eq!(books["recall"].calls, 4);
}

#[test]
fn cost_reaches_the_session_total_once() {
	let mut session = session("plugin-usage-total");
	record_plugin_usage(&mut session, "a", &spent("judge", 2, 0.5, 0, false));
	record_plugin_usage(&mut session, "a", &spent("judge", 1, 0.25, 0, false));
	assert_eq!(session.info.total_cost, 0.75);

	// Folding the books of spend from outside the main loop must not count it again.
	session.fold_external_spend();
	assert_eq!(session.info.total_cost, 0.75);
	assert_eq!(session.info.supervisor_accounting.plugin_usage["judge"].cost, 0.75);
	assert_eq!(session.info.supervisor_accounting.totals().cost, 0.0);
}

#[test]
fn only_blocking_entries_add_to_the_blocking_time() {
	let mut session = session("plugin-usage-blocking");
	record_plugin_usage(&mut session, "a", &spent("judge", 1, 0.0, 400, true));
	record_plugin_usage(&mut session, "a", &spent("judge", 1, 0.0, 600, false));
	let accounting = &session.info.supervisor_accounting;
	assert_eq!(accounting.blocking_wall_time_ms, 400);
	assert_eq!(accounting.plugin_usage["judge"].blocking_wall_time_ms, 400);
	assert_eq!(accounting.plugin_usage["judge"].background_wall_time_ms, 600);
}

#[test]
fn the_map_survives_saving_and_resuming() {
	let file = tempfile::Builder::new().suffix(".jsonl.zst").tempfile().unwrap();
	let mut session = session("plugin-usage-resume");
	session.session_file = Some(file.path().to_path_buf());
	record_plugin_usage(&mut session, "a", &spent("judge", 2, 0.5, 100, true));
	session.save().unwrap();
	session.save().unwrap();

	let resumed = load_session(&file.path().to_path_buf()).unwrap();
	assert_eq!(
		resumed.info.supervisor_accounting.plugin_usage,
		session.info.supervisor_accounting.plugin_usage
	);
	assert_eq!(resumed.info.supervisor_accounting.blocking_wall_time_ms, 100);
	assert_eq!(resumed.info.total_cost, 0.5);
}

#[test]
fn books_without_plugin_usage_keep_their_old_shape() {
	let mut value = serde_json::to_value(SupervisorAccounting::default()).unwrap();
	assert!(value.get("plugin_usage").is_none());
	value.as_object_mut().unwrap().remove("plugin_usage");
	let loaded: SupervisorAccounting = serde_json::from_value(value).unwrap();
	assert!(loaded.plugin_usage.is_empty());
}
