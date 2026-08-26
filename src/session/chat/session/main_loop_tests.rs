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

fn msg(role: &str) -> Message {
	Message {
		role: role.to_string(),
		..Default::default()
	}
}

fn msgs(roles: &[&str]) -> Vec<Message> {
	roles.iter().map(|r| msg(r)).collect()
}

#[test]
fn test_first_call_truncates_to_user_message() {
	// User message added, API call interrupted before any tool ran →
	// remove the user message for a clean retry.
	let messages = msgs(&["system", "user"]);
	assert_eq!(interrupted_call_truncation(&messages, Some(1)), Some(1));

	// Assistant text may already be streaming — still no tools → truncate.
	let messages = msgs(&["system", "user", "assistant"]);
	assert_eq!(interrupted_call_truncation(&messages, Some(1)), Some(1));
}

#[test]
fn test_multiturn_with_tools_preserves_everything() {
	// Tool results after the user message: truncating would orphan the
	// assistant(tool_calls) + tool_result pairing the API already accepted.
	let messages = msgs(&["system", "user", "assistant", "tool"]);
	assert_eq!(interrupted_call_truncation(&messages, Some(1)), None);
}

#[test]
fn test_tools_from_previous_turns_do_not_count() {
	// A tool message BEFORE this operation's user message belongs to a prior
	// turn — the current operation is still a clean first call.
	let messages = msgs(&["user", "assistant", "tool", "user"]);
	assert_eq!(interrupted_call_truncation(&messages, Some(3)), Some(3));
}

#[test]
fn test_missing_or_stale_index_preserves_state() {
	let messages = msgs(&["system", "user"]);
	// No operation context → nothing to truncate
	assert_eq!(interrupted_call_truncation(&messages, None), None);
	// Index at/past the end (already rolled back elsewhere) → no-op
	assert_eq!(interrupted_call_truncation(&messages, Some(2)), None);
	assert_eq!(interrupted_call_truncation(&messages, Some(99)), None);
	// Empty session
	assert_eq!(interrupted_call_truncation(&[], Some(0)), None);
}

#[test]
fn test_clipboard_image_refused_for_known_non_vision_model() {
	use crate::session::chat::reedline_adapter::PendingClipboardItem;
	use crate::session::image::{ImageAttachment, ImageData, SourceType};

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openai:gpt-3.5-turbo".to_string();
	let attachment = ImageAttachment {
		data: ImageData::Base64("unused".to_string()),
		media_type: "image/png".to_string(),
		source_type: SourceType::Clipboard,
		dimensions: Some((1, 1)),
		size_bytes: None,
	};

	apply_clipboard_items(&mut session, vec![PendingClipboardItem::Image(attachment)]);
	assert!(!session.has_pending_image());
}

#[test]
fn test_clipboard_image_attached_for_unknown_proxy_model() {
	use crate::session::chat::reedline_adapter::PendingClipboardItem;
	use crate::session::image::{ImageAttachment, ImageData, SourceType};

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/totally-unknown-model-xyz".to_string();
	let attachment = ImageAttachment {
		data: ImageData::Base64("unused".to_string()),
		media_type: "image/png".to_string(),
		source_type: SourceType::Clipboard,
		dimensions: Some((1, 1)),
		size_bytes: None,
	};

	apply_clipboard_items(&mut session, vec![PendingClipboardItem::Image(attachment)]);
	assert!(session.has_pending_image());
}
