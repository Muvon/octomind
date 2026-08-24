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

//! The stats sink is a process global shared with every other test in the
//! binary, so assertions are presence/monotonicity, never exact totals.

use super::*;

#[test]
fn test_recorders_and_snapshot_render() {
	record_call(CallKind::Gate, 100, 50, 120, 0.001);
	record_call(CallKind::Recall, 10, 5, 15, 0.0);
	gate_run();
	gate_pass();
	gate_fail();
	steer(crate::supervisor::detect::DetectorSignal::None);
	pregate_block();
	lessons(2);
	orientation(1);
	recall();
	condensed(3, 1200);

	let snapshot = snapshot().expect("non-idle stats render a snapshot");
	let text = snapshot.to_string();
	// The snapshot is consumed by /info and telemetry — the load-bearing
	// counters must be present as fields.
	for key in ["calls", "gate", "lessons", "recalls", "condense"] {
		assert!(text.contains(key), "snapshot missing '{key}': {text}");
	}
}
