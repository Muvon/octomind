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

//! Gate-path tests for the compression entry points: the cheap early-return
//! decisions that run on EVERY tool round must never trigger work (or LLM
//! calls) when compression is disabled or the session is small.

use super::*;
use crate::session::chat::session::ChatSession;

fn config_with_threshold(threshold: usize) -> crate::config::Config {
	let mut config: crate::config::Config = toml::from_str(include_str!("../../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config.compression.threshold = threshold;
	config
}

fn cancel_rx() -> tokio::sync::watch::Receiver<bool> {
	tokio::sync::watch::channel(false).1
}

#[tokio::test]
async fn test_disabled_threshold_never_compresses() {
	let config = config_with_threshold(0);
	let mut session = ChatSession::for_tests(Vec::new());
	session.add_user_message("hello").expect("user");
	let before = session.session.messages.len();

	check_and_compress_conversation(&mut session, &config, cancel_rx(), CompressionTrigger::Automatic)
		.await
		.expect("disabled compression is a no-op");
	assert_eq!(session.session.messages.len(), before);
}

#[tokio::test]
async fn test_small_session_below_threshold_is_untouched() {
	// Enormous threshold: a tiny session must never be considered
	let config = config_with_threshold(usize::MAX / 2);
	let mut session = ChatSession::for_tests(Vec::new());
	session.add_user_message("short question").expect("user");
	let before = session.session.messages.len();

	check_and_compress_conversation(&mut session, &config, cancel_rx(), CompressionTrigger::Automatic)
		.await
		.expect("below-threshold compression is a no-op");
	assert_eq!(session.session.messages.len(), before);
}

#[tokio::test]
async fn test_ceiling_check_passes_small_sessions() {
	let config = config_with_threshold(0);
	let mut session = ChatSession::for_tests(Vec::new());
	session.add_user_message("tiny").expect("user");

	ensure_context_within_ceiling(&mut session, &config)
		.await
		.expect("small session is within any ceiling");
}

const IDLE_NOW: u64 = 10_000;
const CACHE_LIFETIME_SECS: u64 = 300;

/// A caching-model session whose cache expired long before `IDLE_NOW`, so
/// only the gate under test can stop an idle compaction.
fn expired_cache_session() -> ChatSession {
	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "anthropic:claude-sonnet-5-5".to_string();
	session.add_user_message("short question").expect("user");
	session.session.info.last_cache_touch = Some(IDLE_NOW - 10 * CACHE_LIFETIME_SECS);
	session.session.info.cache_lifetime_secs = Some(CACHE_LIFETIME_SECS);
	session.session.info.cache_model = Some(session.model.clone());
	session
}

async fn idle_compaction(session: &mut ChatSession, config: &crate::config::Config) -> bool {
	compact_if_cache_expired(session, config, IDLE_NOW, cancel_rx())
		.await
		.expect("an uncancelled idle compaction never errors")
}

#[tokio::test]
async fn idle_compaction_is_off_when_the_config_disables_it() {
	let mut config = config_with_threshold(1);
	config.compression.idle = false;
	let mut session = expired_cache_session();
	let before = session.session.messages.len();

	assert!(!idle_compaction(&mut session, &config).await);
	assert_eq!(session.session.messages.len(), before);
}

#[tokio::test]
async fn idle_compaction_waits_until_the_gap_reaches_the_lifetime() {
	let config = config_with_threshold(1);
	let mut session = expired_cache_session();
	session.session.info.last_cache_touch = Some(IDLE_NOW - CACHE_LIFETIME_SECS + 1);
	let before = session.session.messages.len();

	assert!(!idle_compaction(&mut session, &config).await);
	assert_eq!(session.session.messages.len(), before);
}

#[tokio::test]
async fn idle_compaction_never_runs_on_an_unknown_lifetime() {
	let config = config_with_threshold(1);
	let mut session = expired_cache_session();
	session.session.info.cache_lifetime_secs = None;
	let before = session.session.messages.len();

	assert!(!idle_compaction(&mut session, &config).await);
	assert_eq!(session.session.messages.len(), before);
}

#[tokio::test]
async fn idle_compaction_never_runs_on_an_unknown_last_touch() {
	let config = config_with_threshold(1);
	let mut session = expired_cache_session();
	session.session.info.last_cache_touch = None;
	let before = session.session.messages.len();

	assert!(!idle_compaction(&mut session, &config).await);
	assert_eq!(session.session.messages.len(), before);
}

#[tokio::test]
async fn idle_compaction_never_runs_when_the_cache_owner_is_unknown() {
	let config = config_with_threshold(1);
	let mut session = expired_cache_session();
	session.session.info.cache_model = None;
	let before = session.session.messages.len();

	assert!(!idle_compaction(&mut session, &config).await);
	assert_eq!(session.session.messages.len(), before);
}
