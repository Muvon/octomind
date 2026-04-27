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

// Info command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

pub fn handle_info(session: &ChatSession, config: &Config) -> Result<CommandResult> {
	let info = &session.session.info;

	let tokens_used = info.input_tokens + info.output_tokens;
	let tokens_per_second = if info.total_api_time_ms > 0 {
		(info.output_tokens as f64) / (info.total_api_time_ms as f64 / 1000.0)
	} else {
		0.0
	};
	// Estimate cache savings: approximate cost of cache_read_tokens if they had been
	// charged at the full input rate. The 3x weight for output tokens reflects typical
	// provider pricing (output tokens cost ~3x input tokens).
	let cache_savings =
		if info.cache_read_tokens > 0 && info.input_tokens > 0 && info.total_cost > 0.0 {
			let total_weighted = (info.input_tokens as f64) + (info.output_tokens as f64 * 3.0);
			if total_weighted > 0.0 {
				let estimated_input_rate = info.total_cost / total_weighted;
				info.cache_read_tokens as f64 * estimated_input_rate
			} else {
				0.0
			}
		} else {
			0.0
		};

	let compression_stats = if info.compression_stats.total_compressions() > 0 {
		Some(info.compression_stats.clone())
	} else {
		None
	};

	// Collect cache marker stats
	let cache_manager = crate::session::cache::CacheManager::new();
	let cache_stats =
		cache_manager.get_cache_statistics_with_config(&session.session, Some(config));

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Info {
			session_name: info.name.clone(),
			model: info.model.clone(),
			role: session.role.clone(),
			tokens_input: info.input_tokens,
			tokens_output: info.output_tokens,
			tokens_used,
			tokens_cached: info.cache_read_tokens,
			tokens_cache_write: info.cache_write_tokens,
			tokens_reasoning: info.reasoning_tokens,
			total_cost: info.total_cost,
			cache_savings,
			tokens_per_second,
			compression_stats,
			cache_markers_system: cache_stats.system_markers as u64,
			cache_markers_tool: cache_stats.tool_markers as u64,
			cache_markers_content: cache_stats.content_markers as u64,
			cache_non_cached_tokens: cache_stats.current_non_cached_tokens,
		},
	)))
}
