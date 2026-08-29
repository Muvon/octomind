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

//! Tests for the `/agents` command handler over an empty tap-run registry:
//! the list view, the unknown-id detail error, and the stats aggregation
//! used by `/info`.

use super::*;
use crate::session::tap_runs::{TapJob, TapJobStatus, TapLiveState, TapLiveUsage};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use tokio::sync::watch;

#[test]
fn test_agents_list_view() {
	// Registry contents depend on what other tests in this process ran —
	// only the output shape is stable: a list view with no detail card.
	let result = handle_agents(&[]).expect("dispatch");
	match result {
		CommandResult::HandledWithOutput(output) => match *output {
			CommandOutput::Agents { detail, .. } => {
				assert!(detail.is_none());
			}
			other => panic!("expected Agents output, got {other:?}"),
		},
		other => panic!("expected HandledWithOutput, got {other:?}"),
	}
}

#[test]
fn test_agents_unknown_id_is_an_error() {
	let result = handle_agents(&["no-such-agent-id"]).expect("dispatch");
	match result {
		CommandResult::HandledWithOutput(output) => match *output {
			CommandOutput::Error { error, .. } => {
				assert!(error.contains("no-such-agent-id"), "{error}");
			}
			other => panic!("expected Error output, got {other:?}"),
		},
		other => panic!("expected HandledWithOutput, got {other:?}"),
	}
}

#[test]
fn test_agents_stats_shape_when_present() {
	// Other tests in this process may have recorded runs; when stats exist
	// they must carry the aggregate keys /info renders.
	if let Some(stats) = get_agents_stats() {
		assert!(stats.get("total").is_some(), "{stats}");
	}
}

fn job(id: &str, status: TapJobStatus, live: TapLiveState) -> TapJob {
	let (cancel_tx, _cancel_rx) = watch::channel(false);
	TapJob {
		id: id.to_string(),
		role: "developer:general".to_string(),
		workdir: "/tmp/project".to_string(),
		started_at: SystemTime::now(),
		status: Arc::new(RwLock::new(status)),
		cancel_tx,
		live: Arc::new(RwLock::new(live)),
	}
}

#[tokio::test]
#[serial_test::serial]
async fn list_detail_and_stats_cover_every_job_status() {
	let session_id = "agents-command-statuses".to_string();
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::tap_runs::init_for_session();
		crate::session::tap_runs::register_job(job(
			"running-agent",
			TapJobStatus::Running,
			TapLiveState {
				last_action: Some("shell cargo test".to_string()),
				usage: Some(TapLiveUsage {
					input_tokens: 100,
					output_tokens: 20,
					cache_read_tokens: 50,
					cost: 0.25,
				}),
			},
		));
		crate::session::tap_runs::register_job(job(
			"done-agent",
			TapJobStatus::Done,
			TapLiveState::default(),
		));
		crate::session::tap_runs::register_job(job(
			"failed-agent",
			TapJobStatus::Failed,
			TapLiveState::default(),
		));
		crate::session::tap_runs::register_job(job(
			"cancelled-agent",
			TapJobStatus::Cancelled,
			TapLiveState::default(),
		));

		let CommandResult::HandledWithOutput(output) = handle_agents(&[]).unwrap() else {
			panic!("expected agents output");
		};
		let CommandOutput::Agents {
			running,
			finished,
			total,
			..
		} = *output
		else {
			panic!("expected agents list");
		};
		assert_eq!(total, 4);
		assert_eq!(running.len(), 1);
		assert_eq!(finished.len(), 3);
		assert_eq!(running[0]["last_action"], "shell cargo test");
		assert_eq!(running[0]["tokens_input"], 100);

		let CommandResult::HandledWithOutput(output) = handle_agents(&["running-agent"]).unwrap()
		else {
			panic!("expected detail output");
		};
		let CommandOutput::Agents { detail, total, .. } = *output else {
			panic!("expected detail card");
		};
		let detail = detail.expect("detail");
		assert_eq!(total, 1);
		assert_eq!(detail["status"], "running");
		assert_eq!(detail["tokens_cached"], 50);
		assert_eq!(detail["cost"], 0.25);

		let stats = get_agents_stats().expect("stats");
		assert_eq!(stats["total"], 4);
		assert_eq!(stats["running"], 1);
		assert_eq!(stats["done"], 1);
		assert_eq!(stats["failed"], 1);

		crate::session::tap_runs::clear_for_session(&session_id);
	})
	.await;
}
