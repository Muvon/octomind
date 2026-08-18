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
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
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

	check_and_compress_conversation(
		&mut session,
		&config,
		cancel_rx(),
		CompressionTrigger::Automatic,
	)
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

	check_and_compress_conversation(
		&mut session,
		&config,
		cancel_rx(),
		CompressionTrigger::Automatic,
	)
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
