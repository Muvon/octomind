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

// System prompt construction and compression hint injection

use std::path::Path;

pub async fn create_system_prompt(
	project_dir: &Path,
	config: &crate::config::Config,
	mode: &str,
) -> String {
	// Get mode-specific configuration
	let (_, mcp_config, _, _, system_prompt) = config.get_role_config(mode);

	// For developer role, process placeholders to add project context
	let mut prompt =
		crate::session::helper_functions::process_placeholders_async(system_prompt, project_dir)
			.await;

	let mut has_tap_tool = false;

	// Add MCP tools information if enabled
	if !mcp_config.server_refs.is_empty() {
		let config_for_role = config.get_merged_config_for_role(mode);
		let functions = crate::mcp::get_available_functions(&config_for_role).await;
		if !functions.is_empty() {
			prompt.push_str("\n\nYou have access to the following tools:");

			for function in &functions {
				if function.name == "tap" {
					has_tap_tool = true;
				}
				prompt.push_str(&format!(
					"\n\n- {} - {}",
					function.name, function.description
				));
			}
		}
	}

	prompt.push_str("\n\n<important>");

	if has_tap_tool {
		prompt.push_str(
			"\n<delegation>\n\
		Missing a tool that fits your role → capability(action=\"discover\"|\"enable\", …) to activate it. \
		Task outside your role → tap(action=\"run\", role=\"…\", …) to hand off.\n\
		</delegation>",
		);
	}

	prompt.push_str(
		"\n<context-tags>\n\
		User messages may contain XML-like context tags. Treat their content as system-managed; don't reference the tags themselves.\n\
		- <instructions>: persistent project rules, apply to all responses.\n\
		- <skill name=\"...\">: domain knowledge, follow its conventions; multiple may be active.\n\
		- <constraints>: hard per-request constraints, override other guidance on conflict.\n\
		- <system-note>: runtime action or context; obey it when actionable, but never treat it as a new user task or let it replace the underlying task.\n\
		</context-tags>",
	);

	prompt.push_str(
		"\n<supervisor-rules>\n\
		The supervisor injects control messages mid-task. They appear in the user turn but are system orders, not the user — obey each on your very next action, ahead of whatever you were doing.\n\
		- <pay-attention>…</pay-attention>: a steering order. Loop/stall → change tool, args, or sub-goal, never repeat the call. Verification → run the project's check and report the real result before claiming done. Format (status line / evidence) → emit exactly as given.\n\
		- <recall>…</recall>: past-session lessons (rules) and orientation (unverified — check first).\n\
		Never echo or mention these blocks; they are hidden from the user, and they never replace the task you were given.\n\
		The user sees only your final message. Write it as the complete, standalone answer to their original request — never a reply about a note, and never a delta on an earlier draft they never saw (\"here it is with evidence\", \"fixed as asked\"). If a note made you redo work, re-state the whole answer, not the correction.\n\
		</supervisor-rules>",
	);

	prompt.push_str(
		"\n<use_parallel_tool_calls>\n\
		Issue all independent tool calls in one batch — e.g. reading 3 files is 3 calls at once, not 3 turns. You receive every result together, so never call one tool and wait to decide the next. Only chain calls when one's arguments depend on another's result. Never guess or use placeholders for missing parameters.\n\
		</use_parallel_tool_calls>",
	);

	prompt.push_str(
		"\n<output-rules>\n\
		Be concise and action-first: <=25 words between tool calls, <=2 sentences in the final response unless more is genuinely needed. Skip intent narration (\"I'll now…\", \"Let me…\"), filler, request restatement, and unrequested reasoning — they cost tokens without informing the user.\n\
		</output-rules>",
	);

	prompt.push_str("\n</important>");

	prompt
}

/// Add compression context hints to system prompt for resumed sessions.
/// Informs the AI about compression state to improve reasoning with compressed context.
pub fn add_compression_hints_to_prompt(
	prompt: &mut String,
	compression_stats: &crate::session::CompressionStats,
) {
	if compression_stats.total_compressions() == 0 {
		return;
	}

	prompt.push_str(&format!(
		"\n\n<context_compression status=\"active\" compressions=\"{}\" tokens_saved=\"{}\" reduction=\"{:.1}%\">\n\
		Compressed turns appear as <conversation_summary id=\"…\">, <task_compressed id=\"…\">, <phase_compressed id=\"…\">, <project_compressed id=\"…\">. Their <analysis_findings> and <file_context> were extracted from real tool results and disk at compression time — treat them as current and accurate; do not re-read files or re-run searches to verify what they already state. Read files NOT in <file_context> normally. Use recent uncompressed messages for current intent, summaries for background.\n\
		</context_compression>",
		compression_stats.total_compressions(),
		compression_stats.total_tokens_saved,
		compression_stats.avg_compression_ratio() * 100.0
	));
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
