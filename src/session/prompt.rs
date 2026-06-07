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
		Missing a tool that fits your role → capability(action=\"discover\"|\"enable\", …), activate it yourself. \
		Task outside your role → tap(action=\"run\", role=\"…\", …), hand off to a specialist.\n\
		</delegation>",
		);
	}

	prompt.push_str(
		"\n<context-tags>\n\
		User messages may contain XML-like context tags. Treat their content as system-managed; don't reference the tags themselves.\n\
		- <instructions>: persistent project rules, apply to all responses.\n\
		- <skill name=\"...\">: domain knowledge, follow its conventions; multiple may be active.\n\
		- <constraints>: hard per-request constraints, override other guidance on conflict.\n\
		</context-tags>",
	);

	prompt.push_str(
		"\n<use_parallel_tool_calls>\n\
		If you intend to call multiple tools and there are no dependencies between the tool calls, make all of the independent tool calls in parallel. \
		Prioritize calling tools simultaneously whenever the actions can be done in parallel rather than sequentially. \
		For example, when reading 3 files, run 3 tool calls in parallel to read all 3 files into context at the same time. \
		Maximize use of parallel tool calls where possible to increase speed and efficiency. \
		Do NOT call one tool, stop, and wait for results before deciding the next call — you do not need to yield control to observe intermediate results; the runtime returns all results together. \
		However, if some tool calls depend on previous calls to inform dependent values like the parameters, do NOT call these tools in parallel and instead call them sequentially. \
		Never use placeholders or guess missing parameters in tool calls.\n\
		</use_parallel_tool_calls>",
	);

	prompt.push_str(
		"\n<output-rules>\n\
		Be concise, action-first. Between tool calls <=25 words. Final response <=2 sentences unless more is required. \
		No narration of intent (\"I'll now...\", \"Let me...\"), no filler (\"Great!\", \"Sure!\"), no restating the request, no unsolicited follow-ups, no reasoning unless asked.\n\
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
		Compressed turns appear as XML blocks: <conversation_summary id=\"…\">, <task_compressed id=\"…\">, <phase_compressed id=\"…\">, <project_compressed id=\"…\">.\n\
		<analysis_findings> inside a <conversation_summary> are trustworthy — they were extracted from real tool results. Trust them; do not re-read files or re-run searches just to verify what the summary already states.\n\
		<file_context> sections inside a compressed summary contain real file content auto-read from disk at compression time. Treat this content as current and accurate; for files already there, use the provided content. For files NOT in <file_context>, read them normally.\n\
		Focus on recent uncompressed messages for current intent and on compressed summaries for background knowledge.\n\
		</context_compression>",
		compression_stats.total_compressions(),
		compression_stats.total_tokens_saved,
		compression_stats.avg_compression_ratio() * 100.0
	));
}
