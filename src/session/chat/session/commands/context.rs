// Copyright 2025 Muvon Un Limited
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

// Context command handler

use super::super::core::ChatSession;
use crate::config::Config;
use anyhow::Result;

pub fn handle_context(session: &ChatSession, config: &Config, params: &[&str]) -> Result<bool> {
	// Parse filter parameter if provided
	let filter = if params.is_empty() {
		"all".to_string()
	} else {
		params[0].to_lowercase()
	};

	// Display current session context with filtering
	session.display_session_context_filtered(config, &filter);
	Ok(false)
}
