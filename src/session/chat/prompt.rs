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

use reedline::{Prompt, PromptEditMode, PromptHistorySearch};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct ChatPrompt {
	left: String,
	indicator: String,
	multiline: String,
	reverse_search_active: Arc<AtomicBool>,
}

impl ChatPrompt {
	pub fn new(left: String, indicator: String, reverse_search_active: Arc<AtomicBool>) -> Self {
		Self {
			left,
			indicator,
			// 4 spaces aligns wrapped lines under the input text. The indicator
			// `▍ 〉` is 1 (▍) + 1 (space) + 2 (〉, East-Asian-Wide) = 4 cells.
			multiline: "    ".to_string(),
			reverse_search_active,
		}
	}
}

impl Prompt for ChatPrompt {
	fn render_prompt_left(&self) -> Cow<'_, str> {
		Cow::Owned(self.left.clone())
	}

	fn render_prompt_right(&self) -> Cow<'_, str> {
		Cow::Borrowed("")
	}

	fn render_prompt_indicator(&self, _prompt_mode: PromptEditMode) -> Cow<'_, str> {
		self.reverse_search_active.store(false, Ordering::SeqCst);
		Cow::Owned(self.indicator.clone())
	}

	fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
		Cow::Owned(self.multiline.clone())
	}

	fn render_prompt_history_search_indicator(
		&self,
		history_search: PromptHistorySearch,
	) -> Cow<'_, str> {
		self.reverse_search_active.store(true, Ordering::SeqCst);
		Cow::Owned(format!("(search: {}) ", history_search.term))
	}
}
