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

//! Test suite for Ctrl+C cleanup behavior in different conversation states
//!
//! This test simulates the cleanup logic from main_loop.rs to ensure:
//! 1. Idle/ReadingInput: No cleanup
//! 2. First API call (no tools): Remove user message
//! 3. Multi-turn (tools executed): Preserve everything
//! 4. CompletedWithResults: No cleanup

use octomind::session::Message;

#[derive(Debug, Clone, PartialEq)]
enum ProcessingState {
	Idle,
	ReadingInput,
	CallingAPI,
	CompletedWithResults,
}

#[derive(Debug, Clone)]
struct OperationContext {
	user_message_index: Option<usize>,
}

struct MockSession {
	messages: Vec<Message>,
}

impl MockSession {
	fn new() -> Self {
		Self { messages: Vec::new() }
	}

	fn add_user_message(&mut self, content: &str) {
		self.messages.push(Message {
			role: "user".to_string(),
			content: content.to_string(),
			..Default::default()
		});
	}

	fn add_assistant_message(&mut self, content: &str) {
		self.messages.push(Message {
			role: "assistant".to_string(),
			content: content.to_string(),
			..Default::default()
		});
	}

	fn add_tool_message(&mut self, content: &str) {
		self.messages.push(Message {
			role: "tool".to_string(),
			content: content.to_string(),
			..Default::default()
		});
	}

	fn message_count(&self) -> usize {
		self.messages.len()
	}

	fn has_tool_messages_after(&self, index: usize) -> bool {
		self.messages.iter().skip(index).any(|msg| msg.role == "tool")
	}
}

/// Simulates the Ctrl+C cleanup logic from main_loop.rs
fn simulate_ctrl_c_cleanup(session: &mut MockSession, state: ProcessingState, operation: Option<OperationContext>) {
	match state {
		ProcessingState::Idle | ProcessingState::ReadingInput => {
			// Nothing to clean up - just reset and continue
			println!("✓ Idle/ReadingInput: No cleanup needed");
		}
		ProcessingState::CallingAPI => {
			// API call was interrupted - determine if we're in multi-turn conversation
			if let Some(op) = operation {
				// Check if there are tool messages AFTER the user message for this operation
				let user_idx = op.user_message_index.unwrap_or(0);
				let has_tool_messages = session.has_tool_messages_after(user_idx);

				if has_tool_messages {
					// MULTI-TURN: Tools were executed, conversation state is valid
					println!("✓ Multi-turn: Preserving all conversation state");
				} else {
					// FIRST CALL: No tools executed yet
					// Remove user message (and assistant if added) for clean retry
					if let Some(user_idx) = op.user_message_index {
						if user_idx < session.messages.len() {
							session.messages.truncate(user_idx);
							println!("✓ First call: Removed user message for clean retry");
						}
					}
				}
			}
		}
		ProcessingState::CompletedWithResults => {
			// Operation completed successfully - nothing to clean up
			println!("✓ Completed: All work preserved");
		}
	}
}

#[test]
fn test_ctrl_c_during_idle() {
	println!("\n=== TEST: Ctrl+C during Idle ===");
	let mut session = MockSession::new();
	session.add_user_message("Previous message");
	session.add_assistant_message("Previous response");

	let initial_count = session.message_count();
	simulate_ctrl_c_cleanup(&mut session, ProcessingState::Idle, None);

	assert_eq!(
		session.message_count(),
		initial_count,
		"Idle state should not remove any messages"
	);
	println!("✅ PASS: No messages removed during idle\n");
}

#[test]
fn test_ctrl_c_during_reading_input() {
	println!("\n=== TEST: Ctrl+C during ReadingInput ===");
	let mut session = MockSession::new();
	session.add_user_message("Previous message");
	session.add_assistant_message("Previous response");

	let initial_count = session.message_count();
	simulate_ctrl_c_cleanup(&mut session, ProcessingState::ReadingInput, None);

	assert_eq!(
		session.message_count(),
		initial_count,
		"ReadingInput state should not remove any messages"
	);
	println!("✅ PASS: No messages removed during input reading\n");
}

#[test]
fn test_ctrl_c_during_first_api_call_before_response() {
	println!("\n=== TEST: Ctrl+C during first API call (before response) ===");
	let mut session = MockSession::new();
	session.add_user_message("Previous message");
	session.add_assistant_message("Previous response");
	let user_idx = session.message_count();
	session.add_user_message("New question"); // This should be removed

	let operation = Some(OperationContext {
		user_message_index: Some(user_idx),
	});

	simulate_ctrl_c_cleanup(&mut session, ProcessingState::CallingAPI, operation);

	assert_eq!(
		session.message_count(),
		user_idx,
		"Should remove user message when cancelled before response"
	);
	assert_eq!(
		session.messages.last().unwrap().role,
		"assistant",
		"Last message should be previous assistant response"
	);
	println!("✅ PASS: User message removed for clean retry\n");
}

#[test]
fn test_ctrl_c_during_first_api_call_after_response_started() {
	println!("\n=== TEST: Ctrl+C during first API call (after response started) ===");
	let mut session = MockSession::new();
	session.add_user_message("Previous message");
	session.add_assistant_message("Previous response");
	let user_idx = session.message_count();
	session.add_user_message("New question");
	session.add_assistant_message("Partial response..."); // Started responding but no tools

	let operation = Some(OperationContext {
		user_message_index: Some(user_idx),
	});

	simulate_ctrl_c_cleanup(&mut session, ProcessingState::CallingAPI, operation);

	assert_eq!(
		session.message_count(),
		user_idx,
		"Should remove both user and partial assistant message"
	);
	assert_eq!(
		session.messages.last().unwrap().role,
		"assistant",
		"Last message should be previous assistant response"
	);
	println!("✅ PASS: User and partial assistant messages removed\n");
}

#[test]
fn test_ctrl_c_during_multiturn_after_tools_executed() {
	println!("\n=== TEST: Ctrl+C during multi-turn (after tools executed) ===");
	let mut session = MockSession::new();
	session.add_user_message("Previous message");
	session.add_assistant_message("Previous response");
	let user_idx = session.message_count();
	session.add_user_message("New question");
	session.add_assistant_message("Let me check that...");
	session.add_tool_message("Tool result 1");
	session.add_tool_message("Tool result 2");
	// AI is now making follow-up call to process tool results
	// User presses Ctrl+C during this follow-up call

	let initial_count = session.message_count();
	let operation = Some(OperationContext {
		user_message_index: Some(user_idx),
	});

	simulate_ctrl_c_cleanup(&mut session, ProcessingState::CallingAPI, operation);

	assert_eq!(
		session.message_count(),
		initial_count,
		"Multi-turn: Should preserve ALL messages (user + assistant + tools)"
	);
	assert_eq!(
		session.messages.last().unwrap().role,
		"tool",
		"Last message should still be tool result"
	);
	println!("✅ PASS: All conversation state preserved during multi-turn\n");
}

#[test]
fn test_ctrl_c_during_multiturn_with_partial_followup_response() {
	println!("\n=== TEST: Ctrl+C during multi-turn (partial follow-up response) ===");
	let mut session = MockSession::new();
	session.add_user_message("Previous message");
	session.add_assistant_message("Previous response");
	let user_idx = session.message_count();
	session.add_user_message("New question");
	session.add_assistant_message("Let me check that...");
	session.add_tool_message("Tool result 1");
	session.add_tool_message("Tool result 2");
	session.add_assistant_message("Based on the results..."); // Follow-up response started
														   // User presses Ctrl+C during follow-up response

	let initial_count = session.message_count();
	let operation = Some(OperationContext {
		user_message_index: Some(user_idx),
	});

	simulate_ctrl_c_cleanup(&mut session, ProcessingState::CallingAPI, operation);

	assert_eq!(
		session.message_count(),
		initial_count,
		"Multi-turn: Should preserve ALL messages including partial follow-up"
	);
	assert_eq!(
		session.messages.last().unwrap().role,
		"assistant",
		"Last message should be partial follow-up response"
	);
	println!("✅ PASS: All conversation state preserved including partial follow-up\n");
}

#[test]
fn test_ctrl_c_after_completion() {
	println!("\n=== TEST: Ctrl+C after completion ===");
	let mut session = MockSession::new();
	session.add_user_message("Question");
	session.add_assistant_message("Complete answer");

	let initial_count = session.message_count();
	simulate_ctrl_c_cleanup(&mut session, ProcessingState::CompletedWithResults, None);

	assert_eq!(
		session.message_count(),
		initial_count,
		"Completed state should not remove any messages"
	);
	println!("✅ PASS: No messages removed after completion\n");
}

#[test]
fn test_ctrl_c_complex_multiturn_scenario() {
	println!("\n=== TEST: Ctrl+C in complex multi-turn scenario ===");
	let mut session = MockSession::new();

	// Previous conversation
	session.add_user_message("Previous question");
	session.add_assistant_message("Previous answer");

	// New request that triggers multi-turn
	let user_idx = session.message_count();
	session.add_user_message("Complex task requiring multiple tools");
	session.add_assistant_message("I'll use several tools...");
	session.add_tool_message("Tool 1 result");
	session.add_tool_message("Tool 2 result");
	session.add_tool_message("Tool 3 result");
	// AI makes follow-up call to process results
	// User presses Ctrl+C during this call

	let initial_count = session.message_count();
	let operation = Some(OperationContext {
		user_message_index: Some(user_idx),
	});

	simulate_ctrl_c_cleanup(&mut session, ProcessingState::CallingAPI, operation);

	assert_eq!(
		session.message_count(),
		initial_count,
		"Complex multi-turn: Should preserve all messages"
	);

	// Verify conversation structure is intact
	let messages_after_user = &session.messages[user_idx..];
	assert_eq!(messages_after_user[0].role, "user");
	assert_eq!(messages_after_user[1].role, "assistant");
	assert_eq!(messages_after_user[2].role, "tool");
	assert_eq!(messages_after_user[3].role, "tool");
	assert_eq!(messages_after_user[4].role, "tool");

	println!("✅ PASS: Complex multi-turn conversation preserved correctly\n");
}

#[test]
fn test_ctrl_c_edge_case_empty_session() {
	println!("\n=== TEST: Ctrl+C with empty session ===");
	let mut session = MockSession::new();
	let user_idx = session.message_count();
	session.add_user_message("First message");

	let operation = Some(OperationContext {
		user_message_index: Some(user_idx),
	});

	simulate_ctrl_c_cleanup(&mut session, ProcessingState::CallingAPI, operation);

	assert_eq!(
		session.message_count(),
		0,
		"Should remove first message on cancellation"
	);
	println!("✅ PASS: First message removed correctly\n");
}

#[test]
fn test_ctrl_c_no_operation_context() {
	println!("\n=== TEST: Ctrl+C with no operation context ===");
	let mut session = MockSession::new();
	session.add_user_message("Message");

	let initial_count = session.message_count();
	simulate_ctrl_c_cleanup(&mut session, ProcessingState::CallingAPI, None);

	assert_eq!(
		session.message_count(),
		initial_count,
		"No operation context: Should not remove any messages"
	);
	println!("✅ PASS: No cleanup without operation context\n");
}
