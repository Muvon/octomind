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
use crate::session::chat::test_support;

/// Anthropic's ping interval for the 5m conversation cache (0.9 × TTL).
const FIVE_MINUTE_INTERVAL: Duration = Duration::from_secs(270);

fn message(role: &str, content: &str, cached: bool, cache_ttl: Option<&str>) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		cached,
		cache_ttl: cache_ttl.map(str::to_string),
		..Message::default()
	}
}

fn user_message(cached: bool) -> Message {
	message("user", "hello", cached, None)
}

/// Rewrite penalty of a prefix that is all tail, as on Opus/Sonnet 5.5.
const FULL_TAIL_PENALTY: f64 = 24.0;

fn keepalive_config() -> Config {
	let mut config = test_support::fake_provider_config();
	config.cache_keepalive_enabled = true;
	config
}

fn session(model: &str, messages: Vec<Message>) -> ChatSession {
	let mut session = ChatSession::for_tests(messages);
	session.model = model.to_string();
	session
}

fn minutes(n: u64) -> Duration {
	Duration::from_secs(n * 60)
}

// --- pricing bound ---------------------------------------------------------

#[test]
fn penalty_follows_the_price_ratio() {
	// Opus 5.5: write $5, read $0.20 → (5 − 0.2) / 0.2 = 24 — a float that
	// lands just below 24, so the bound must still floor to 24.
	assert_eq!(max_pings(rewrite_penalty_in_pings(5.00, 0.20, 1.0)), 24);
	// Fable 5.1: write $12.50, read $0.25 → 49.
	assert_eq!(max_pings(rewrite_penalty_in_pings(12.50, 0.25, 1.0)), 49);
	// A 0.1× read model (Opus 5): write $6.25, read $0.50 → 11.5 → 11.
	assert_eq!(max_pings(rewrite_penalty_in_pings(6.25, 0.50, 1.0)), 11);
}

#[test]
fn penalty_scales_with_the_expiring_share() {
	// Only a quarter of the prefix would be re-written: 24 × 0.25 = 6.
	assert_eq!(max_pings(rewrite_penalty_in_pings(5.00, 0.20, 0.25)), 6);
}

#[test]
fn penalty_is_zero_without_a_cache_discount() {
	assert_eq!(
		rewrite_penalty_in_pings(5.00, 0.0, 1.0),
		0.0,
		"free reads have no meaningful bound"
	);
	assert_eq!(
		rewrite_penalty_in_pings(2.00, 2.00, 1.0),
		0.0,
		"no write premium: never ping"
	);
}

#[test]
fn tail_share_excludes_the_long_cached_system_prompt() {
	let system = message("system", &"rules ".repeat(2_000), true, None);
	let snapshot = [system, message("user", "hello", true, None)];
	let share = tail_share(&snapshot, None, "anthropic:claude-sonnet-5-5");
	assert!(
		share > 0.0 && share < 0.1,
		"the short tail is a small share, got {}",
		share
	);
}

#[test]
fn tail_share_starts_after_the_last_long_marker() {
	let system = message("system", "rules", true, None);
	let summary = message("user", &"summary ".repeat(2_000), true, Some("1h"));
	let snapshot = [system, summary, message("user", "hello", true, None)];
	let share = tail_share(&snapshot, None, "anthropic:claude-sonnet-5-5");
	assert!(share < 0.1, "the 1h summary stays warm on its own, got {}", share);
}

#[test]
fn tail_share_is_zero_when_the_newest_marker_is_the_system_prompt() {
	let snapshot = [message("system", "rules", true, None)];
	assert_eq!(tail_share(&snapshot, None, "anthropic:claude-sonnet-5-5"), 0.0);
}

#[test]
fn pings_to_bridge_counts_whole_intervals() {
	assert_eq!(pings_to_bridge(minutes(20), FIVE_MINUTE_INTERVAL), 4);
	assert_eq!(
		pings_to_bridge(Duration::from_secs(200), FIVE_MINUTE_INTERVAL),
		0,
		"a wait shorter than one interval needs no ping"
	);
}

// --- budget ----------------------------------------------------------------

/// Waits oldest first: (minutes, answered).
fn waits(entries: &[(u64, bool)]) -> VecDeque<Wait> {
	entries
		.iter()
		.map(|&(length, answered)| Wait {
			length: minutes(length),
			answered,
		})
		.collect()
}

fn budget(history: &[(u64, bool)]) -> u32 {
	ping_budget(&waits(history), None, FIVE_MINUTE_INTERVAL, FULL_TAIL_PENALTY)
}

#[test]
fn budget_without_history_is_the_ski_rental_bound() {
	assert_eq!(budget(&[]), 24);
}

#[test]
fn budget_bridges_an_affordable_known_wake_exactly() {
	let history = waits(&[(200, true)]);
	assert_eq!(
		ping_budget(&history, Some(minutes(20)), FIVE_MINUTE_INTERVAL, FULL_TAIL_PENALTY),
		4
	);
}

#[test]
fn budget_ignores_a_known_wake_beyond_the_bound() {
	// 3h needs 40 pings > 24: the wake is not worth bridging, so the budget
	// falls back to the history (none yet: the bound).
	assert_eq!(
		ping_budget(
			&VecDeque::new(),
			Some(minutes(180)),
			FIVE_MINUTE_INTERVAL,
			FULL_TAIL_PENALTY
		),
		24
	);
}

#[test]
fn budget_is_zero_when_answers_always_outlast_the_bound() {
	assert_eq!(
		budget(&[(180, true), (240, true)]),
		0,
		"pinging only adds cost when the cache expires anyway"
	);
}

#[test]
fn budget_is_zero_after_waits_the_user_walked_away_from() {
	// Cut-short waits are evidence too: the answer took at least that long.
	assert_eq!(budget(&[(120, false), (120, false), (120, false)]), 0);
}

#[test]
fn budget_covers_the_waits_the_session_keeps_having() {
	assert!(budget(&[(20, true), (20, true), (20, true)]) >= 4);
}

#[test]
fn budget_follows_the_recent_rhythm_over_an_old_one() {
	let slow = [(180, true); 10];
	assert_eq!(budget(&slow), 0, "a slow rhythm alone never pays");

	let mut drifted = [(180, true); 10].to_vec();
	drifted[6..].fill((20, true));
	assert_eq!(
		budget(&drifted),
		4,
		"recent 20-minute replies outweigh older 3-hour ones"
	);
}

#[test]
fn hazards_without_history_read_as_a_coin_flip() {
	let hazards = interval_hazards(&VecDeque::new(), FIVE_MINUTE_INTERVAL, 3);
	assert_eq!(hazards, vec![0.5; 4]);
}

#[test]
fn a_cut_short_wait_lowers_the_hazard_of_the_intervals_it_reached() {
	let hazards = interval_hazards(&waits(&[(60, false)]), FIVE_MINUTE_INTERVAL, 3);
	assert!(hazards.iter().all(|h| *h < 0.5), "got {:?}", hazards);
}

#[test]
fn wait_history_keeps_kinds_apart() {
	let mut history = WaitHistory::default();
	history.record(WaitKind::Reply, minutes(10), true);
	history.record(WaitKind::Background, minutes(1), true);
	history.record(WaitKind::Tools, minutes(3), false);
	assert_eq!(history.samples(WaitKind::Reply), &waits(&[(10, true)]));
	assert_eq!(history.samples(WaitKind::Background), &waits(&[(1, true)]));
	assert_eq!(history.samples(WaitKind::Tools), &waits(&[(3, false)]));
}

#[test]
fn wait_history_keeps_only_the_recent_waits() {
	let mut history = WaitHistory::default();
	for n in 0..(MAX_WAITS as u64 + 8) {
		history.record(WaitKind::Reply, minutes(n), true);
	}
	let kept = history.samples(WaitKind::Reply);
	assert_eq!(kept.len(), MAX_WAITS);
	assert_eq!(kept.front().map(|w| w.length), Some(minutes(8)), "the oldest go first");
}

// --- spawn -----------------------------------------------------------------

#[tokio::test]
async fn spawn_returns_none_when_disabled() {
	let mut config = keepalive_config();
	config.cache_keepalive_enabled = false;
	let chat = session("anthropic:claude-sonnet-5-5", vec![user_message(true)]);
	assert!(KeepaliveHandle::spawn(&chat, &config, WaitKind::Reply).is_none());
}

#[tokio::test]
async fn spawn_returns_none_when_no_message_is_cached() {
	let chat = session("anthropic:claude-sonnet-5-5", vec![user_message(false)]);
	assert!(KeepaliveHandle::spawn(&chat, &keepalive_config(), WaitKind::Reply).is_none());
}

#[tokio::test]
async fn spawn_returns_none_for_a_structured_output_session() {
	// The API rejects a pre-warm that carries an output schema.
	let mut chat = session("anthropic:claude-sonnet-5-5", vec![user_message(true)]);
	chat.schema = Some(serde_json::json!({"type": "object"}));
	assert!(KeepaliveHandle::spawn(&chat, &keepalive_config(), WaitKind::Reply).is_none());
}

#[tokio::test]
async fn spawn_returns_none_for_unparseable_model() {
	let chat = session("no-provider-prefix", vec![user_message(true)]);
	assert!(KeepaliveHandle::spawn(&chat, &keepalive_config(), WaitKind::Reply).is_none());
}

#[tokio::test]
async fn spawn_returns_none_for_provider_without_keepalive_policy() {
	// OpenAI manages its prompt cache server-side; pinging it would burn
	// money for nothing.
	let chat = session("openai:gpt-5", vec![user_message(true)]);
	assert!(KeepaliveHandle::spawn(&chat, &keepalive_config(), WaitKind::Reply).is_none());
}

#[tokio::test]
async fn a_reply_wait_with_work_in_flight_is_a_background_wait() {
	use crate::session::tap_runs::{self, TapJob, TapJobStatus, TapLiveState};
	use std::sync::{Arc, RwLock};

	let id = "keepalive-background".to_string();
	crate::session::context::with_session_id(id.clone(), async {
		tap_runs::init_for_session();
		let (cancel_tx, _keep_alive) = watch::channel(false);
		tap_runs::register_job(TapJob {
			id: "keepalive-background-run".to_string(),
			role: "developer:general".to_string(),
			workdir: "/tmp".to_string(),
			started_at: std::time::SystemTime::UNIX_EPOCH,
			error: None,
			status: Arc::new(RwLock::new(TapJobStatus::Running)),
			cancel_tx,
			live: Arc::new(RwLock::new(TapLiveState::default())),
		});

		let config = keepalive_config();
		let mut chat = session("anthropic:claude-sonnet-5-5", vec![user_message(true)]);
		let handle = KeepaliveHandle::spawn(&chat, &config, WaitKind::Reply).expect("cached Anthropic session spawns");
		handle.finish(&mut chat, true).await;

		assert_eq!(
			chat.wait_history.samples(WaitKind::Background).len(),
			1,
			"a running tap is a result that will come back"
		);
		assert!(chat.wait_history.samples(WaitKind::Reply).is_empty());
	})
	.await;
	crate::session::context::cleanup_session(&id);
}

#[tokio::test]
async fn finish_records_how_the_wait_ended() {
	let config = keepalive_config();
	let mut chat = session("anthropic:claude-sonnet-5-5", vec![user_message(true)]);

	let handle = KeepaliveHandle::spawn(&chat, &config, WaitKind::Tools).expect("cached Anthropic session spawns");
	// The first ping is one interval away, so finishing now sends nothing.
	assert_eq!(handle.finish(&mut chat, true).await, 0);
	let handle = KeepaliveHandle::spawn(&chat, &config, WaitKind::Tools).expect("spawns again");
	handle.finish(&mut chat, false).await;

	let recorded: Vec<bool> = chat
		.wait_history
		.samples(WaitKind::Tools)
		.iter()
		.map(|w| w.answered)
		.collect();
	assert_eq!(
		recorded,
		vec![true, false],
		"answered and cut-short waits both feed the budget"
	);
}

fn ping_exchange(cost: Option<f64>) -> ProviderExchange {
	ProviderExchange {
		request: serde_json::Value::Null,
		response: serde_json::Value::Null,
		timestamp: 0,
		usage: Some(crate::providers::TokenUsage {
			input_tokens: 4,
			cache_read_tokens: 20_000,
			cache_write_tokens: 0,
			output_tokens: 0,
			reasoning_tokens: 0,
			total_tokens: 20_004,
			cost,
			request_time_ms: Some(300),
		}),
		provider: "anthropic".to_string(),
		rate_limit_headers: None,
	}
}

#[test]
fn pings_bill_cost_without_touching_turn_counters() {
	let mut info = session("anthropic:claude-sonnet-5-5", Vec::new()).session.info;
	let before = info.clone();

	bill_pings(&mut info, &[ping_exchange(Some(0.002)), ping_exchange(Some(0.002))]);

	assert!(
		(info.total_cost - before.total_cost - 0.004).abs() < 1e-12,
		"pings are part of the bill"
	);
	// Compression paces itself and sizes context from these: a ping is not a
	// conversation request.
	assert_eq!(info.total_api_calls, before.total_api_calls);
	assert_eq!(info.current_total_tokens, before.current_total_tokens);
	assert_eq!(info.cache_read_tokens, before.cache_read_tokens);
	assert_eq!(info.total_api_time_ms, before.total_api_time_ms);
}

#[test]
fn a_ping_without_reported_cost_marks_the_main_cost_unknown() {
	let mut info = session("anthropic:claude-sonnet-5-5", Vec::new()).session.info;
	bill_pings(&mut info, &[ping_exchange(None)]);
	assert_eq!(info.main_cost, None, "an unpriced ping must not pass for free");
}

// --- run -------------------------------------------------------------------

fn profile(model: &str) -> ModelProfile {
	session(model, Vec::new()).model_profile(&keepalive_config())
}

#[tokio::test]
async fn run_sends_nothing_with_a_zero_budget() {
	let (_tx, rx) = watch::channel(false);
	let exchanges = run(
		vec![user_message(true)],
		profile("anthropic:claude-sonnet-5-5"),
		keepalive_config(),
		Duration::from_millis(1),
		0,
		rx,
	)
	.await;
	assert!(exchanges.is_empty());
}

#[tokio::test]
async fn run_stops_on_cancel_during_interval() {
	let (tx, rx) = watch::channel(false);
	let task = tokio::spawn(run(
		vec![user_message(true)],
		profile("anthropic:claude-sonnet-5-5"),
		keepalive_config(),
		FIVE_MINUTE_INTERVAL,
		24,
		rx,
	));
	tokio::time::sleep(Duration::from_millis(25)).await;
	tx.send(true).expect("cancel signal sent");
	let exchanges = task.await.expect("keepalive task finished cleanly");
	assert!(exchanges.is_empty());
}

#[tokio::test]
async fn run_stops_when_cancel_sender_drops() {
	let (tx, rx) = watch::channel(false);
	drop(tx);
	let exchanges = run(
		vec![user_message(true)],
		profile("anthropic:claude-sonnet-5-5"),
		keepalive_config(),
		FIVE_MINUTE_INTERVAL,
		24,
		rx,
	)
	.await;
	assert!(exchanges.is_empty());
}

#[tokio::test]
async fn run_spends_at_most_its_budget_on_failing_pings() {
	// Unparseable model: every ping fails before any network call, so only
	// the budget can end the loop.
	let (_tx, rx) = watch::channel(false);
	let exchanges = run(
		vec![user_message(true)],
		profile("no-provider-prefix"),
		keepalive_config(),
		Duration::from_millis(5),
		3,
		rx,
	)
	.await;
	assert!(exchanges.is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn run_stops_after_a_ping_that_misses_the_cache() {
	let _env = test_support::ENV_LOCK.lock().await;
	let url = test_support::spawn_stub(vec![
		test_support::final_response("ok"),
		test_support::final_response("ok"),
	])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	// The stub reports no cache read: the first ping re-wrote the prefix, so
	// the keepalive must stop instead of paying that every interval.
	let (_tx, rx) = watch::channel(false);
	let exchanges = run(
		vec![user_message(true)],
		profile("ollama:fake-model"),
		keepalive_config(),
		Duration::from_millis(10),
		5,
		rx,
	)
	.await;

	std::env::remove_var("OLLAMA_API_URL");

	assert_eq!(
		exchanges.len(),
		1,
		"the missed ping is billed once, then the loop stops"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn run_sends_real_zero_output_pings_through_the_anthropic_provider() {
	let _lock = test_support::ENV_LOCK.lock().await;
	let _env = test_support::EnvGuard::new(&["ANTHROPIC_API_URL", "ANTHROPIC_API_KEY"]);
	// A pre-warm answers with no content and the whole prefix read from cache.
	let cache_hit = serde_json::json!({
		"id": "msg_1",
		"type": "message",
		"role": "assistant",
		"model": "claude-sonnet-5-5",
		"content": [],
		"stop_reason": "max_tokens",
		"stop_sequence": null,
		"usage": {
			"input_tokens": 4,
			"cache_read_input_tokens": 9000,
			"cache_creation_input_tokens": 0,
			"output_tokens": 0
		}
	});
	let (url, captured) = test_support::spawn_recording_stub(vec![cache_hit.clone(), cache_hit]).await;
	// The stub ignores the path; the provider wants the full messages endpoint.
	std::env::set_var("ANTHROPIC_API_URL", url.replace("/v1/chat/completions", "/v1/messages"));
	std::env::set_var("ANTHROPIC_API_KEY", "test-key");

	let snapshot = vec![
		message("system", "rules", true, None),
		message("user", "cached question", true, None),
	];
	let (_tx, rx) = watch::channel(false);
	let exchanges = run(
		snapshot,
		profile("anthropic:claude-sonnet-5-5"),
		keepalive_config(),
		Duration::from_millis(10),
		2,
		rx,
	)
	.await;

	assert_eq!(exchanges.len(), 2, "a hit does not stop the loop: both pings go out");
	for exchange in &exchanges {
		let usage = exchange.usage.as_ref().expect("ping usage");
		assert!(usage.cache_read_tokens > 0, "the ping read the cached prefix");
	}

	let requests = captured.lock().expect("captured requests").clone();
	assert_eq!(requests.len(), 2);
	for body in &requests {
		assert_eq!(body["max_tokens"], 0, "a pre-warm generates nothing");
		assert_eq!(body["stream"], false, "the API rejects max_tokens 0 with streaming");
		let messages = body["messages"].as_array().expect("messages array");
		let last = messages.last().expect("at least the cached user message");
		assert_eq!(last["role"], "user", "nothing follows the last cached message");
		let last_block = last["content"]
			.as_array()
			.and_then(|blocks| blocks.last())
			.expect("content block");
		assert_eq!(last_block["text"], "cached question");
		assert!(
			last_block.get("cache_control").is_some(),
			"the marker rides the last block"
		);
	}
}
