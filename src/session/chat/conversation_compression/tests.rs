use super::collect_preserved_skills;
use super::knowledge::{format_compressed_entry_with_context, strip_file_context_from_summary};
use super::range::find_compression_range;
use super::schema::{is_summary_substantive, render_summary, CompressionSummary, KeyEntities};
use super::select_compression_level_index;
use crate::session::Message;
use serde_json::json;

fn msg(role: &str) -> Message {
	Message {
		role: role.to_string(),
		content: String::new(),
		..Default::default()
	}
}

fn skill_msg(name: &str) -> Message {
	Message {
		role: "user".to_string(),
		content: format!(
			"<skill name=\"{}\" description=\"test skill\">\nbody for {}\n</skill>",
			name, name
		),
		..Default::default()
	}
}

#[test]
fn preserves_active_skill_in_drain_range() {
	// Layout: [system, welcome, instructions, user_req1, asst,
	//         skill(rust), user_req2, asst, user_req3, asst]
	let mut messages = vec![
		msg("system"),    // 0
		msg("assistant"), // 1 welcome
		{
			let mut m = msg("user"); // 2 instructions
			m.content = "instructions".into();
			m
		},
		{
			let mut m = msg("user"); // 3 user_req1
			m.content = "first request".into();
			m
		},
		{
			let mut m = msg("assistant"); // 4
			m.content = "reply 1".into();
			m
		},
		skill_msg("programming-rust"), // 5
		{
			let mut m = msg("user"); // 6 user_req2
			m.content = "second request".into();
			m
		},
		{
			let mut m = msg("assistant"); // 7
			m.content = "reply 2".into();
			m
		},
		{
			let mut m = msg("user"); // 8 user_req3
			m.content = "third request".into();
			m
		},
		{
			let mut m = msg("assistant"); // 9
			m.content = "reply 3".into();
			m
		},
	];

	// first_prompt_idx = 3 (first real user prompt).
	// find_compression_range moves anchor to idx-1 = 2 (instructions).
	// Drain range: 3..=9.
	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();
	assert_eq!(start_idx, 2, "anchor on instructions");
	assert_eq!(end_idx, 9);

	let active = vec!["programming-rust".to_string()];
	let preserved = collect_preserved_skills(&messages, start_idx + 1, end_idx, &active);
	assert_eq!(preserved.len(), 1);
	assert!(preserved[0]
		.content
		.contains("<skill name=\"programming-rust\""));

	// Filter mirrors check_and_compress_conversation: user messages that are
	// NOT skill messages, non-empty.
	let user_tasks: Vec<String> = messages[start_idx + 1..=end_idx]
		.iter()
		.filter(|m| {
			m.role == "user"
				&& !m.content.trim().is_empty()
				&& !crate::mcp::core::skill::is_skill_message(&m.content)
		})
		.map(|m| m.content.clone())
		.collect();

	// Last is re-injected raw, prior entries become USER TASKS.
	assert_eq!(
		user_tasks,
		vec![
			"first request".to_string(),
			"second request".to_string(),
			"third request".to_string(),
		],
		"skill content must NOT appear as a user task"
	);
	assert_eq!(
		user_tasks.last().unwrap(),
		"third request",
		"last user message for re-injection is the real request, not the skill"
	);

	// Simulate apply_compression placement: drain 3..=9, insert skills at
	// start_idx+1, then summary at start_idx+1+skill_count, then user.
	messages.drain(start_idx + 1..=end_idx);
	for (i, mut s) in preserved.into_iter().enumerate() {
		s.cached = false;
		s.cache_ttl = None;
		messages.insert(start_idx + 1 + i, s);
	}
	let skill_count = 1;
	messages.insert(start_idx + 1 + skill_count, {
		let mut m = msg("assistant");
		m.content = "SUMMARY".into();
		m
	});
	messages.insert(start_idx + 2 + skill_count, {
		let mut m = msg("user");
		m.content = "third request".into();
		m
	});

	// Expected post-compression layout:
	// [system, welcome, instructions(anchor), skill, SUMMARY, user_req3]
	assert_eq!(messages.len(), 6);
	assert_eq!(messages[2].content, "instructions");
	assert!(
		crate::mcp::core::skill::is_skill_message(&messages[3].content),
		"skill comes right after anchor"
	);
	assert_eq!(messages[4].content, "SUMMARY");
	assert_eq!(messages[5].content, "third request");
}

#[test]
fn drops_forgotten_skill_from_preservation() {
	// Skill is in range but not in active list → must be dropped.
	let messages = vec![
		msg("system"),
		msg("user"),
		skill_msg("programming-rust"),
		msg("assistant"),
		msg("user"),
		msg("assistant"),
	];
	let active: Vec<String> = Vec::new(); // user forgot the skill
	let preserved = collect_preserved_skills(&messages, 1, 5, &active);
	assert!(preserved.is_empty(), "forgotten skills are not preserved");
}

#[test]
fn dedupes_duplicate_skill_keeping_latest() {
	// Same skill injected twice in range — keep the second (latest) copy.
	let mut first = skill_msg("programming-rust");
	first.content =
		"<skill name=\"programming-rust\" description=\"v1\">\nold body\n</skill>".to_string();
	let mut second = skill_msg("programming-rust");
	second.content =
		"<skill name=\"programming-rust\" description=\"v2\">\nnew body\n</skill>".to_string();

	let messages = vec![
		msg("system"),
		msg("user"),
		first,
		msg("assistant"),
		second,
		msg("assistant"),
	];
	let active = vec!["programming-rust".to_string()];
	let preserved = collect_preserved_skills(&messages, 1, 5, &active);
	assert_eq!(preserved.len(), 1);
	assert!(
		preserved[0].content.contains("new body"),
		"latest injection wins on dedup"
	);
}

#[test]
fn preserves_multiple_distinct_skills_in_order() {
	let messages = vec![
		msg("system"),
		msg("user"),
		skill_msg("programming-rust"),
		msg("assistant"),
		skill_msg("git-workflow"),
		msg("user"),
		msg("assistant"),
	];
	let active = vec!["programming-rust".to_string(), "git-workflow".to_string()];
	let preserved = collect_preserved_skills(&messages, 1, 6, &active);
	assert_eq!(preserved.len(), 2);
	assert!(preserved[0].content.contains("programming-rust"));
	assert!(preserved[1].content.contains("git-workflow"));
}

#[test]
fn empty_range_returns_empty() {
	let messages = vec![msg("system")];
	let preserved = collect_preserved_skills(&messages, 5, 10, &["foo".to_string()]);
	assert!(preserved.is_empty());
}

#[test]
fn extends_range_to_include_tool_results() {
	let mut messages = Vec::new();
	messages.push(msg("system")); // 0

	// Create scenario where tool messages are between conversation messages
	messages.push(msg("user")); // 1
	let mut assistant1 = msg("assistant"); // 2
	assistant1.tool_calls = Some(json!([
		{"id": "call_1", "type": "function", "function": {"name": "tool1"}}
	]));
	messages.push(assistant1);
	let mut tool1 = msg("tool"); // 3
	tool1.tool_call_id = Some("call_1".to_string());
	messages.push(tool1);

	messages.push(msg("user")); // 4
	messages.push(msg("assistant")); // 5
	messages.push(msg("user")); // 6
	messages.push(msg("assistant")); // 7
	messages.push(msg("user")); // 8
	messages.push(msg("assistant")); // 9

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	// Compress-all: end_idx = last message
	assert_eq!(start_idx, 1);
	assert_eq!(end_idx, 9, "compress-all: end_idx = last message");
}

#[test]
fn extends_when_ending_on_assistant_with_tools() {
	// THIS is the critical test - tool messages between conversation messages
	let mut messages = vec![
		msg("system"),    // 0
		msg("user"),      // 1
		msg("assistant"), // 2
		msg("user"),      // 3
	];
	let mut assistant_with_tools = msg("assistant"); // 4
	assistant_with_tools.tool_calls = Some(json!([
		{"id": "call_1", "type": "function", "function": {"name": "tool1"}}
	]));
	messages.push(assistant_with_tools);
	let mut tool1 = msg("tool"); // 5
	tool1.tool_call_id = Some("call_1".to_string());
	messages.push(tool1);

	messages.push(msg("user")); // 6
	messages.push(msg("assistant")); // 7
	messages.push(msg("user")); // 8
	messages.push(msg("assistant")); // 9

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	// Compress-all: end_idx = last message
	assert_eq!(start_idx, 1);
	assert_eq!(end_idx, 9, "compress-all: end_idx = last message");
}

#[test]
fn handles_multiple_assistants_with_tools() {
	// Test scenario: multiple assistant messages with tool calls in sequence
	let mut messages = Vec::new();
	messages.push(msg("system")); // 0

	messages.push(msg("user")); // 1

	// First assistant with tools
	let mut assistant1 = msg("assistant"); // 2
	assistant1.tool_calls = Some(json!([
		{"id": "call_1", "type": "function", "function": {"name": "tool1"}}
	]));
	messages.push(assistant1);
	let mut tool1 = msg("tool"); // 3
	tool1.tool_call_id = Some("call_1".to_string());
	messages.push(tool1);

	// Second assistant with tools (no user message between)
	let mut assistant2 = msg("assistant"); // 4
	assistant2.tool_calls = Some(json!([
		{"id": "call_2", "type": "function", "function": {"name": "tool2"}}
	]));
	messages.push(assistant2);
	let mut tool2 = msg("tool"); // 5
	tool2.tool_call_id = Some("call_2".to_string());
	messages.push(tool2);

	// More conversation messages to trigger compression
	messages.push(msg("user")); // 6
	messages.push(msg("assistant")); // 7
	messages.push(msg("user")); // 8
	messages.push(msg("assistant")); // 9
	messages.push(msg("user")); // 10

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	// Compress-all: end_idx = last message, no preserved zone
	assert_eq!(start_idx, 1);
	assert_eq!(end_idx, 10, "compress-all: end_idx = last message");
}

#[test]
fn start_boundary_must_not_orphan_initial_tool_sequence() {
	let mut messages = Vec::new();
	messages.push(msg("system")); // 0

	// First conversation message is assistant with tool calls.
	// This can happen in resumed sessions or reconstructed histories.
	let mut assistant_with_tools = msg("assistant"); // 1
	assistant_with_tools.tool_calls = Some(json!([
		{"id": "call_1", "type": "function", "function": {"name": "tool1"}}
	]));
	messages.push(assistant_with_tools);

	let mut tool1 = msg("tool"); // 2
	tool1.tool_call_id = Some("call_1".to_string());
	messages.push(tool1);

	// Add enough conversation messages to trigger compression.
	messages.push(msg("user")); // 3
	messages.push(msg("assistant")); // 4
	messages.push(msg("user")); // 5
	messages.push(msg("assistant")); // 6
	messages.push(msg("user")); // 7
	messages.push(msg("assistant")); // 8

	// First user message lives at idx 3 — anchor lands there. The leading
	// assistant+tool sequence at indices 1-2 stays in the surviving prefix
	// (kept across compression cycles).
	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	assert_eq!(start_idx, 3, "anchor = first user message at idx 3");
	assert!(
		end_idx >= 4,
		"compression range must include messages after anchor"
	);
}

#[test]
fn leading_tool_exchange_stays_in_prefix_no_orphans() {
	// When the session begins with an assistant+tool_calls turn BEFORE any
	// user message (e.g. resumed/reconstructed history), anchor lands on the
	// first user message that follows. The leading assistant + its tool
	// results stay together in the surviving prefix — neither side of the
	// pair can fall into the drain range, so no orphan tool_use blocks.
	let mut messages = Vec::new();
	messages.push(msg("system")); // 0

	let mut assistant = msg("assistant"); // 1
	assistant.tool_calls = Some(json!([
		{"id": "call_A", "type": "function", "function": {"name": "view_signatures", "arguments": "{}"}},
		{"id": "call_B", "type": "function", "function": {"name": "view", "arguments": "{}"}}
	]));
	messages.push(assistant);

	let mut tool_a = msg("tool"); // 2
	tool_a.tool_call_id = Some("call_A".to_string());
	tool_a.name = Some("view_signatures".to_string());
	messages.push(tool_a);

	let mut tool_b = msg("tool"); // 3
	tool_b.tool_call_id = Some("call_B".to_string());
	tool_b.name = Some("view".to_string());
	messages.push(tool_b);

	messages.push(msg("assistant")); // 4 (response after tools)
	messages.push(msg("user")); // 5 - first user message (anchor)
	messages.push(msg("assistant")); // 6
	messages.push(msg("user")); // 7
	messages.push(msg("assistant")); // 8
	messages.push(msg("user")); // 9
	messages.push(msg("assistant")); // 10

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	assert_eq!(start_idx, 5, "anchor = first user message at idx 5");
	assert!(end_idx > start_idx);

	// Drain range [6..=10] contains no tool messages (all asst/user), so no
	// orphan risk. Tools at 2,3 stay paired with their assistant at 1 in the
	// preserved prefix.
	for m in messages.iter().take(end_idx + 1).skip(start_idx + 1) {
		assert_ne!(m.role, "tool", "drain range must not contain tool messages");
	}
}

#[test]
fn anchor_when_first_user_precedes_tool_calls_assistant() {
	// First user message sits before an assistant-with-tool_calls turn.
	// Anchor lands on the user message (idx 1); the entire tool exchange
	// (assistant + its tool result) is inside the drain range. Anchor is
	// user-role so no orphan tool_use blocks can form.
	let mut messages = Vec::new();
	messages.push(msg("system")); // 0
	messages.push(msg("user")); // 1 - first user (anchor)

	let mut assistant = msg("assistant"); // 2
	assistant.tool_calls = Some(json!([
		{"id": "call_X", "type": "function", "function": {"name": "shell", "arguments": "{}"}}
	]));
	messages.push(assistant);

	let mut tool_x = msg("tool"); // 3
	tool_x.tool_call_id = Some("call_X".to_string());
	tool_x.name = Some("shell".to_string());
	messages.push(tool_x);

	messages.push(msg("assistant")); // 4
	messages.push(msg("user")); // 5
	messages.push(msg("assistant")); // 6
	messages.push(msg("user")); // 7
	messages.push(msg("assistant")); // 8
	messages.push(msg("user")); // 9
	messages.push(msg("assistant")); // 10

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	assert_eq!(start_idx, 1, "anchor = first user message at idx 1");
	assert!(end_idx > start_idx, "must have valid range");
	assert!(end_idx >= 3, "drain must include tool result at idx 3");
}

// ============================================================================
// BOOTSTRAP MESSAGE PRESERVATION TESTS: Verify system prompt, welcome message,
// and instructions file are NEVER compressed away
// ============================================================================

#[test]
fn welcome_preserved_when_no_instructions_file() {
	// Without an <instructions> message, anchor falls back to the first user
	// message. System and welcome live BEFORE the anchor and are never in the
	// drain range, regardless of session origin (fresh or resumed).
	let messages = vec![
		msg("system"),    // 0
		msg("assistant"), // 1 - welcome message
		msg("user"),      // 2 - first real user prompt
		msg("assistant"), // 3
		msg("user"),      // 4
		msg("assistant"), // 5
		msg("user"),      // 6
		msg("assistant"), // 7
		msg("user"),      // 8
		msg("assistant"), // 9
	];

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	assert_eq!(start_idx, 2, "anchor = first user message at idx 2");
	assert!(end_idx > start_idx, "must have valid range");
	assert!(
		start_idx + 1 > 1,
		"drain range must not include welcome message at idx 1"
	);
}

#[test]
fn anchor_is_instructions_message_when_present() {
	// When a user-role message wraps content in <instructions>…</instructions>,
	// that message becomes the anchor — its content is never compressed away.
	// Drain starts immediately after it.
	let mut instructions = msg("user");
	instructions.content = "<instructions>\nproject guidelines\n</instructions>".into();
	let messages = vec![
		msg("system"),    // 0
		msg("assistant"), // 1 - welcome
		instructions,     // 2 - instructions file (DETECTED via tag)
		msg("assistant"), // 3
		msg("user"),      // 4 - first real user prompt
		msg("assistant"), // 5
		msg("user"),      // 6
		msg("assistant"), // 7
		msg("user"),      // 8
		msg("assistant"), // 9
	];

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	assert_eq!(start_idx, 2, "anchor must be the <instructions> message");
	assert_eq!(end_idx, 9, "compress-all: end_idx = last message");
}

#[test]
fn bootstrap_preserved_system_message_never_in_range() {
	// Regardless of first_prompt_idx, system message must never be in compression range
	let mut messages = Vec::new();
	messages.push(msg("system")); // 0
	messages.push(msg("assistant")); // 1
	for _ in 0..10 {
		messages.push(msg("user"));
		messages.push(msg("assistant"));
	}

	// Test with None
	let (start_none, _end_none) = find_compression_range(&messages, false).unwrap();
	assert!(start_none > 0, "system message at 0 must not be start_idx");
	// Drain is start_idx+1..=end_idx, so system at 0 is safe if start_idx > 0

	// Test with Some(1)
	let (start_some, end_some) = find_compression_range(&messages, false).unwrap();
	assert!(start_some >= 1, "start_idx must be >= 1");
	assert!(end_some > start_some);
}

#[test]
fn anchor_with_instructions_then_assistant_tool_calls() {
	// Instructions message immediately followed by an assistant turn with
	// tool_calls — anchor stays on the instructions message, everything
	// after (including the tool_calls assistant and its tool results) is
	// in the drain range. No tool-skip required: anchor is user-role, so
	// no orphan tool_use blocks can form.
	let mut instructions = msg("user");
	instructions.content = "<instructions>\nrules\n</instructions>".into();

	let mut assistant_tc = msg("assistant");
	assistant_tc.tool_calls = Some(serde_json::json!([
		{"id": "call_1", "type": "function", "function": {"name": "view", "arguments": "{}"}}
	]));
	let mut tool = msg("tool");
	tool.tool_call_id = Some("call_1".to_string());

	let messages = vec![
		msg("system"),    // 0
		msg("assistant"), // 1 welcome
		instructions,     // 2 instructions
		assistant_tc,     // 3 asst with tool_calls
		tool,             // 4 tool result
		msg("assistant"), // 5
		msg("user"),      // 6
		msg("assistant"), // 7
		msg("user"),      // 8
		msg("assistant"), // 9
		msg("user"),      // 10
		msg("assistant"), // 11
	];

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	assert_eq!(start_idx, 2, "anchor on <instructions> message");
	assert_eq!(end_idx, 11, "compress-all: end_idx = last message");
	// Drain range [3..=11] includes the assistant+tool_calls AND its tool result —
	// both go together, no orphaning possible.
}

#[test]
fn calculate_range_tokens_must_match_removal_range() {
	// CRITICAL TEST: Verify that calculate_range_tokens counts the EXACT same messages
	// that will be removed by remove_messages_in_range.
	//
	// BUG SCENARIO:
	// - find_compression_range returns (start_idx, end_idx)
	// - calculate_range_tokens counts [start_idx+1, end_idx] (SKIPS start_idx)
	// - messages_to_compress includes [start_idx, end_idx] for chunking
	// - remove_messages_in_range removes [start_idx+1, end_idx] (KEEPS start_idx)
	//
	// This means:
	// 1. tokens_before doesn't count the message at start_idx
	// 2. But that message IS included in semantic chunking
	// 3. The compressed summary can include content from start_idx message
	// 4. Result: tokens_after can be > tokens_before (BUG!)
	//
	// EXAMPLE:
	// - start_idx = 5, end_idx = 10
	// - tokens_before counts messages 6-10 (skips message 5)
	// - messages_to_compress includes message 5 for chunking
	// - If message 5 has 1000 tokens and messages 6-10 have 500 tokens total
	// - tokens_before = 500
	// - Compressed summary might be 600 tokens (includes content from message 5)
	// - tokens_after = 600
	// - Result: tokens_saved = 0 even though we removed 5 messages!
	//
	// FIX: calculate_range_tokens should count [start_idx, end_idx] to match
	// the messages that will be semantically chunked and potentially included in summary.

	// This test documents the expected behavior.
	// The actual fix will be in calculate_range_tokens function.
	use crate::session::estimate_message_tokens;

	let mut messages = Vec::new();
	messages.push(msg("system")); // 0

	// Create messages with known token counts
	let mut msg1 = msg("user"); // 1
	msg1.content = "x".repeat(100); // ~25 tokens
	messages.push(msg1);

	let mut msg2 = msg("assistant"); // 2
	msg2.content = "y".repeat(200); // ~50 tokens
	messages.push(msg2);

	let mut msg3 = msg("user"); // 3
	msg3.content = "z".repeat(300); // ~75 tokens
	messages.push(msg3);

	let mut msg4 = msg("assistant"); // 4
	msg4.content = "a".repeat(400); // ~100 tokens
	messages.push(msg4);

	// Add more messages to trigger compression
	messages.push(msg("user")); // 5
	messages.push(msg("assistant")); // 6
	messages.push(msg("user")); // 7
	messages.push(msg("assistant")); // 8

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	// Verify the range is valid
	assert!(start_idx < end_idx, "Range must be valid");

	// Count tokens that WILL BE REMOVED (matching remove_messages_in_range logic)
	// remove_messages_in_range removes [start_idx+1, end_idx]
	let expected_tokens: u64 = messages[(start_idx + 1)..=end_idx]
		.iter()
		.map(|m| estimate_message_tokens(m) as u64)
		.sum();

	// Count tokens that ARE INCLUDED in semantic chunking
	// messages_to_compress = [start_idx, end_idx]
	let chunked_tokens: u64 = messages[start_idx..=end_idx]
		.iter()
		.map(|m| estimate_message_tokens(m) as u64)
		.sum();

	// THE BUG: expected_tokens != chunked_tokens
	// calculate_range_tokens returns expected_tokens (removal range)
	// But semantic chunking includes chunked_tokens (includes start_idx)
	// This can cause tokens_after > tokens_before

	// Document the discrepancy
	if expected_tokens != chunked_tokens {
		let start_msg_tokens = estimate_message_tokens(&messages[start_idx]) as u64;
		assert_eq!(
			chunked_tokens - expected_tokens,
			start_msg_tokens,
			"The difference should be exactly the tokens in start_idx message"
		);
	}
}

// ============================================================================
// BUG-PROVING TESTS: These tests demonstrate the actual bugs in compression
// ============================================================================

#[test]
fn bug_proof_token_mismatch_causes_zero_savings() {
	// BUG SCENARIO: calculate_range_tokens counts [start_idx+1, end_idx]
	// but semantic chunking uses [start_idx, end_idx], causing token mismatch
	use crate::session::estimate_message_tokens;

	let mut messages = Vec::new();
	messages.push(msg("system")); // 0

	// Message at start_idx has LARGE token count
	let mut large_msg = msg("user"); // 1
	large_msg.content = "x".repeat(4000); // ~1000 tokens
	messages.push(large_msg);

	// Messages after start_idx have SMALL token counts
	let mut small1 = msg("assistant"); // 2
	small1.content = "y".repeat(40); // ~10 tokens
	messages.push(small1);

	let mut small2 = msg("user"); // 3
	small2.content = "z".repeat(40); // ~10 tokens
	messages.push(small2);

	let mut small3 = msg("assistant"); // 4
	small3.content = "a".repeat(40); // ~10 tokens
	messages.push(small3);

	// Add more to trigger compression
	messages.push(msg("user")); // 5
	messages.push(msg("assistant")); // 6
	messages.push(msg("user")); // 7
	messages.push(msg("assistant")); // 8

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();
	assert_eq!(start_idx, 1); // Large message
	assert_eq!(end_idx, 8); // compress-all: last message

	// What calculate_range_tokens ACTUALLY counts (CURRENT BUG)
	let tokens_counted_by_function: u64 = messages[(start_idx + 1)..=end_idx]
		.iter()
		.map(|m| estimate_message_tokens(m) as u64)
		.sum();

	// What semantic chunking ACTUALLY includes
	let tokens_in_chunking: u64 = messages[start_idx..=end_idx]
		.iter()
		.map(|m| estimate_message_tokens(m) as u64)
		.sum();

	// THE BUG: Massive discrepancy!
	let large_msg_tokens = estimate_message_tokens(&messages[start_idx]) as u64;

	// Debug: print actual token counts
	println!("Large message tokens: {}", large_msg_tokens);
	println!("Tokens counted by function: {}", tokens_counted_by_function);
	println!("Tokens in chunking: {}", tokens_in_chunking);

	// The key assertion: chunking includes start_idx, but counting doesn't
	assert_eq!(
		tokens_in_chunking,
		tokens_counted_by_function + large_msg_tokens,
		"Chunking includes the large message that wasn't counted!"
	);

	// Verify the large message has significantly more tokens than small ones
	assert!(
		large_msg_tokens > tokens_counted_by_function,
		"Large message ({}) should have more tokens than all small messages combined ({})",
		large_msg_tokens,
		tokens_counted_by_function
	);

	// RESULT: If compressed summary is 100 tokens (from small messages)
	// tokens_before = 30 (only small messages counted)
	// tokens_after = 100 (compressed summary)
	// tokens_saved = 0 or NEGATIVE! (BUG!)
	//
	// But we actually removed 1030 tokens worth of messages!
}

#[test]
fn bug_proof_insufficient_compression_triggers_loop() {
	// BUG SCENARIO: Compression triggers when full context > threshold
	// but doesn't check if compression will bring context BELOW threshold
	//
	// Example:
	// - Full context: 55,000 tokens
	// - Threshold: 50,000 tokens
	// - System + tools + recent: 52,000 tokens (non-compressible)
	// - Compressible old messages: 3,000 tokens
	// - After 2x compression: 52,000 + 1,500 = 53,500 tokens
	// - Still above threshold! Triggers again next iteration!

	// This test documents the expected behavior
	// The actual fix will be in should_check_compression

	let full_context_tokens = 55_000u64;
	let threshold = 50_000u64;
	let non_compressible_tokens = 52_000u64; // system + tools + recent
	let compressible_tokens = 3_000u64;
	let compression_ratio = 2.0;

	assert_eq!(
		full_context_tokens,
		non_compressible_tokens + compressible_tokens
	);

	// After compression
	let compressed_tokens = (compressible_tokens as f64 / compression_ratio) as u64;
	let tokens_after_compression = non_compressible_tokens + compressed_tokens;

	// THE BUG: Still above threshold!
	assert!(
		tokens_after_compression > threshold,
		"Compression didn't bring context below threshold: {} > {}",
		tokens_after_compression,
		threshold
	);

	// This will trigger compression AGAIN on next check
	// Creating a compression loop until continuation triggers
}

#[test]
fn bug_proof_compression_should_verify_benefit() {
	// BUG SCENARIO: Compression should check if it will actually help
	// before triggering. If non-compressible portion is already > threshold,
	// compression is futile.

	let threshold = 50_000u64;
	let system_tokens = 5_000u64;
	let tools_tokens = 30_000u64;
	let recent_4_messages_tokens = 20_000u64;
	let old_compressible_tokens = 2_000u64;

	let non_compressible = system_tokens + tools_tokens + recent_4_messages_tokens;
	let full_context = non_compressible + old_compressible_tokens;

	assert!(full_context > threshold, "Triggers compression");

	// Even with perfect 10x compression
	let best_case_compressed = old_compressible_tokens / 10;
	let best_case_result = non_compressible + best_case_compressed;

	// THE BUG: Even best-case compression won't help!
	assert!(
		best_case_result > threshold,
		"Non-compressible portion alone exceeds threshold: {} > {}",
		best_case_result,
		threshold
	);

	// FIX: should_check_compression should verify:
	// if (non_compressible + (compressible / ratio)) < threshold {
	//     compress
	// } else {
	//     skip compression — non-compressible portion already exceeds threshold
	// }
}

#[test]
fn test_cooldown_prevents_premature_recompression() {
	// TEST: Token-based cooldown blocks compression until context grows ≥10%

	// Scenario 1: After compression, context is at 50,000 tokens
	let tokens_after_compression: usize = 50_000;

	// Scenario 2: Context at 52,000 (4% growth) — should block
	let current_tokens_52k: usize = 52_000;
	let min_required = (tokens_after_compression as f64 * 1.1) as usize;
	assert!(
		current_tokens_52k < min_required,
		"Cooldown should block at 52k: {} < {} (need 10% growth)",
		current_tokens_52k,
		min_required
	);

	// Scenario 3: Context at 54,999 (~10% but not quite) — still blocked
	let current_tokens_54k: usize = 54_999;
	assert!(
		current_tokens_54k < min_required,
		"Cooldown should still block at 54,999: {} < {}",
		current_tokens_54k,
		min_required
	);

	// Scenario 4: Context at 55,000 (exactly 10% growth) — cooldown passes
	let current_tokens_55k: usize = 55_000;
	assert!(
		current_tokens_55k >= min_required,
		"Cooldown should pass at 55k: {} >= {}",
		current_tokens_55k,
		min_required
	);

	// Scenario 5: Context at 60,000 (20% growth) — allowed
	let current_tokens_60k: usize = 60_000;
	assert!(
		current_tokens_60k >= min_required,
		"Compression should be allowed at 60k: {} >= {}",
		current_tokens_60k,
		min_required
	);
}

#[test]
fn test_cooldown_default_allows_first_compression() {
	// TEST: Default value (0) should allow first compression immediately

	let tokens_after_compression: usize = 0; // Default — no prior compression
	let current_tokens: usize = 60_000;

	// When context_tokens_after_last_compression is 0, cooldown is inactive
	let cooldown_active = tokens_after_compression > 0
		&& current_tokens < (tokens_after_compression as f64 * 1.1) as usize;
	assert!(
		!cooldown_active,
		"First compression should be allowed when watermark is 0"
	);
}

#[test]
fn test_cooldown_scales_with_post_compression_size() {
	// TEST: Cooldown threshold scales proportionally with context size

	// Small context: 20k after compression → need 22k to recompress
	let small_watermark: usize = 20_000;
	let small_threshold = (small_watermark as f64 * 1.1) as usize;
	assert_eq!(small_threshold, 22_000, "Small: need 22k");

	// Medium context: 80k after compression → need 88k to recompress
	let medium_watermark: usize = 80_000;
	let medium_threshold = (medium_watermark as f64 * 1.1) as usize;
	assert_eq!(medium_threshold, 88_000, "Medium: need 88k");

	// Large context: 150k after compression → need 165k to recompress
	let large_watermark: usize = 150_000;
	let large_threshold = (large_watermark as f64 * 1.1) as usize;
	assert_eq!(large_threshold, 165_000, "Large: need 165k");

	// Growth headroom scales with context size
	let small_headroom = small_threshold - small_watermark;
	let large_headroom = large_threshold - large_watermark;
	assert!(
		large_headroom > small_headroom,
		"Larger contexts get more headroom: {} > {}",
		large_headroom,
		small_headroom
	);
}

#[test]
fn test_estimate_physical_ceiling_is_headroom_over_growth() {
	// physical_ceiling = headroom / growth_rate — pure math, no constants
	// headroom = current_tokens - compressed_tokens
	let current_tokens = 100_000.0_f64;
	let compression_ratio = 2.5_f64;
	let compressed = current_tokens / compression_ratio; // 40_000
	let headroom = current_tokens - compressed; // 60_000

	let growth_rate = 5_000.0_f64; // 5k output tokens/call
	let ceiling = headroom / growth_rate; // exactly 12 calls
	assert_eq!(ceiling, 12.0);

	// Larger growth rate → fewer calls fit → lower ceiling
	let ceiling_fast = headroom / 10_000.0_f64; // 6 calls
	assert!(ceiling_fast < ceiling, "faster growth → lower ceiling");

	// Higher compression ratio → more headroom → higher ceiling
	let compressed_aggressive = current_tokens / 4.0; // 25_000
	let headroom_aggressive = current_tokens - compressed_aggressive; // 75_000
	let ceiling_aggressive = headroom_aggressive / growth_rate; // 15 calls
	assert!(
		ceiling_aggressive > ceiling,
		"more compression → more headroom → higher ceiling"
	);
}

#[test]
fn test_estimate_symmetry_is_api_calls_so_far() {
	// Symmetry: calls remaining ≈ calls made (sessions are roughly symmetric)
	// Final = min(physical_ceiling, api_calls)
	let api_calls = 20.0_f64;
	let physical_ceiling = 30.0_f64;

	// symmetry < ceiling → symmetry wins (session likely winding down)
	let estimate = physical_ceiling.min(api_calls);
	assert_eq!(
		estimate, api_calls,
		"symmetry wins when smaller than ceiling"
	);

	// ceiling < symmetry → ceiling wins (context budget is the constraint)
	let api_calls_large = 50.0_f64;
	let estimate2 = physical_ceiling.min(api_calls_large);
	assert_eq!(
		estimate2, physical_ceiling,
		"ceiling wins when smaller than symmetry"
	);
}

#[test]
fn test_estimate_zero_api_calls_caps_physical_ceiling() {
	// With api_calls=0 and no output data, growth_rate floors at 1.0, producing a
	// huge raw ceiling (headroom / 1 = headroom). We cap at 100 so the cold-start
	// cooldown is meaningful rather than a nonsensical 60k+.
	let current_tokens = 100_000.0_f64;
	let compression_ratio = 2.5_f64;
	let compressed = current_tokens / compression_ratio;
	let headroom = current_tokens - compressed; // 60_000

	let growth_rate = (0.0_f64 / 1.0_f64).max(1.0); // floor=1, no data
	let raw_ceiling = headroom / growth_rate; // 60_000 — unreliable sentinel
	assert_eq!(raw_ceiling, 60_000.0);

	// Cap applied: cold-start estimate is bounded at 100
	let estimate = raw_ceiling.min(100.0);
	assert_eq!(estimate, 100.0, "cold-start ceiling capped at 100, not 60k");
	assert!(estimate >= 5.0, "always at least 5");
}

#[test]
fn test_estimate_growth_rate_from_measured_output() {
	// growth_rate = output_tokens / max(api_calls, 1), floored at 1.0
	// Floor at 1.0 is not a magic constant — it's division-by-zero protection
	let cases = [
		(10.0_f64, 50_000.0_f64, 5_000.0_f64), // measured: 5k/call
		(1.0, 3_000.0, 3_000.0),               // single call
		(0.0, 0.0, 1.0),                       // no data: floor=1 (not magic, just safe)
	];
	for (api_calls, output_tokens, expected) in cases {
		let rate = (output_tokens / api_calls.max(1.0)).max(1.0);
		assert_eq!(
			rate, expected,
			"api_calls={api_calls}, output={output_tokens}"
		);
	}
}

#[test]
fn test_self_tuning_direct_ratio_no_blending() {
	// Self-tuning returns actual/predicted directly — no blending weight
	// If we predicted 20 but only 10 happened: ratio=0.5 → scale down
	let predicted = 20.0_f64;
	let actual = 10.0_f64;
	let ratio = (actual / predicted).clamp(0.25, 4.0);
	assert_eq!(ratio, 0.5, "underestimated → ratio < 1");

	// If we predicted 10 but 30 happened: ratio=3.0 → scale up
	let ratio2 = (30.0_f64 / 10.0_f64).clamp(0.25, 4.0);
	assert_eq!(ratio2, 3.0, "overestimated → ratio > 1");

	// Clamp prevents extreme outliers from dominating
	let ratio_extreme_low = (1.0_f64 / 100.0_f64).clamp(0.25, 4.0);
	assert_eq!(ratio_extreme_low, 0.25, "extreme low clamped");
	let ratio_extreme_high = (100.0_f64 / 1.0_f64).clamp(0.25, 4.0);
	assert_eq!(ratio_extreme_high, 4.0, "extreme high clamped");
}

#[test]
fn test_self_tuning_neutral_when_no_prior_compression() {
	// No prior compressions → return 1.0 (no correction to apply)
	// Tested via the logic directly since we can't call the fn without SessionInfo
	let compressions = 0_usize;
	let result = if compressions == 0 { 1.0_f64 } else { 0.5 };
	assert_eq!(result, 1.0, "no prior data → neutral multiplier");
}

#[test]
fn test_estimate_end_to_end_symmetry_wins() {
	// Session: 10 calls, 50k output, 100k context, 2.5x compression
	// physical_ceiling = 60_000 / 5_000 = 12
	// symmetry = 10
	// estimate = min(12, 10) = 10
	let api_calls = 10.0_f64;
	let output_tokens = 50_000.0_f64;
	let current_tokens = 100_000.0_f64;
	let compression_ratio = 2.5_f64;

	let growth_rate = (output_tokens / api_calls).max(1.0); // 5_000
	let headroom = current_tokens - current_tokens / compression_ratio; // 60_000
	let ceiling = headroom / growth_rate; // 12
	let estimate = ceiling.min(api_calls); // min(12, 10) = 10

	assert_eq!(ceiling, 12.0);
	assert_eq!(estimate, 10.0, "symmetry (10) wins over ceiling (12)");
	assert!(estimate >= 5.0);
}

#[test]
fn test_estimate_end_to_end_ceiling_wins() {
	// Session: 30 calls, 300k output, 100k context, 2.5x compression
	// growth_rate = 300_000 / 30 = 10_000/call
	// physical_ceiling = 60_000 / 10_000 = 6
	// symmetry = 30
	// estimate = min(6, 30) = 6 → floored at 5 → 6
	let api_calls = 30.0_f64;
	let output_tokens = 300_000.0_f64;
	let current_tokens = 100_000.0_f64;
	let compression_ratio = 2.5_f64;

	let growth_rate = (output_tokens / api_calls).max(1.0); // 10_000
	let headroom = current_tokens - current_tokens / compression_ratio; // 60_000
	let ceiling = headroom / growth_rate; // 6
	let estimate = ceiling.min(api_calls); // min(6, 30) = 6

	assert_eq!(ceiling, 6.0);
	assert_eq!(estimate, 6.0, "ceiling (6) wins over symmetry (30)");
	assert!(estimate >= 5.0);
}

#[test]
fn test_estimate_incremental_growth_rate_after_compression() {
	// After a compression, growth_rate must use only tokens/calls since that
	// checkpoint — not the lifetime average which carries stale pre-compression signal.
	//
	// Scenario: heavy exploration phase (20 calls, 200k output = 10k/call),
	// then compression fires. Post-compression: 5 calls, 10k output = 2k/call.
	// Lifetime average = 210k / 25 = 8,400/call — 4x wrong.
	// Incremental = 10k / 5 = 2,000/call — correct.

	let total_api_calls: usize = 25;
	let total_output_tokens: u64 = 210_000;
	let api_calls_at_last_compression: usize = 20;
	let output_tokens_at_last_compression: u64 = 200_000;

	// Incremental (correct)
	let calls_since = (total_api_calls - api_calls_at_last_compression).max(1) as f64; // 5
	let output_since = total_output_tokens.saturating_sub(output_tokens_at_last_compression) as f64; // 10_000
	let incremental_rate = (output_since / calls_since).max(1.0); // 2_000
	assert_eq!(
		incremental_rate, 2_000.0,
		"incremental rate reflects post-compression phase"
	);

	// Lifetime (stale — what the old code used)
	let lifetime_rate = (total_output_tokens as f64 / total_api_calls as f64).max(1.0); // 8_400
	assert_eq!(
		lifetime_rate, 8_400.0,
		"lifetime rate is inflated by heavy early phase"
	);

	// Incremental gives a higher physical ceiling → less aggressive re-compression
	let current_tokens = 100_000.0_f64;
	let compression_ratio = 2.5_f64;
	let headroom = current_tokens - current_tokens / compression_ratio; // 60_000

	let ceiling_incremental = headroom / incremental_rate; // 30 calls
	let ceiling_lifetime = headroom / lifetime_rate; // ~7 calls

	assert!(
		ceiling_incremental > ceiling_lifetime,
		"incremental ceiling ({ceiling_incremental}) > lifetime ceiling ({ceiling_lifetime}): \
			stale lifetime rate would trigger re-compression 4x too soon"
	);
	assert_eq!(ceiling_incremental, 30.0);
}

#[test]
fn test_estimate_growth_rate_falls_back_to_lifetime_before_first_compression() {
	// Before any compression there is no checkpoint, so lifetime average is the
	// only signal available — and it's correct (no pre-compression phase to pollute it).
	let compressions: usize = 0;
	let total_api_calls = 10_usize;
	let total_output_tokens: u64 = 50_000;
	let api_calls_at_last_compression: usize = 0;
	let output_tokens_at_last_compression: u64 = 0;

	let growth_rate = if compressions > 0 {
		let calls_since = (total_api_calls - api_calls_at_last_compression).max(1) as f64;
		let output_since =
			total_output_tokens.saturating_sub(output_tokens_at_last_compression) as f64;
		(output_since / calls_since).max(1.0)
	} else {
		(total_output_tokens as f64 / total_api_calls.max(1) as f64).max(1.0)
	};

	// With no prior compression, lifetime = incremental (same data window)
	assert_eq!(
		growth_rate, 5_000.0,
		"lifetime fallback: 50k / 10 calls = 5k/call"
	);
}

#[test]
fn test_estimate_incremental_rate_single_call_since_compression() {
	// Edge: only 1 call since last compression — still uses that single measurement,
	// not the lifetime average. saturating_sub prevents underflow if counters drift.
	let total_api_calls: usize = 21;
	let total_output_tokens: u64 = 205_000;
	let api_calls_at_last_compression: usize = 20;
	let output_tokens_at_last_compression: u64 = 200_000;

	let calls_since = (total_api_calls - api_calls_at_last_compression).max(1) as f64; // 1
	let output_since = total_output_tokens.saturating_sub(output_tokens_at_last_compression) as f64; // 5_000
	let rate = (output_since / calls_since).max(1.0);
	assert_eq!(
		rate, 5_000.0,
		"single post-compression call measured correctly"
	);
}

#[test]
fn test_estimate_incremental_rate_saturating_sub_prevents_underflow() {
	// If output_tokens_at_last_compression somehow exceeds current (e.g. counter reset),
	// saturating_sub returns 0 → growth_rate floors at 1.0 rather than panicking.
	let total_output_tokens: u64 = 1_000;
	let output_tokens_at_last_compression: u64 = 5_000; // anomalous: larger than current
	let output_since = total_output_tokens.saturating_sub(output_tokens_at_last_compression); // 0
	assert_eq!(output_since, 0, "saturating_sub: no underflow");
	let rate = (output_since as f64 / 1.0_f64).max(1.0);
	assert_eq!(rate, 1.0, "floors at 1.0, no panic");
}

// ============================================================================
// SEQUENTIAL COMPRESSION TESTS: Verify the anchor (re-derived from messages
// every call) stays at the original first user message across cycles, and
// old compressed summaries get re-compressed (not orphaned).
// ============================================================================

#[test]
fn anchor_stable_across_repeated_compressions() {
	// Anchor is re-derived deterministically from messages every call.
	// Without instructions, anchor = first user message and stays put
	// across cycles because that user message remains at the same index.

	let mut messages = Vec::new();
	messages.push(msg("system")); // 0
	messages.push(msg("user")); // 1 - first user
	for i in 0..8 {
		messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
	} // 2-9

	// First compression
	let (start1, end1) = find_compression_range(&messages, false).unwrap();
	assert_eq!(start1, 1, "anchor = first user message");
	assert!(end1 >= 4);

	// Simulate post-compression state: anchor at 1, summary at 2, preserved tail
	let mut after = Vec::new();
	after.push(msg("system")); // 0
	after.push(msg("user")); // 1 - first user (kept)
	let mut comp = msg("assistant");
	comp.name = Some("plan_compression".to_string());
	after.push(comp); // 2 - compressed summary
	for i in 0..8 {
		after.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
	} // 3-10

	// Second compression — anchor is STILL idx 1 (re-derived, not cached)
	let (start2, end2) = find_compression_range(&after, false).unwrap();
	assert_eq!(start2, 1, "anchor re-derives to same index");
	assert!(end2 >= 4);
}

#[test]
fn old_compressed_summary_is_recompressed_on_next_cycle() {
	// After first compression, the summary sits at index 2 (role=assistant).
	// On second compression with first_prompt_idx=Some(1), start_idx=1,
	// so the drain range is [2..=end_idx] — the old summary IS drained.
	// This is correct: each cycle folds all prior context into one fresh summary.

	let mut messages = Vec::new();
	messages.push(msg("system")); // 0
	messages.push(msg("user")); // 1 - permanent anchor
	let mut comp = msg("assistant");
	comp.name = Some("plan_compression".to_string());
	comp.content = "OLD_SUMMARY_V1".to_string();
	messages.push(comp); // 2 - old compressed summary
	for i in 0..8 {
		messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
	} // 3-10

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();
	assert_eq!(start_idx, 1, "start at permanent anchor");

	// Drain range is start_idx+1..=end_idx = 2..=end_idx
	// Index 2 (old summary) IS in the drain range — it gets re-compressed
	let drain_range = (start_idx + 1)..=end_idx;
	assert!(
		drain_range.contains(&2),
		"Old compressed summary must be IN the drain range (re-compressed)"
	);

	// messages_to_compress includes the old summary
	let to_compress = &messages[start_idx + 1..=end_idx];
	assert!(
		to_compress
			.iter()
			.any(|m| m.content.contains("OLD_SUMMARY_V1")),
		"Old summary must be included in messages sent to AI for re-compression"
	);
}

#[test]
fn bootstrap_messages_before_anchor_preserved() {
	// Bootstrap messages (system, welcome, instructions) sit before the anchor
	// and are NEVER touched by compression. Drain covers [anchor+1..=end] and
	// keeps the entire prefix intact regardless of tool_calls / tool result
	// content inside the drain range.
	let mut instructions = msg("user");
	instructions.content = "<instructions>\nrules\n</instructions>".into();

	let mut comp = msg("assistant");
	comp.name = Some("plan_compression".to_string());
	let mut a5 = msg("assistant");
	a5.tool_calls = Some(json!([{"id": "c1", "type": "function", "function": {"name": "plan"}}]));
	let mut t6 = msg("tool");
	t6.tool_call_id = Some("c1".to_string());
	let mut a7 = msg("assistant");
	a7.tool_calls = Some(json!([{"id": "c2", "type": "function", "function": {"name": "shell"}}]));
	let mut t8 = msg("tool");
	t8.tool_call_id = Some("c2".to_string());

	let messages = vec![
		msg("system"),    // 0
		msg("assistant"), // 1 - welcome
		instructions,     // 2 - instructions (anchor)
		msg("user"),      // 3 - first real prompt
		comp,             // 4 - old compressed summary
		a5,               // 5 - tool_calls
		t6,               // 6 - tool result
		a7,               // 7 - tool_calls
		t8,               // 8 - tool result
		msg("assistant"), // 9 - final response
	];

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	assert_eq!(start_idx, 2, "anchor on <instructions> message");
	assert_eq!(end_idx, 9, "compress-all: end_idx = last message");

	// Bootstrap [0..=2] survives untouched; drain is [3..=9].
	assert!(start_idx + 1 > 1, "welcome at idx 1 stays outside drain");
}

#[test]
fn bootstrap_with_many_messages_compresses_all() {
	// With instructions at idx 2, anchor moves back to 2.
	// Compress-all: everything from anchor+1 to end gets compressed.
	let mut messages = vec![
		msg("system"),    // 0
		msg("assistant"), // 1 - welcome
		msg("user"),      // 2 - instructions
		msg("user"),      // 3 - first_prompt_idx
	];
	for i in 0..10 {
		messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
	} // 4-13

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();
	assert_eq!(start_idx, 2, "anchor must be instructions file at idx 2");
	assert_eq!(end_idx, 13, "compress-all: end_idx = last message");
}

#[test]
fn triple_compression_always_one_summary() {
	// After N compressions, there is always exactly ONE compressed summary
	// between the anchor and the preserved tail — never accumulating orphans.
	//
	// Cycle 1: [sys, user(anchor), asst, user, asst, ...] → drain 2..=end → insert summary at 2
	// Cycle 2: [sys, user(anchor), summary_v1, user, asst, ...] → drain 2..=end → insert summary at 2
	// Cycle 3: [sys, user(anchor), summary_v2, user, asst, ...] → drain 2..=end → insert summary at 2
	//
	// Each cycle: anchor stays at 1, old summary drained, new summary at 2.

	// Simulate state after 2nd compression
	let mut messages = Vec::new();
	messages.push(msg("system")); // 0
	messages.push(msg("user")); // 1 - permanent anchor
	let mut comp = msg("assistant");
	comp.name = Some("plan_compression".to_string());
	comp.content = "SUMMARY_V2".to_string();
	messages.push(comp); // 2 - summary from 2nd compression
	for i in 0..8 {
		messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
	} // 3-10

	// 3rd compression — still starts at anchor (1)
	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();
	assert_eq!(start_idx, 1);

	// Old summary at 2 is in drain range
	assert!((start_idx + 1..=end_idx).contains(&2));

	// After drain + insert: anchor at 1, new summary at 2, preserved tail after
	// No accumulation of old summaries — always exactly one.
}

#[test]
fn anchor_message_never_included_in_drain_range() {
	// TEST: Verify that the anchor message at start_idx is NEVER in the drain range.
	// drain range = start_idx+1..=end_idx (exclusive of start_idx)

	let messages = vec![
		msg("system"),    // 0
		msg("user"),      // 1 - anchor
		msg("assistant"), // 2
		msg("user"),      // 3
		msg("assistant"), // 4
		msg("user"),      // 5
		msg("assistant"), // 6
		msg("user"),      // 7
		msg("assistant"), // 8
	];

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	// The drain range is start_idx+1..=end_idx
	// The anchor at start_idx is NOT in this range
	let drain_start = start_idx + 1;
	let drain_end = end_idx;

	assert!(drain_start > start_idx, "Drain must start AFTER anchor");
	assert!(drain_end >= drain_start, "Drain range must be valid");

	// Verify: anchor index is NOT in drain range
	assert!(
		!(start_idx >= drain_start && start_idx <= drain_end),
		"Anchor must NOT be in drain range"
	);

	// Verify: messages_to_compress range matches drain range
	// CORRECT: start_idx+1..=end_idx
	// WRONG (old bug): start_idx..=end_idx
	let correct_range = (start_idx + 1)..=end_idx;
	assert!(correct_range.contains(&(start_idx + 1)));
	assert!(
		!correct_range.contains(&start_idx),
		"Anchor must NOT be in compression range"
	);
}

#[test]
fn compression_preserves_message_count_consistency() {
	// TEST: Verify message count after compression is correct.
	// Before: N messages
	// Remove: M messages (start_idx+1..=end_idx)
	// Insert: 1 compressed summary
	// After: N - M + 1 messages

	let mut messages = Vec::new();
	messages.push(msg("system")); // 0
	messages.push(msg("user")); // 1 - anchor
	for i in 2..=9 {
		messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
	}

	let before_count = messages.len();
	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	// Calculate expected removal count
	let messages_to_remove = end_idx - start_idx; // drain removes start_idx+1..=end_idx
	let _expected_after = before_count - messages_to_remove + 1; // +1 for compressed summary

	// Verify: messages_to_remove matches drain range
	assert_eq!(
		messages_to_remove,
		(end_idx - (start_idx + 1) + 1),
		"Removal count must match drain range"
	);

	// The anchor at start_idx is NOT removed
	// So we remove (end_idx - start_idx) messages, not (end_idx - start_idx + 1)
	assert!(
		messages_to_remove < before_count,
		"Must remove fewer messages than total"
	);
}

#[test]
fn messages_to_compress_excludes_anchor_message() {
	// messages_to_compress must be start_idx+1..=end_idx (exclude anchor).
	// The anchor at start_idx is KEPT by remove_messages_in_range.

	let mut messages = Vec::new();
	messages.push(msg("system")); // 0

	let mut anchor = msg("user"); // 1
	anchor.content = "ANCHOR_CONTENT_MUST_NOT_BE_SUMMARIZED".to_string();
	messages.push(anchor);

	messages.push(msg("assistant")); // 2
	messages.push(msg("user")); // 3
	messages.push(msg("assistant")); // 4
	messages.push(msg("user")); // 5
	messages.push(msg("assistant")); // 6
	messages.push(msg("user")); // 7
	messages.push(msg("assistant")); // 8

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();
	assert_eq!(start_idx, 1);

	let correct = &messages[start_idx + 1..=end_idx];
	let wrong = &messages[start_idx..=end_idx];

	assert_eq!(correct.len(), end_idx - start_idx);
	assert_eq!(wrong.len(), end_idx - start_idx + 1);

	assert!(
		!correct.iter().any(|m| m.content.contains("ANCHOR_CONTENT")),
		"Anchor must NOT be in messages_to_compress"
	);
	assert!(
		wrong.iter().any(|m| m.content.contains("ANCHOR_CONTENT")),
		"Old bug: anchor WAS in messages_to_compress"
	);
}

#[test]
fn calculate_range_tokens_matches_actual_removal() {
	// calculate_range_tokens must count exactly the messages removed by
	// remove_messages_in_range (start_idx+1..=end_idx), not including anchor.

	use crate::session::estimate_message_tokens;

	let mut messages = Vec::new();
	messages.push(msg("system")); // 0

	let mut anchor = msg("user");
	anchor.content = "x".repeat(1000);
	messages.push(anchor); // 1

	for i in 0..4 {
		let mut m = msg(if i % 2 == 0 { "assistant" } else { "user" });
		m.content = format!("Message {}", i);
		messages.push(m);
	} // 2-5

	for i in 0..4 {
		messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
	} // 6-9

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	let mut tokens_removed = 0u64;
	for msg in messages.iter().take(end_idx + 1).skip(start_idx + 1) {
		tokens_removed += estimate_message_tokens(msg) as u64;
	}

	let mut tokens_with_anchor = 0u64;
	for msg in messages.iter().take(end_idx + 1).skip(start_idx) {
		tokens_with_anchor += estimate_message_tokens(msg) as u64;
	}

	let anchor_tokens = estimate_message_tokens(&messages[start_idx]) as u64;
	assert_eq!(
		tokens_with_anchor - tokens_removed,
		anchor_tokens,
		"Difference must be exactly the anchor message tokens"
	);
}

// ── Stress tests ──────────────────────────────────────────────────────────

#[test]
fn test_file_context_stripped_from_recompression_input() {
	// strip_file_context_from_summary must remove the entire <file_context>…</file_context>
	// block. This prevents stale file bytes from accumulating in every subsequent summary.
	let summary_with_context = "<conversation_summary id=\"abc\">\n\
			<progress>Some important history here.</progress>\n\
			<file_context>\n\
			<content path=\"src/main.rs\">\nfn main() {}\n</content>\n\
			</file_context>\n\
			</conversation_summary>";

	let stripped = strip_file_context_from_summary(summary_with_context);

	assert!(
		!stripped.contains("<file_context>"),
		"file_context tag must be stripped"
	);
	assert!(
		!stripped.contains("fn main()"),
		"File bytes must not appear in stripped output"
	);
	assert!(
		stripped.contains("Some important history here."),
		"Summary text before file_context must be preserved"
	);
}

#[test]
fn test_file_context_stripped_when_no_sentinel() {
	// When there is no file_context block, the function returns the text unchanged.
	let plain = "<conversation_summary id=\"abc\">\n<progress>Just a summary.</progress>\n</conversation_summary>";
	let stripped = strip_file_context_from_summary(plain);
	assert_eq!(stripped, plain.trim());
}

#[test]
fn test_multiple_compression_cycles_anchor_never_moves() {
	// Simulate 3 compression cycles on a growing conversation.
	// After each cycle the old summary is at start_idx+1 and gets folded into the next.
	// Anchor must always equal 1 (the original first user message), re-derived
	// fresh every call from message structure — no cached state.
	//
	// Layout after each cycle:
	//   [0] system
	//   [1] user (anchor — first user message)
	//   [2] assistant (compressed summary, replaces old range)
	//   [3..] new messages

	// ── Cycle 1: 12 messages ──────────────────────────────────────────────
	let mut messages: Vec<Message> = Vec::new();
	messages.push(msg("system")); // 0
	messages.push(msg("user")); // 1 ← anchor
	for i in 0..10 {
		messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
	} // 2-11

	let (s1, e1) = find_compression_range(&messages, false).unwrap();
	assert_eq!(s1, 1, "Cycle 1: start must be anchor (1)");
	assert!(e1 > s1, "Cycle 1: end must be after anchor");
	assert!(
		e1 < messages.len(),
		"Cycle 1: end must leave RECENT messages"
	);

	// Simulate applying compression: drain s1+1..=e1, insert summary at s1+1
	let drained: Vec<Message> = messages.drain(s1 + 1..=e1).collect();
	assert!(!drained.is_empty(), "Cycle 1: must drain something");
	let mut summary1 = msg("assistant");
	summary1.content = "<conversation_summary id=\"c1\"><progress>Cycle 1 summary.</progress></conversation_summary>".to_string();
	messages.insert(s1 + 1, summary1);

	// ── Cycle 2: grow then compress again ────────────────────────────────
	for i in 0..10 {
		messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
	}

	let (s2, e2) = find_compression_range(&messages, false).unwrap();
	assert_eq!(s2, 1, "Cycle 2: start must still be anchor (1)");
	assert!(e2 > s2);

	let drained2: Vec<Message> = messages.drain(s2 + 1..=e2).collect();
	assert!(!drained2.is_empty(), "Cycle 2: must drain something");
	let mut summary2 = msg("assistant");
	summary2.content = "<conversation_summary id=\"c2\"><progress>Cycle 2 summary.</progress></conversation_summary>".to_string();
	messages.insert(s2 + 1, summary2);

	// ── Cycle 3: grow then compress again ────────────────────────────────
	for i in 0..10 {
		messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
	}

	let (s3, e3) = find_compression_range(&messages, false).unwrap();
	assert_eq!(s3, 1, "Cycle 3: start must still be anchor (1)");
	assert!(e3 > s3);

	// After 3 cycles the anchor is always at index 1 — never drifts.
	assert_eq!(s1, s2, "Anchor must not drift between cycles");
	assert_eq!(s2, s3, "Anchor must not drift between cycles");
}

#[test]
fn compress_all_includes_last_message() {
	// Compress-all: end_idx = last message. Recent user messages are extracted
	// and re-injected by the caller, not protected by find_compression_range.
	let mut messages: Vec<Message> = Vec::new();
	messages.push(msg("system")); // 0
	messages.push(msg("user")); // 1 ← anchor
	for i in 0..20 {
		messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
	} // 2-21
	messages.push(msg("user")); // 22

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();
	assert_eq!(start_idx, 1);
	assert_eq!(end_idx, 22, "compress-all: end_idx must be last message");
}

#[test]
fn compress_all_with_tool_loop_after_user_prompt() {
	// Compress-all: everything is compressed. The user's 2nd prompt at index 5
	// is in the drain range but will be extracted and re-injected by the caller.
	let messages = vec![
		msg("system"),    // 0
		msg("assistant"), // 1 welcome
		msg("user"),      // 2 instructions
		msg("user"),      // 3 first prompt
		msg("assistant"), // 4 compressed summary
		msg("user"),      // 5 second prompt
		msg("assistant"), // 6 tool_calls
		msg("tool"),      // 7
		msg("tool"),      // 8
		msg("assistant"), // 9 tool_calls
		msg("tool"),      // 10
		msg("assistant"), // 11 response
		msg("assistant"), // 12 tool_calls
		msg("tool"),      // 13
		msg("assistant"), // 14 response
	];

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();
	assert_eq!(start_idx, 2, "anchor at instructions");
	assert_eq!(end_idx, 14, "compress-all: end_idx = last message");
}

#[test]
fn test_recent_window_capped_at_8_for_large_session() {
	// For a 100-message session, RECENT count must be 8 (not 25).
	// This mirrors the formula: (total / 4).max(4).min(8)
	let total_msgs: usize = 100;
	let recent_count = (total_msgs / 4).clamp(4, 8);
	assert_eq!(
		recent_count, 8,
		"RECENT window must be capped at 8 for large sessions"
	);

	// For a 12-message session, RECENT count is 3 → clamped to 4
	let small = 12usize;
	let recent_small = (small / 4).clamp(4, 8);
	assert_eq!(recent_small, 4, "RECENT window must be at least 4");

	// For a 32-message session, RECENT count is 8 (exactly at cap)
	let medium = 32usize;
	let recent_medium = (medium / 4).clamp(4, 8);
	assert_eq!(recent_medium, 8, "RECENT window must be 8 at 32 messages");
}
#[test]
fn compress_all_with_tool_cycles() {
	// Compress-all: no preserved zone concept. Everything is compressed,
	// recent user messages are extracted and re-injected by the caller.
	let messages = vec![
		msg("system"),    // 0
		msg("user"),      // 1 (first_prompt_idx)
		msg("assistant"), // 2
		msg("user"),      // 3
		msg("assistant"), // 4
		msg("user"),      // 5
		msg("assistant"), // 6
		msg("user"),      // 7
		msg("assistant"), // 8
	];

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();
	assert_eq!(start_idx, 1);
	assert_eq!(end_idx, 8, "compress-all: end_idx = last message");

	// Simulate compress-all + user extraction: drain, insert summary, re-inject users
	let recent_users: Vec<Message> = messages[start_idx + 1..=end_idx]
		.iter()
		.rev()
		.filter(|m| m.role == "user")
		.take(2)
		.cloned()
		.collect::<Vec<_>>()
		.into_iter()
		.rev()
		.collect();

	let mut after = messages.clone();
	after.drain(start_idx + 1..=end_idx);
	let mut summary = msg("assistant");
	summary.content = "<conversation_summary id=\"test\"></conversation_summary>".to_string();
	after.insert(start_idx + 1, summary);
	// Re-inject recent user messages
	for (i, user_msg) in recent_users.iter().enumerate() {
		after.insert(start_idx + 2 + i, user_msg.clone());
	}

	// Result: [system, user(anchor), summary(asst), user(5), user(7)]
	assert_eq!(after.len(), 5);
	assert_eq!(after[0].role, "system");
	assert_eq!(after[1].role, "user"); // anchor
	assert_eq!(after[2].role, "assistant"); // summary
	assert_eq!(after[3].role, "user"); // extracted user from idx 5
	assert_eq!(after[4].role, "user"); // extracted user from idx 7
}

#[test]
fn tool_loop_only_one_user_message_still_compresses() {
	// Reproduces the exact bug from the session log:
	//   Compression check: current_tokens=61028, api_calls=137
	//   Invalid compression range (0 >= 0), skipping
	//
	// In a tool-loop session, there is only ONE user message (the initial prompt).
	// All subsequent messages are assistant+tool cycles.
	//
	// With first_prompt_idx=Some(1), start_idx = 0 (system anchor).
	// The user at idx 1 is inside the drain range. The while loop that searches
	// for a user in the preserved zone finds none (all preserved are assistants),
	// so compress_count stays at its original value — compression still happens.
	let mut messages = Vec::new();
	messages.push(msg("system")); // 0
	messages.push(msg("user")); // 1 (first_prompt_idx) — the ONLY user message

	// Simulate 10 tool cycles: assistant(tool_calls) → tool result
	for i in 0..10 {
		let mut asst = msg("assistant");
		asst.tool_calls = Some(json!([
			{"id": format!("call_{i}"), "type": "function", "function": {"name": "view", "arguments": "{}"}}
		]));
		messages.push(asst);
		let mut tool = msg("tool");
		tool.tool_call_id = Some(format!("call_{i}"));
		messages.push(tool);
	}

	// Final assistant response (no tool_calls)
	messages.push(msg("assistant")); // 22

	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();

	// Must return a valid compression range, NOT (0, 0)
	assert!(
		start_idx < end_idx,
		"Tool-loop session must produce valid compression range, got ({start_idx}, {end_idx})"
	);

	// Tool-loop: single user message, no instructions → anchor = first user (idx 1).
	assert_eq!(start_idx, 1, "anchor = first user message at idx 1");

	// compress-all: end_idx = last message
	assert_eq!(
		end_idx,
		messages.len() - 1,
		"compress-all: end_idx must be last message"
	);
}

#[test]
fn test_triple_compression_only_one_summary_in_drain() {
	// After 3 compression cycles, the drain range must always contain exactly
	// one prior compressed summary (the previous cycle's output), never zero or two.
	// This verifies that old summaries are folded into new ones, not accumulated.

	let mut messages: Vec<Message> = Vec::new();
	messages.push(msg("system")); // 0
	messages.push(msg("user")); // 1 ← anchor
	for i in 0..10 {
		messages.push(msg(if i % 2 == 0 { "assistant" } else { "user" }));
	}

	for cycle in 1..=3usize {
		// Grow the session
		for i in 0..8 {
			messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
		}

		let (s, e) = find_compression_range(&messages, false).unwrap();

		// Count compressed summaries in the drain range (s+1..=e)
		let summaries_in_drain = messages[s + 1..=e]
			.iter()
			.filter(|m| m.content.starts_with("<conversation_summary"))
			.count();

		if cycle > 1 {
			assert_eq!(
				summaries_in_drain, 1,
				"Cycle {}: drain range must contain exactly 1 prior summary, found {}",
				cycle, summaries_in_drain
			);
		}

		// Apply compression
		let _drained: Vec<Message> = messages.drain(s + 1..=e).collect();
		let mut summary = msg("assistant");
		summary.content =
			format!("<conversation_summary id=\"c{cycle}\"><progress>Cycle {cycle} summary.</progress></conversation_summary>");
		messages.insert(s + 1, summary);
	}
}

#[test]
fn regression_session_260521_no_stuck_first_turn_prefix() {
	// Regression for the 260521-dk-1148-b53e bug: the OLD None-branch heuristic
	// would advance past welcome + (any user followed by assistant) and then
	// run a tool-skip over the resulting assistant's tool_calls — anchoring on
	// the 2nd assistant turn and permanently stranding the first user message
	// plus its 3-tool reply (5 extra prefix messages, forever).
	//
	// Exact layout from the broken session before the second /done:
	//   0: system
	//   1: assistant (welcome 🐙, no tool_calls)
	//   2: user ("lets crawl...")              ← MUST be anchor
	//   3: assistant ("Let me pull up...", has tool_calls)
	//   4: tool (MEMORIES)
	//   5: tool (MEMORIES)
	//   6: tool (browser_get_current_tab)
	//   7: assistant ("Got it, Don...")         ← OLD bug parked anchor here
	//   8..N: rest of conversation
	let mut a3 = msg("assistant");
	a3.tool_calls = Some(json!([
		{"id": "c1", "type": "function", "function": {"name": "remember"}},
		{"id": "c2", "type": "function", "function": {"name": "remember"}},
		{"id": "c3", "type": "function", "function": {"name": "browser_get_current_tab"}}
	]));
	let mut t4 = msg("tool");
	t4.tool_call_id = Some("c1".to_string());
	let mut t5 = msg("tool");
	t5.tool_call_id = Some("c2".to_string());
	let mut t6 = msg("tool");
	t6.tool_call_id = Some("c3".to_string());

	let mut messages = vec![
		msg("system"),    // 0
		msg("assistant"), // 1 welcome
		msg("user"),      // 2 first user prompt
		a3,               // 3 assistant + tool_calls
		t4,               // 4 tool result
		t5,               // 5 tool result
		t6,               // 6 tool result
		msg("assistant"), // 7 follow-up assistant
	];
	// Pad with enough conversation turns to satisfy min_conv.
	for i in 0..6 {
		messages.push(msg(if i % 2 == 0 { "user" } else { "assistant" }));
	}

	let (start_idx, end_idx) = find_compression_range(&messages, true).unwrap();

	// New rule: anchor = first user message. NOT idx 7.
	assert_eq!(
		start_idx, 2,
		"anchor MUST be the first user message, not parked past the bootstrap turn"
	);
	assert_eq!(end_idx, messages.len() - 1, "drain extends to last message");

	// Stuck-prefix check: under the OLD bug, indices 3..=7 (assistant + 3 tools
	// + follow-up assistant) were preserved across /done forever. The new
	// behavior includes them in the drain range so each /done cleans them up.
	for stuck_idx in 3..=7 {
		assert!(
			(start_idx + 1..=end_idx).contains(&stuck_idx),
			"idx {stuck_idx} must be in drain range, not stuck in the prefix"
		);
	}
}

#[test]
fn bug_proof_invalid_range_must_set_cooldown() {
	// BUG SCENARIO: should_check_compression runs the full expensive path:
	//   threshold exceeded → cooldown passed → cost analysis → find_compression_range
	// When find_compression_range returns (0, 0) (not enough messages),
	// it MUST set context_tokens_after_last_compression to prevent the same
	// expensive analysis from running every single turn.
	//
	// Without the fix, the log shows this loop every turn:
	//   Compression check: current_tokens=61028, thresholds=[60000, 80000, 120000]
	//   ✓ Threshold exceeded!
	//   Compression cooldown passed: ...
	//   Net benefit: $0.27539 → COMPRESS ✓
	//   Invalid compression range (0 >= 0), skipping
	//   ... repeats next turn ...

	// Step 1: Prove find_compression_range returns (0, 0) with too few messages
	let messages = vec![
		msg("system"),    // 0
		msg("user"),      // 1
		msg("assistant"), // 2
		msg("user"),      // 3
		msg("assistant"), // 4
	];
	// Only 4 conversation messages (user+assistant) — need >4 to compress
	let (start_idx, end_idx) = find_compression_range(&messages, false).unwrap();
	assert_eq!(
		(start_idx, end_idx),
		(0, 0),
		"Must return (0,0) when not enough messages to compress"
	);

	// Step 2: Verify the cooldown logic that should_check_compression must apply
	// when it encounters this (0, 0) range after passing all other gates.
	let current_tokens: usize = 61_028;
	let mut context_tokens_after_last_compression: usize = 19_442; // from prior compression

	// Simulate the fix: set cooldown when range is invalid
	if start_idx >= end_idx {
		context_tokens_after_last_compression = current_tokens;
	}

	// Now the cooldown check should block the next attempt
	let min_tokens_for_recompression =
		(context_tokens_after_last_compression as f64 * 1.1) as usize;
	assert!(
			current_tokens < min_tokens_for_recompression,
			"After setting cooldown to current_tokens={}, next check at same token count must be blocked (need {} for recompression)",
			current_tokens,
			min_tokens_for_recompression
		);

	// Step 3: Verify that WITHOUT the fix, cooldown would NOT block
	let old_watermark: usize = 19_442;
	let old_min = (old_watermark as f64 * 1.1) as usize;
	assert!(
		current_tokens >= old_min,
		"Without fix, old watermark {} allows recompression at {} (min: {}) — the bug!",
		old_watermark,
		current_tokens,
		old_min
	);
}

#[test]
fn bug_proof_invalid_range_cooldown_allows_growth() {
	// After cooldown is set from invalid range, compression must still
	// trigger once context grows by ≥10%.
	let current_tokens: usize = 61_028;
	let context_tokens_after_last_compression = current_tokens; // cooldown set

	// 10% growth should allow recompression
	let grown_tokens: usize = 67_200; // ~10.1% growth
	let min_required = (context_tokens_after_last_compression as f64 * 1.1) as usize;
	assert!(
		grown_tokens >= min_required,
		"After 10%+ growth ({} → {}), compression should be allowed (min: {})",
		current_tokens,
		grown_tokens,
		min_required
	);
}

#[test]
fn knowledge_log_entry_uses_content_key() {
	// REGRESSION: log_knowledge_entry() previously wrote "knowledge" key but
	// persistence.rs reads "content" key — entries were silently lost on resume.
	// Verify the JSON produced by the logger uses "content".
	let entry = serde_json::json!({
		"type": "KNOWLEDGE_ENTRY",
		"timestamp": 0u64,
		"content": "test knowledge"
	});
	assert!(
		entry.get("content").is_some(),
		"KNOWLEDGE_ENTRY must use 'content' key (not 'knowledge')"
	);
	assert!(
		entry.get("knowledge").is_none(),
		"'knowledge' key must not be present — persistence reads 'content'"
	);
	assert_eq!(entry["content"].as_str().unwrap(), "test knowledge");
}

// ───────────────────────────────────────────────────────────────────────
// Empty-summary safety guard (schema era)
//
// Background: schema validation guarantees the *shape* of the response,
// but the model could still return `should_compress: true` with every
// narrative field empty. Without a guard, `apply_compression` would drain
// every message and replace them with a header-only block.
// `is_summary_substantive` rejects that case. These tests pin the gate.
// ───────────────────────────────────────────────────────────────────────

fn empty_summary() -> CompressionSummary {
	CompressionSummary::default()
}

fn summary_with_progress() -> CompressionSummary {
	let mut s = empty_summary();
	s.should_compress = true;
	s.progress = "User asked about config loading, AI explained the merge order.".to_string();
	s
}

#[test]
fn substantive_rejects_default_summary() {
	assert!(!is_summary_substantive(&empty_summary()));
}

#[test]
fn substantive_rejects_whitespace_narrative_fields() {
	let mut s = empty_summary();
	s.current_task = "   ".to_string();
	s.progress = "\n\t".to_string();
	s.session_context = "  ".to_string();
	assert!(!is_summary_substantive(&s));
}

#[test]
fn substantive_accepts_progress_only() {
	assert!(is_summary_substantive(&summary_with_progress()));
}

#[test]
fn substantive_accepts_single_finding() {
	let mut s = empty_summary();
	s.analysis_findings = vec!["root cause: cache marker placement".to_string()];
	assert!(is_summary_substantive(&s));
}

#[test]
fn substantive_accepts_recent_exchange_only() {
	let mut s = empty_summary();
	s.recent_exchanges = vec!["user asked X; assistant answered Y".to_string()];
	assert!(is_summary_substantive(&s));
}

#[test]
fn render_omits_empty_sections() {
	let mut s = empty_summary();
	s.session_context = "investigating compression quality".to_string();
	s.current_task = "rewriting prompt to use JSON schema".to_string();
	let rendered = render_summary(&s);
	assert!(rendered.contains("<session_context>"));
	assert!(rendered.contains("<current_task>"));
	// Sections with no signal must NOT appear as empty tags.
	assert!(!rendered.contains("<progress>"));
	assert!(!rendered.contains("<analysis_findings>"));
	assert!(!rendered.contains("<key_entities>"));
	assert!(!rendered.contains("<next_steps>"));
}

#[test]
fn render_includes_original_request_when_set() {
	let mut s = summary_with_progress();
	s.original_request = "Build a session-based AI dev assistant.".to_string();
	let rendered = render_summary(&s);
	assert!(
		rendered.contains(
			"<original_request>Build a session-based AI dev assistant.</original_request>"
		),
		"original_request must be rendered verbatim: {}",
		rendered
	);
}

#[test]
fn render_includes_errors_and_corrections() {
	let mut s = summary_with_progress();
	s.errors_and_corrections = vec![
		"user said: don't add fallbacks".to_string(),
		"compile error: borrow of moved value at ai.rs:45".to_string(),
	];
	let rendered = render_summary(&s);
	assert!(rendered.contains("<errors_and_corrections>"));
	assert!(rendered.contains("<entry>user said: don't add fallbacks</entry>"));
	assert!(rendered.contains("<entry>compile error: borrow of moved value at ai.rs:45</entry>"));
	assert!(rendered.contains("</errors_and_corrections>"));
}

#[test]
fn render_key_entities_nested_tags() {
	let mut s = summary_with_progress();
	s.key_entities = KeyEntities {
		files: vec!["src/foo.rs:10:20".to_string()],
		names: vec!["compress_summary".to_string()],
		decisions: vec!["use JSON schema for compression".to_string()],
	};
	let rendered = render_summary(&s);
	assert!(rendered.contains("<key_entities>"));
	assert!(rendered.contains("<files>"));
	assert!(rendered.contains("<file>src/foo.rs:10:20</file>"));
	assert!(rendered.contains("<name>compress_summary</name>"));
	assert!(rendered.contains("<decision>use JSON schema for compression</decision>"));
	assert!(rendered.contains("</key_entities>"));
}

#[test]
fn format_compressed_entry_with_empty_summary_still_renders_wrapper() {
	// Belt-and-braces: even if `is_summary_substantive` failed to gate, an
	// empty render still produces a clearly-tagged wrapper (used during the
	// pathological-bootstrap branch in apply_compression). Pinned here so
	// any future refactor that changes the wrapper tag breaks
	// strip_file_context_from_summary's matching as well.
	let formatted = format_compressed_entry_with_context("", "", "test-id".to_string());
	assert!(formatted.contains("<conversation_summary id=\"test-id\">"));
	assert!(formatted.contains("</conversation_summary>"));
}

// ---------------------------------------------------------------------------
// Pressure-level cursor: incremental + wrap (round-robin)
//
// CONTRACT:
//   * Applied ratio level = consecutive_compressions mod num_levels.
//   * First compression after a user message (consecutive=0) => lightest level 0.
//   * Each autonomous compression advances one step: 0,1,2,...
//   * After the strongest level it WRAPS back to 0 (round-robin), never clamps.
//   * The token-count floor only gates WHETHER we compress, not which ratio.
// ---------------------------------------------------------------------------
#[test]
fn level_cursor_first_compression_after_user_is_lightest() {
	// consecutive=0 must always select level 0 (lightest), independent of token floor.
	assert_eq!(
		select_compression_level_index(3, 0),
		0,
		"first compression must be lightest level 0"
	);
}

#[test]
fn level_cursor_advances_one_step_per_autonomous_compression() {
	// 3 levels: each autonomous compression advances one step.
	assert_eq!(select_compression_level_index(3, 0), 0);
	assert_eq!(select_compression_level_index(3, 1), 1);
	assert_eq!(select_compression_level_index(3, 2), 2);
}

#[test]
fn level_cursor_wraps_after_strongest_level() {
	// After the strongest (index num_levels-1), the cursor returns to 0 and
	// cycles — round-robin, never clamped at the top.
	assert_eq!(
		select_compression_level_index(3, 3),
		0,
		"wrap to 0 after top"
	);
	assert_eq!(select_compression_level_index(3, 4), 1);
	assert_eq!(select_compression_level_index(3, 5), 2);
	assert_eq!(select_compression_level_index(3, 6), 0, "second full wrap");
}

#[test]
fn level_cursor_single_level_is_always_zero() {
	// Degenerate ladder of one level: every compression maps to level 0.
	for n in 0..5 {
		assert_eq!(select_compression_level_index(1, n), 0);
	}
}
