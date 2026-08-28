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
use reedline::PromptViMode;

use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn prompt() -> (ChatPrompt, Arc<AtomicBool>) {
	let reverse_search_active = Arc::new(AtomicBool::new(false));
	let prompt = ChatPrompt::new(
		"octomind".to_string(),
		"▍ 〉".to_string(),
		reverse_search_active.clone(),
	);
	(prompt, reverse_search_active)
}

#[test]
fn new_exposes_left_text_and_indicator() {
	let (prompt, _) = prompt();
	assert_eq!(prompt.render_prompt_left(), "octomind");
	assert_eq!(
		prompt.render_prompt_indicator(PromptEditMode::Default),
		"▍ 〉"
	);
}

#[test]
fn right_prompt_is_empty() {
	let (prompt, _) = prompt();
	assert_eq!(prompt.render_prompt_right(), "");
}

#[test]
fn indicator_resets_reverse_search_flag() {
	let (prompt, flag) = prompt();
	flag.store(true, Ordering::SeqCst);
	prompt.render_prompt_indicator(PromptEditMode::Emacs);
	assert!(!flag.load(Ordering::SeqCst));
}

#[test]
fn indicator_is_same_for_every_edit_mode() {
	let (prompt, _) = prompt();
	let default = prompt.render_prompt_indicator(PromptEditMode::Default);
	assert_eq!(
		prompt.render_prompt_indicator(PromptEditMode::Emacs),
		default
	);
	assert_eq!(
		prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Normal)),
		default
	);
}

#[test]
fn multiline_indicator_carries_the_prompt_rail() {
	let (prompt, _) = prompt();
	let multiline = prompt.render_prompt_multiline_indicator();
	assert!(
		multiline.contains('▍'),
		"continuation rail missing: {multiline}"
	);
}

#[test]
fn history_search_indicator_sets_flag_and_shows_term() {
	let (prompt, flag) = prompt();
	let search = PromptHistorySearch::new(PromptHistorySearchStatus::Passing, "query".to_string());
	let rendered = prompt.render_prompt_history_search_indicator(search);
	assert!(flag.load(Ordering::SeqCst));
	assert_eq!(rendered, "(search: query) ");
}

#[test]
fn history_search_indicator_shows_term_also_when_failing() {
	let (prompt, _) = prompt();
	let search = PromptHistorySearch::new(PromptHistorySearchStatus::Failing, "nope".to_string());
	let rendered = prompt.render_prompt_history_search_indicator(search);
	assert!(rendered.contains("nope"), "search term missing: {rendered}");
}
