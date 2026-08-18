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
use serde_json::json;

fn exchange_with_response(response: serde_json::Value) -> ProviderExchange {
	ProviderExchange::new(json!({}), response, None, "test")
}

#[test]
fn test_extract_original_tool_calls_unified_format() {
	let calls = json!([{"tool_name": "shell", "parameters": {"cmd": "ls"}, "tool_id": "id1"}]);
	let exchange = exchange_with_response(json!({ "tool_calls": calls }));
	assert_eq!(
		MessageHandler::extract_original_tool_calls(&exchange),
		Some(calls)
	);
}

#[test]
fn test_extract_original_tool_calls_absent() {
	let exchange = exchange_with_response(json!({"content": "plain answer"}));
	assert!(MessageHandler::extract_original_tool_calls(&exchange).is_none());
}
