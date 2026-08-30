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

fn call(params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: "recall".to_string(),
		parameters: params,
		tool_id: "t1".to_string(),
	}
}

#[tokio::test]
async fn rejects_missing_empty_and_oversized_id_lists() {
	assert!(execute_recall(&call(json!({}))).await.is_err());
	assert!(execute_recall(&call(json!({ "ids": [] }))).await.is_err());
	assert!(
		execute_recall(&call(json!({ "ids": ["b:1", "b:2", "b:3"] })))
			.await
			.is_err(),
		"per-call block bound must be enforced"
	);
}
