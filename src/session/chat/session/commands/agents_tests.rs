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
