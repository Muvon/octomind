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
use crate::session::Message;

/// Build a minimal ChatSession with the given messages for testing compression primitives.
/// Delegates to the crate-shared test constructor (see `ChatSession::for_tests`).
fn make_session(messages: Vec<Message>) -> ChatSession {
	ChatSession::for_tests(messages)
}

fn msg(role: &str, cached: bool) -> Message {
	Message {
		role: role.to_string(),
		cached,
		..Default::default()
	}
}

#[test]
fn real_user_follow_up_does_not_clear_analysis_findings() {
	let mut session = make_session(Vec::new());
	session.analysis_findings = vec!["load-bearing root cause".to_string()];

	session.add_user_message("continue").unwrap();

	assert_eq!(
		session.analysis_findings,
		vec!["load-bearing root cause"],
		"task continuity is resolved later; message insertion must not destroy findings"
	);
}

#[test]
fn active_memory_pack_is_single_runtime_message_and_clears_on_new_turn() {
	let mut session = make_session(vec![msg("system", false)]);
	session.set_active_memory_pack(Some(
		"<active_memory_pack trust=\"test\">first</active_memory_pack>".to_string(),
	));
	session.set_active_memory_pack(Some(
		"<active_memory_pack trust=\"test\">second</active_memory_pack>".to_string(),
	));
	assert_eq!(session.session.messages.len(), 1);
	session.ensure_active_memory_pack_message();
	session.ensure_active_memory_pack_message();
	let packs: Vec<_> = session
		.session
		.messages
		.iter()
		.filter(|message| message.name.as_deref() == Some("__active_memory_pack"))
		.collect();
	assert_eq!(packs.len(), 1);
	assert!(packs[0].content.contains("second"));
	session.remove_active_memory_pack_message();
	assert_eq!(session.session.messages.len(), 1);
	assert!(session.active_memory_pack.is_some());
	session.ensure_active_memory_pack_message();

	session.used_memory_ids.insert("M1".to_string());
	session.learning_outcome = crate::supervisor::learning::TrajectoryOutcome::Failed;
	session.recalled_refs.push((
		"M1".to_string(),
		"rule".to_string(),
		"role".to_string(),
		"project".to_string(),
	));
	session.add_user_message("new task").unwrap();
	assert!(session.active_memory_pack.is_none());
	assert!(session.used_memory_ids.is_empty());
	assert!(session.recalled_refs.is_empty());
	assert_eq!(
		session.learning_outcome,
		crate::supervisor::learning::TrajectoryOutcome::Unknown
	);
	assert!(!session
		.session
		.messages
		.iter()
		.any(|message| message.name.as_deref() == Some("__active_memory_pack")));
}

#[test]
fn learning_session_stats_separate_use_from_outcome_credit() {
	let mut stats = crate::session::LearningSessionStats::default();
	stats.record_pack(3, 420);
	stats.record_use(0.05);
	stats.record_use(-0.15);
	stats.record_use(0.0);
	assert_eq!(stats.packs, 1);
	assert_eq!(stats.items, 3);
	assert_eq!(stats.tokens, 420);
	assert_eq!(stats.used, 3);
	assert_eq!(stats.credit_positive, 1);
	assert_eq!(stats.credit_negative, 1);
	assert_eq!(stats.used_without_verdict, 1);
}

#[test]
fn system_managed_event_does_not_take_ownership_of_the_human_task() {
	let mut session = make_session(Vec::new());

	session
		.add_user_message("monitor the active operation")
		.unwrap();
	assert!(session.completion_gate_eligible);

	session
		.add_system_managed_turn_message("[monitor] still running")
		.unwrap();
	assert!(!session.completion_gate_eligible);

	// Notes injected within that response preserve its ownership. Otherwise a
	// recitation or supervisor hint could accidentally re-enable the old task.
	session
		.add_system_managed_user_message("<pay-attention>wait for the next event</pay-attention>")
		.unwrap();
	assert!(!session.completion_gate_eligible);

	session.add_user_message("new human task").unwrap();
	assert!(session.completion_gate_eligible);
}

/// Collect indices of all content-cached messages (user/assistant/tool with cached=true).
/// System markers are excluded — they are managed separately and never touched by compression.
fn content_cache_indices(session: &ChatSession) -> Vec<usize> {
	session
		.session
		.messages
		.iter()
		.enumerate()
		.filter(|(_, m)| m.cached && m.role != "system")
		.map(|(i, _)| i)
		.collect()
}

#[test]
fn real_user_turn_resets_steer_streak() {
	let mut cs = make_session(vec![msg("system", false)]);
	cs.steer_attempt = 2;
	cs.steer_last_signal = crate::supervisor::detect::DetectorSignal::Loop;

	cs.add_user_message("new task after prior loop stopped")
		.unwrap();

	assert_eq!(cs.steer_attempt, 0);
	assert_eq!(
		cs.steer_last_signal,
		crate::supervisor::detect::DetectorSignal::None
	);
}

#[test]
fn only_real_user_turn_resets_adaptive_compression_runway() {
	let mut cs = make_session(vec![msg("system", false)]);
	cs.session.info.consecutive_compressions = 3;
	cs.session.info.context_tokens_after_last_compression = 42_000;

	cs.add_system_managed_user_message("<system-note>continue</system-note>")
		.unwrap();
	assert_eq!(cs.session.info.consecutive_compressions, 3);

	cs.add_user_message("new human request").unwrap();
	assert_eq!(cs.session.info.consecutive_compressions, 0);
	assert_eq!(
		cs.session.info.context_tokens_after_last_compression, 42_000,
		"the exact post-compression watermark remains usable"
	);
}

// ── Case 1: no cache markers anywhere ────────────────────────────────────────
// Compressed block gets cached=true (marker #1) and the last eligible user/tool
// message in the preserved zone gets marker #2 automatically.
#[test]
fn case1_no_markers_compressed_block_gets_cached() {
	// idx: 0=system, 1=user(start), 2=assistant, 3=user, 4=assistant(end), 5..8=preserved
	let messages = vec![
		msg("system", false),
		msg("user", false), // start_idx=1 (kept)
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false), // end_idx=4
		msg("user", false),
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	let (_, had_cached) = cs.remove_messages_in_range(1, 4).unwrap();
	assert!(!had_cached);
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	let markers = content_cache_indices(&cs);
	// After drain+insert: [sys(0), user(1), COMP(2), user(3), asst(4), user(5), asst(6)]
	// Compressed block at idx 2 (marker #1) + last user at idx 5 (marker #2)
	assert_eq!(
		markers.len(),
		2,
		"must have 2 markers: compressed block + last eligible message"
	);
	assert!(markers.contains(&2), "compressed block must be cached");
	assert_eq!(*markers.last().unwrap(), 5, "marker #2 on last user");
}

// ── Case 2: one marker inside the range ──────────────────────────────────────
// Marker destroyed by drain. Compressed block gets marker #1, last eligible
// message gets marker #2 — always 2 markers after compression.
#[test]
fn case2_one_marker_inside_range_compressed_block_gets_cached() {
	// idx: 0=system, 1=user(start), 2=assistant, 3=user(cached!), 4=assistant(end), 5..8=preserved
	let messages = vec![
		msg("system", false),
		msg("user", false), // start_idx=1 (kept)
		msg("assistant", false),
		msg("user", true),       // marker #1 — inside range, will be removed
		msg("assistant", false), // end_idx=4
		msg("user", false),
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	let (_, had_cached) = cs.remove_messages_in_range(1, 4).unwrap();
	assert!(had_cached, "should detect the removed marker");
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	let markers = content_cache_indices(&cs);
	assert_eq!(
		markers.len(),
		2,
		"must have 2 markers: compressed block + last eligible message"
	);
	assert!(markers.contains(&2), "compressed block must be cached");
}

// ── Case 3: two markers both inside the range ─────────────────────────────────
// Both destroyed by drain. Compressed block gets marker #1, last eligible
// message gets marker #2 — always 2 markers after compression.
#[test]
fn case3_two_markers_inside_range_compressed_block_gets_cached() {
	// idx: 0=system, 1=user(start), 2=user(cached!), 3=assistant, 4=user(cached!), 5=assistant(end), 6..9=preserved
	let messages = vec![
		msg("system", false),
		msg("user", false), // start_idx=1 (kept)
		msg("user", true),  // marker #1 — inside range
		msg("assistant", false),
		msg("user", true),       // marker #2 — inside range
		msg("assistant", false), // end_idx=5
		msg("user", false),
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	let (_, had_cached) = cs.remove_messages_in_range(1, 5).unwrap();
	assert!(had_cached);
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	let markers = content_cache_indices(&cs);
	// After drain+insert: [sys(0), user(1), COMP(2), user(3), asst(4), user(5), asst(6)]
	assert_eq!(
		markers.len(),
		2,
		"must have 2 markers: compressed block + last eligible message"
	);
	assert!(markers.contains(&2), "compressed block must be cached");
}

// ── Case 4: marker at start_idx, one inside range ───────────────────────────
// start_idx marker survives the drain initially, but is redundant once the
// compressed block is inserted. The second marker moves to the latest preserved
// user/tool boundary so the preserved tail remains cached.
#[test]
fn case4_marker_at_start_idx_and_one_inside_moves_to_latest_preserved_boundary() {
	// idx: 0=system, 1=user(start,cached!), 2=assistant, 3=user(cached!), 4=assistant(end), 5..8=preserved
	let messages = vec![
		msg("system", false),
		msg("user", true), // start_idx=1, marker #1 (KEPT by drain, later evicted)
		msg("assistant", false),
		msg("user", true),       // marker #2 — inside range, removed
		msg("assistant", false), // end_idx=4
		msg("user", false),
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	let (_, had_cached) = cs.remove_messages_in_range(1, 4).unwrap();
	assert!(had_cached);
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	let markers = content_cache_indices(&cs);
	// compressed block at idx=2 + latest preserved user at idx=5
	assert_eq!(
		markers,
		vec![2, 5],
		"compressed block + latest preserved boundary are cached"
	);
}
// ── Case 5: marker at start_idx only, nothing inside range ───────────────────
// had_cached=false from remove, but compressed block must still get cached=true.
// start_idx marker is then evicted so marker #2 can cover the preserved tail.
#[test]
fn case5_marker_at_start_idx_only_moves_to_latest_preserved_boundary() {
	// idx: 0=system, 1=user(start,cached!), 2=assistant, 3=user, 4=assistant(end), 5..8=preserved
	let messages = vec![
		msg("system", false),
		msg("user", true), // start_idx=1, marker #1 (KEPT by drain, later evicted)
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false), // end_idx=4
		msg("user", false),
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	let (_, had_cached) = cs.remove_messages_in_range(1, 4).unwrap();
	assert!(!had_cached, "nothing inside range was cached");
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	let markers = content_cache_indices(&cs);
	assert_eq!(
		markers,
		vec![2, 5],
		"compressed block + latest preserved boundary are cached"
	);
}

// ── Case 6: marker in preserved zone (after end_idx) — untouched ─────────────
// Compression should not disturb markers that are beyond the compressed range.
#[test]
fn case6_marker_in_preserved_zone_stays_untouched() {
	// idx: 0=system, 1=user(start), 2=assistant, 3=user(end), 4=user, 5=assistant, 6=user(cached!), 7=assistant
	let messages = vec![
		msg("system", false),
		msg("user", false), // start_idx=1 (kept)
		msg("assistant", false),
		msg("user", false), // end_idx=3
		msg("user", false), // preserved zone starts
		msg("assistant", false),
		msg("user", true), // marker in preserved zone
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	let (_, had_cached) = cs.remove_messages_in_range(1, 3).unwrap();
	assert!(!had_cached);
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	let markers = content_cache_indices(&cs);
	// Compressed block at idx 2 (cached=true) + preserved zone marker shifted to idx 5
	assert!(markers.contains(&2), "compressed block must be cached");
	// The preserved zone marker should still exist somewhere after the compressed block
	let preserved_marker_exists = cs.session.messages[3..]
		.iter()
		.any(|m| m.cached && m.role != "system");
	assert!(
		preserved_marker_exists,
		"preserved zone marker must survive untouched"
	);
}

// ── Case 7: system marker never touched ──────────────────────────────────────
// System message cached=true must never be affected by compression.
#[test]
fn case7_system_marker_never_touched_by_compression() {
	let messages = vec![
		msg("system", true), // system marker — must never change
		msg("user", false),  // start_idx=1 (kept)
		msg("assistant", false),
		msg("user", true),       // content marker inside range
		msg("assistant", false), // end_idx=4
		msg("user", false),
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	let (_, _) = cs.remove_messages_in_range(1, 4).unwrap();
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	assert!(
		cs.session.messages[0].cached,
		"system marker must remain cached=true"
	);
}

// ── Case 8: two markers already in preserved zone ────────────────────────────
// This is the bug introduced in commit 659992f: insert_compressed_knowledge
// unconditionally sets cached=true on the compressed block even when 2 content
// markers already exist in the preserved zone.  That produces 3 content markers
// (system + tools + 3 content = 5 cache_control blocks) which Anthropic rejects
// with "A maximum of 4 blocks with cache_control may be provided. Found 5."
//
// Correct behaviour: when 2 content markers already exist outside the compressed
// range, the compressed block must NOT add a third one.  Instead it should evict
// the oldest surviving content marker so the total stays at ≤ 2.
#[test]
fn case8_two_markers_in_preserved_zone_compressed_block_must_not_exceed_two_content_markers() {
	// idx: 0=system, 1=user(start), 2=assistant, 3=user(end),
	//      4=user(cached!), 5=assistant, 6=user(cached!), 7=assistant  ← preserved zone
	let messages = vec![
		msg("system", false),
		msg("user", false), // start_idx=1 (kept)
		msg("assistant", false),
		msg("user", false), // end_idx=3
		msg("user", true),  // marker #1 in preserved zone
		msg("assistant", false),
		msg("user", true), // marker #2 in preserved zone
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	let (_, had_cached) = cs.remove_messages_in_range(1, 3).unwrap();
	assert!(!had_cached, "nothing inside range was cached");
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	// After compression the total number of non-system cached messages must be ≤ 2.
	// Before the fix this was 3 (compressed block + 2 preserved markers), which
	// causes Anthropic to reject the request.
	let markers = content_cache_indices(&cs);
	assert!(
		markers.len() <= 2,
		"must have at most 2 content cache markers after compression, got {}: {:?}",
		markers.len(),
		markers
	);
}

// ── Case 9: THE BUG — markers disappear after compression ────────────────────
// Regression test for the core bug: before the fix, when markers existed inside
// the compressed range they were destroyed by drain, and insert_compressed_knowledge
// only placed marker #1 on the compressed block.  Marker #2 was never restored,
// so the entire preserved zone was sent uncached on the next API call.
//
// This test simulates a realistic session: 2 markers exist (one mid-conversation,
// one near the end), compression removes the range containing the first marker.
// After compression we MUST have exactly 2 markers:
//   - marker #1: the compressed block (stable history boundary)
//   - marker #2: the last eligible user/tool message (moving boundary)
#[test]
fn case9_markers_must_not_disappear_after_compression() {
	// Realistic session layout:
	// idx: 0=system, 1=user(start), 2=assistant, 3=user(cached!), 4=assistant,
	//      5=user, 6=assistant(end),
	//      7=user, 8=assistant, 9=user, 10=assistant, 11=user(cached!), 12=assistant
	let messages = vec![
		msg("system", false),
		msg("user", false), // start_idx=1 (anchor, kept)
		msg("assistant", false),
		msg("user", true), // marker #1 — inside compression range
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false), // end_idx=6
		// preserved zone:
		msg("user", false),
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false),
		msg("user", true), // marker #2 — in preserved zone
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	// Verify pre-compression state: exactly 2 content markers
	let markers_before = content_cache_indices(&cs);
	assert_eq!(
		markers_before.len(),
		2,
		"pre-compression: must have 2 markers, got {:?}",
		markers_before
	);

	// Compress: drain indices 2..=6, insert compressed block
	let (removed, had_cached) = cs.remove_messages_in_range(1, 6).unwrap();
	assert!(removed > 0);
	assert!(had_cached, "marker #1 was inside the range");
	cs.insert_compressed_knowledge(1, "compressed summary".to_string())
		.unwrap();

	// Post-compression: MUST still have exactly 2 content markers
	let markers_after = content_cache_indices(&cs);
	assert_eq!(
		markers_after.len(),
		2,
		"post-compression: must have exactly 2 markers, got {:?}. \
			 BUG: markers disappeared after compression!",
		markers_after
	);

	// Verify marker #1 is the compressed block (always at start_idx+1)
	let compressed_idx = 2; // inserted after start_idx=1
	assert!(
		markers_after.contains(&compressed_idx),
		"marker #1 must be the compressed block at idx {}",
		compressed_idx
	);

	// Verify marker #2 is NOT the compressed block (it's somewhere in preserved zone)
	let marker2 = markers_after
		.iter()
		.find(|&&i| i != compressed_idx)
		.unwrap();
	assert!(
		*marker2 > compressed_idx,
		"marker #2 must be after the compressed block"
	);

	// Verify the message at marker #2 is user or tool (eligible for caching)
	let marker2_msg = &cs.session.messages[*marker2];
	assert!(
		marker2_msg.role == "user" || marker2_msg.role == "tool",
		"marker #2 must be on a user or tool message, got role='{}'",
		marker2_msg.role
	);
}

// ── Case 10: no markers before compression — both created fresh ──────────────
// When a session has never had any cache markers (e.g. caching was disabled or
// the session is very short), compression must still establish the full 2-marker
// layout from scratch.
#[test]
fn case10_no_markers_before_compression_both_created() {
	let messages = vec![
		msg("system", false),
		msg("user", false), // start_idx=1
		msg("assistant", false),
		msg("user", false),
		msg("assistant", false), // end_idx=4
		msg("user", false),
		msg("assistant", false),
		msg("user", false), // last user — should become marker #2
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	// Pre-compression: zero markers
	let markers_before = content_cache_indices(&cs);
	assert_eq!(markers_before.len(), 0, "no markers before compression");

	let (_, _) = cs.remove_messages_in_range(1, 4).unwrap();
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	let markers_after = content_cache_indices(&cs);
	assert_eq!(
		markers_after.len(),
		2,
		"must create both markers from scratch, got {:?}",
		markers_after
	);
}

// ── Case 11: marker #2 on tool message in preserved zone ─────────────────────
// Tool messages are also eligible for marker #2.
#[test]
fn case11_marker2_placed_on_tool_message() {
	let messages = vec![
		msg("system", false),
		msg("user", false), // start_idx=1
		msg("assistant", false),
		msg("user", false), // end_idx=3
		// preserved zone:
		msg("user", false),
		msg("assistant", false),
		msg("tool", false), // last eligible — should become marker #2
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	let (_, _) = cs.remove_messages_in_range(1, 3).unwrap();
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	let markers = content_cache_indices(&cs);
	assert_eq!(markers.len(), 2, "must have 2 markers, got {:?}", markers);

	// The second marker should be on the tool message
	let last_marker_idx = *markers.last().unwrap();
	assert_eq!(
		cs.session.messages[last_marker_idx].role, "tool",
		"marker #2 should be on the tool message"
	);
}

#[test]
fn case12_compression_moves_second_marker_to_latest_preserved_message() {
	let messages = vec![
		msg("system", false),
		msg("user", false), // start_idx=1
		msg("assistant", false),
		msg("user", false), // end_idx=3
		msg("user", true),  // stale marker in preserved zone
		msg("assistant", false),
		msg("user", false), // freshest eligible — must become marker #2
		msg("assistant", false),
	];
	let mut cs = make_session(messages);

	let (_, _) = cs.remove_messages_in_range(1, 3).unwrap();
	cs.insert_compressed_knowledge(1, "summary".to_string())
		.unwrap();

	let markers = content_cache_indices(&cs);
	assert_eq!(markers.len(), 2, "must have exactly 2 markers");
	assert_eq!(cs.session.messages[markers[0]].content, "summary");
	assert_eq!(markers[1], 5, "marker #2 must move to freshest user");
	assert!(
		!cs.session.messages[3].cached,
		"stale preserved marker must be evicted"
	);
}

#[test]
fn generate_session_name_format() {
	// Format: YYMMDD-<basename>-HHMM-<uuid4>. The basename is the working
	// directory name and may itself contain dashes, so parse from the ends
	// instead of by dash position.
	let name = generate_session_name();
	let parts: Vec<&str> = name.split('-').collect();
	assert!(
		parts.len() >= 4,
		"session name should have at least 4 dash-separated parts, got: {name}"
	);

	// First part: YYMMDD (6 digits)
	let date_part = parts[0];
	assert_eq!(
		date_part.len(),
		6,
		"date part should be 6 chars, got: {date_part}"
	);
	assert!(
		date_part.chars().all(|c| c.is_ascii_digit()),
		"date part should be all digits, got: {date_part}"
	);

	// Middle: basename (directory name, non-empty, may contain dashes)
	let basename = parts[1..parts.len() - 2].join("-");
	assert!(!basename.is_empty(), "basename should not be empty");

	// Second-to-last part: HHMM (4 digits)
	let time_part = parts[parts.len() - 2];
	assert_eq!(
		time_part.len(),
		4,
		"time part should be 4 chars, got: {time_part}"
	);
	assert!(
		time_part.chars().all(|c| c.is_ascii_digit()),
		"time part should be all digits, got: {time_part}"
	);

	// Last part: uuid4 (4 hex chars)
	let uuid_part = parts[parts.len() - 1];
	assert_eq!(
		uuid_part.len(),
		4,
		"uuid part should be 4 chars, got: {uuid_part}"
	);
	assert!(
		uuid_part.chars().all(|c| c.is_ascii_hexdigit()),
		"uuid part should be hex, got: {uuid_part}"
	);
}
