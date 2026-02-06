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

// Info command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use anyhow::Result;

pub fn handle_info(session: &ChatSession) -> Result<CommandResult> {
	// Get session info - for now, extract key fields
	// The full JSON is available via to_json() for WebSocket
	let info = &session.session.info;

	let tokens_used = info.input_tokens + info.output_tokens;
	let cache_savings = 0.0; // TODO: Calculate cache savings if needed

	let compression_stats = if info.compression_stats.total_compressions() > 0 {
		Some(info.compression_stats.clone())
	} else {
		None
	};

	Ok(CommandResult::HandledWithOutput(CommandOutput::Info {
		session_name: info.name.clone(),
		model: info.model.clone(),
		role: session.role.clone(),
		tokens_used,
		tokens_cached: info.cached_tokens,
		total_cost: info.total_cost,
		cache_savings,
		compression_stats,
	}))
}
