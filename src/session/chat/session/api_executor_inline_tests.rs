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
use serial_test::serial;

#[serial]
#[tokio::test]
async fn a_pack_that_does_not_fit_keeps_the_plugin_blocks_marked_keep_and_tells_the_plugin_the_rest() {
	use crate::plugins::protocol::event;
	use crate::plugins::test_support::{connect_stub, StubPlugin};

	let id = "o11-pack-ceiling";
	let stub = StubPlugin::new(&[event::REQUEST_BEFORE]).answer(serde_json::json!({"context": [
		{"name": "small", "text": "keep me", "keep": true},
		{"name": "big", "text": "word ".repeat(4000)},
	]}));
	let mut config = crate::session::chat::test_support::fake_provider_config();
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.name = id.to_string();
	session.model = "notaprovider:no-such-model".to_string();
	connect_stub(id, "assistant", std::path::Path::new("."), &stub).await;
	session
		.send_request_before(&config, RequestPhase::TurnStart)
		.await
		.expect("the plugin answers");

	// Room for the small block and nothing like the big one.
	let baseline = session.get_full_context_tokens(&config).await;
	config.max_session_tokens_threshold = baseline + 600;
	session.set_active_memory_pack(Some(format!("keep me{}", "word ".repeat(4000))));

	make_room_for_memory(&mut session, &config).await;

	assert_eq!(session.active_memory_pack.as_deref(), Some("keep me"));
	session
		.send_request_before(&config, RequestPhase::FollowUp)
		.await
		.expect("the plugin answers");
	assert_eq!(
		stub.sent(event::REQUEST_BEFORE)[1]["dropped_context"],
		serde_json::json!(["big"])
	);

	crate::plugins::end_session(id);
}
