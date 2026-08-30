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

fn cached_user_message() -> Message {
	Message {
		role: "user".to_string(),
		content: "hello".to_string(),
		cached: true,
		..Message::default()
	}
}

fn uncached_user_message() -> Message {
	Message {
		role: "user".to_string(),
		content: "hello".to_string(),
		cached: false,
		..Message::default()
	}
}

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

#[tokio::test]
async fn spawn_returns_none_when_disabled() {
	let handle = KeepaliveHandle::spawn(
		vec![cached_user_message()],
		"anthropic:claude-sonnet-4-6".to_string(),
		template_config(),
		false,
		Duration::from_secs(60),
	);
	assert!(handle.is_none());
}

#[tokio::test]
async fn spawn_returns_none_with_empty_snapshot() {
	let handle = KeepaliveHandle::spawn(
		Vec::new(),
		"anthropic:claude-sonnet-4-6".to_string(),
		template_config(),
		true,
		Duration::from_secs(60),
	);
	assert!(handle.is_none());
}

#[tokio::test]
async fn spawn_returns_none_when_no_message_is_cached() {
	let handle = KeepaliveHandle::spawn(
		vec![uncached_user_message()],
		"anthropic:claude-sonnet-4-6".to_string(),
		template_config(),
		true,
		Duration::from_secs(60),
	);
	assert!(handle.is_none());
}

#[tokio::test]
async fn spawn_returns_none_for_unparseable_model() {
	let handle = KeepaliveHandle::spawn(
		vec![cached_user_message()],
		"no-provider-prefix".to_string(),
		template_config(),
		true,
		Duration::from_secs(60),
	);
	assert!(handle.is_none());
}

#[tokio::test]
async fn spawn_returns_none_for_unsupported_model() {
	let handle = KeepaliveHandle::spawn(
		vec![cached_user_message()],
		"anthropic:claude-not-a-real-model".to_string(),
		template_config(),
		true,
		Duration::from_secs(60),
	);
	assert!(handle.is_none());
}

#[tokio::test]
async fn spawn_returns_none_for_provider_without_keepalive_policy() {
	// OpenAI manages its prompt cache server-side; pinging it would burn
	// money for nothing, so spawn must refuse.
	let handle = KeepaliveHandle::spawn(
		vec![cached_user_message()],
		"openai:gpt-5".to_string(),
		template_config(),
		true,
		Duration::from_secs(60),
	);
	assert!(handle.is_none());
}

#[tokio::test]
async fn spawned_handle_cancels_before_any_ping() {
	let handle = KeepaliveHandle::spawn(
		vec![cached_user_message()],
		"anthropic:claude-sonnet-4-6".to_string(),
		template_config(),
		true,
		Duration::from_secs(600),
	)
	.expect("cached snapshot + Anthropic model must spawn");
	// Anthropic's policy interval is 54m, so cancel lands mid-sleep and
	// no ping (network call) ever fires.
	let exchanges = handle.cancel().await;
	assert!(exchanges.is_empty());
}

#[tokio::test]
async fn run_stops_when_max_idle_elapses() {
	// Unparseable model: every ping fails fast at parse time (no network),
	// so only the max_idle guard can end the loop.
	let (_tx, rx) = watch::channel(false);
	let exchanges = run(
		vec![cached_user_message()],
		"no-provider-prefix".to_string(),
		template_config(),
		Duration::from_millis(10),
		Duration::from_millis(80),
		rx,
	)
	.await;
	assert!(exchanges.is_empty());
}

#[tokio::test]
async fn run_stops_on_cancel_during_interval() {
	let (tx, rx) = watch::channel(false);
	let task = tokio::spawn(run(
		vec![cached_user_message()],
		"anthropic:claude-sonnet-4-6".to_string(),
		template_config(),
		Duration::from_secs(300),
		Duration::ZERO,
		rx,
	));
	tokio::time::sleep(Duration::from_millis(25)).await;
	tx.send(true).expect("cancel signal sent");
	let exchanges = task.await.expect("keepalive task finished cleanly");
	assert!(exchanges.is_empty());
}

#[tokio::test]
async fn run_stops_immediately_when_cancel_pre_signalled() {
	let (tx, rx) = watch::channel(false);
	tx.send(true).expect("cancel pre-signalled");
	let exchanges = run(
		vec![cached_user_message()],
		"anthropic:claude-sonnet-4-6".to_string(),
		template_config(),
		Duration::from_secs(300),
		Duration::from_secs(300),
		rx,
	)
	.await;
	assert!(exchanges.is_empty());
}

#[tokio::test]
async fn run_stops_when_cancel_sender_drops() {
	let (tx, rx) = watch::channel(false);
	drop(tx);
	let exchanges = run(
		vec![cached_user_message()],
		"anthropic:claude-sonnet-4-6".to_string(),
		template_config(),
		Duration::from_secs(300),
		Duration::ZERO,
		rx,
	)
	.await;
	assert!(exchanges.is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn run_harvests_successful_ping_exchange() {
	use crate::session::chat::test_support;

	let _env = test_support::ENV_LOCK.lock().await;
	let url = test_support::spawn_stub(vec![test_support::final_response("ok")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let (tx, rx) = watch::channel(false);
	let task = tokio::spawn(run(
		vec![cached_user_message()],
		"ollama:fake-model".to_string(),
		test_support::fake_provider_config(),
		Duration::from_millis(50),
		Duration::from_secs(1), // backstop if the cancel signal is missed
		rx,
	));
	// Several pings at 50ms intervals hit the local stub before the
	// cancel at 300ms; each scripted response becomes one exchange.
	tokio::time::sleep(Duration::from_millis(300)).await;
	tx.send(true).expect("cancel signal sent");
	let exchanges = task.await.expect("keepalive task finished cleanly");

	std::env::remove_var("OLLAMA_API_URL");

	assert!(
		!exchanges.is_empty(),
		"at least one scripted ping must be harvested"
	);
}
