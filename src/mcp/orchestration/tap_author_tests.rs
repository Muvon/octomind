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
use crate::session::context::{with_session_id, SessionMode};

fn call(params: Value) -> McpToolCall {
	McpToolCall {
		tool_name: "tap".into(),
		tool_id: "test".into(),
		parameters: params,
	}
}

#[tokio::test]
async fn creation_rejects_unknown_fields_and_copies_before_writing_any_definition() {
	let config = toml::from_str(include_str!("../../../config-templates/default.toml")).unwrap();
	let id = "author-denies-invalid".to_owned();
	with_session_id(id.clone(), async {
		context::init_session_services("local", SessionMode::default());
		let params = json!({"action":"create","name":"test","description":"test","system":"hi","command":"no"});
		assert!(author(&call(params.clone()), &config)
			.await
			.unwrap_err()
			.to_string()
			.contains("invalid tap create"));
		context::set_session_mode(
			&id,
			SessionMode {
				copy: true,
				readonly: false,
			},
		);
		assert!(author(&call(params), &config)
			.await
			.unwrap_err()
			.to_string()
			.contains("copy"));
		context::cleanup_session(&id);
	})
	.await;
}
