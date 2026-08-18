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

fn response_with_finish(finish_reason: Option<&str>) -> crate::providers::ProviderResponse {
	crate::providers::ProviderResponse {
		content: String::new(),
		exchange: crate::providers::ProviderExchange::new(
			serde_json::json!({}),
			serde_json::json!({}),
			None,
			"test",
		),
		tool_calls: None,
		thinking: None,
		finish_reason: finish_reason.map(str::to_string),
		response_id: None,
		structured_output: None,
	}
}

#[test]
fn test_check_should_continue() {
	let config =
		toml::from_str::<Config>(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");

	// Tool-call finish reasons continue regardless of has_more_tools
	for reason in ["tool_calls", "tool_use"] {
		assert!(check_should_continue(
			&response_with_finish(Some(reason)),
			&config,
			false
		));
	}

	// Terminal finish reasons stop even with tools pending
	for reason in ["stop", "length", "end_turn"] {
		assert!(!check_should_continue(
			&response_with_finish(Some(reason)),
			&config,
			true
		));
	}

	// Unknown finish reason is conservative: continue
	assert!(check_should_continue(
		&response_with_finish(Some("weird_reason")),
		&config,
		false
	));

	// No finish reason: fall back to whether tools are pending
	assert!(check_should_continue(
		&response_with_finish(None),
		&config,
		true
	));
	assert!(!check_should_continue(
		&response_with_finish(None),
		&config,
		false
	));
}
