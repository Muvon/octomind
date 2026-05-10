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

	if has_tap_tool {
		prompt.push_str(
			"\n\nIf required tools, skills, or capabilities are missing, call `tap` with `action=\"capability\"` and a short `prompt` describing the need.",
		);
	}

	// Add context about structured tags in conversation
	prompt.push_str(
		"\n\n## CONTEXT TAGS\n\
		User messages may contain structured context in XML-like tags:\n\
		- `<instructions>` — Project-specific instructions from the working directory. \
		Treat as persistent rules that apply to ALL your responses in this session.\n\
		- `<skill name=\"...\" description=\"...\">` — Domain knowledge injected on demand. \
		Follow the conventions and best practices described within. Multiple skills may be active simultaneously.\n\
		- `<constraints>` — Hard constraints appended to individual requests. \
		These override other guidance when they conflict.\n\n\
		These tags are system-managed context, not user-written messages. \
		Do not reference the tags themselves — just follow the content within them.",
	);

	// Enforce concise, action-first output behavior across all models and roles.
	// Modeled after production agentic system prompts (Claude Code, internal Anthropic guidelines).
	// Hard word limits between tool calls are the single most effective lever for mid-task verbosity.
	prompt.push_str(
		"\n\n## OUTPUT RULES\n\
		Go straight to the point. Be extra concise. Do not overdo it.\n\n\
		Between tool calls: <=25 words of text. State what you found or decided -- nothing else.\n\
		Final response: <=2 sentences unless the task explicitly requires more detail.\n\n\
		Never narrate intent before acting. Skip \"I'll now...\", \"Let me...\", \"I will search for...\" -- just act.\n\
		Never restate the request, add filler (\"Great!\", \"Sure!\"), or offer unsolicited follow-ups.\n\
		Don't explain your reasoning unless asked. State results and decisions directly.",
	);

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
		"\n\n## CONTEXT COMPRESSION ACTIVE\n\
		- {} compressions performed ({} tokens saved, {:.1}% reduction)\n\
		- Compressed sections marked with [COMPRESSED: id]\n\
		- **ANALYSIS FINDINGS in compressed summaries are trustworthy** — they were extracted from real tool results. \
		Do NOT re-read files or re-run searches just to verify what the summary already states.\n\
		- **FILE CONTEXT sections contain real file content** auto-read from disk at compression time. \
		Treat this content as current and accurate — do NOT re-read files that are already in FILE CONTEXT.\n\
		- If you need a file NOT in FILE CONTEXT, read it normally. But for files already there, use the provided content.\n\
		- Focus on recent uncompressed messages for current intent, compressed summaries for background knowledge.",
		compression_stats.total_compressions(),
		compression_stats.total_tokens_saved,
		compression_stats.avg_compression_ratio() * 100.0
	));
}
