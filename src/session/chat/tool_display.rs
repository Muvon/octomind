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

//! Shared tool display functions for consistent rendering across main sessions and agent execution

use crate::config::Config;
use colored::Colorize;

/// Display individual tool header with parameters (for parallel execution results)
pub async fn display_individual_tool_header_with_params(
	tool_name: &str,
	stored_tool_call: &Option<crate::mcp::McpToolCall>,
	config: &Config,
	tool_index: usize,
) {
	display_individual_tool_header_with_context(
		tool_name,
		stored_tool_call,
		config,
		tool_index,
		None, // No context suffix for main session
	)
	.await;
}

/// Display individual tool header with optional execution context (layer/agent).
/// Format: `╭ tool · server [· context]` — corner glyph opens the block,
/// the result line (printed by the executor) closes it with `╰`. Params are
/// indented under the header.
pub async fn display_individual_tool_header_with_context(
	tool_name: &str,
	stored_tool_call: &Option<crate::mcp::McpToolCall>,
	config: &Config,
	_tool_index: usize,
	execution_context: Option<&str>, // e.g., "layer_name" or "agent_context_gatherer"
) {
	let server_name =
		crate::session::chat::response::get_tool_server_name_async(tool_name, config).await;

	let corner = "╭".bright_cyan();
	let sep = "·".bright_black();
	let header = if let Some(context) = execution_context {
		format!(
			"{} {} {} {} {} {}",
			corner,
			tool_name.bright_cyan(),
			sep,
			server_name.bright_blue(),
			"·".bright_black(),
			context.bright_yellow(),
		)
	} else {
		format!(
			"{} {} {} {}",
			corner,
			tool_name.bright_cyan(),
			sep,
			server_name.bright_blue(),
		)
	};
	println!("{}", header);

	// Show parameters if available and log level allows. Indented 3 cells
	// under the corner so the block reads as `╭ tool …` + `   k v` lines.
	if let Some(tool_call) = stored_tool_call {
		if config.get_log_level().is_info_enabled() || config.get_log_level().is_debug_enabled() {
			display_tool_parameters_full(tool_call, config);
		}
	}
}

/// Display tool output in smart format with `│ ` rail prefix on each line,
/// creating visual continuity between the `╭` header and `╰` close in a
/// tool result block.
pub fn display_tool_output_smart(output_str: &str) {
	let rail = "│".bright_black();
	let lines: Vec<&str> = output_str.lines().collect();

	if lines.len() <= 20 && output_str.chars().count() <= 2000 {
		for line in &lines {
			println!("{} {}", rail, line);
		}
	} else if lines.len() > 20 {
		for line in lines.iter().take(15) {
			println!("{} {}", rail, line);
		}
		println!(
			"{} {}",
			rail,
			format!("... [{} more lines]", lines.len().saturating_sub(15)).bright_black(),
		);
	} else {
		let truncated: String = output_str.chars().take(1997).collect();
		println!("{} {}...", rail, truncated);
	}
}

/// Display tool parameters in full detail (for info/debug modes)
pub fn display_tool_parameters_full(tool_call: &crate::mcp::McpToolCall, config: &Config) {
	if let Ok(params_obj) = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
		tool_call.parameters.clone(),
	) {
		if !params_obj.is_empty() {
			// Find the longest key for column alignment (max 20 chars to prevent excessive spacing)
			let max_key_length = params_obj
				.keys()
				.map(|k| k.len())
				.max()
				.unwrap_or(0)
				.min(20);

			let rail = "│".bright_black();
			for (key, value) in params_obj.iter() {
				let formatted_value = if config.get_log_level().is_debug_enabled() {
					format_parameter_value_full(value)
				} else {
					format_parameter_value_smart(value)
				};

				// `│ key  value` — rail connects `╭` header to `╰` close,
				// key dim, value default fg, separator is plain spaces.
				println!(
					"{} {} {}",
					rail,
					format!("{:width$}", key, width = max_key_length).bright_black(),
					formatted_value
				);
			}
		}
	} else {
		// Fallback for non-object parameters (arrays, primitives, etc.)
		let params_str = serde_json::to_string(&tool_call.parameters).unwrap_or_default();
		if params_str != "null" {
			let rail = "│".bright_black();
			if config.get_log_level().is_debug_enabled() {
				println!("{} {} {}", rail, "params".bright_black(), params_str);
			} else if params_str.chars().count() > 100 {
				let truncated: String = params_str.chars().take(97).collect();
				println!("{} {} {}...", rail, "params".bright_black(), truncated);
			} else {
				println!("{} {} {}", rail, "params".bright_black(), params_str);
			}
		}
	}
}

/// Format a parameter value for smart display (info mode)
fn format_parameter_value_smart(value: &serde_json::Value) -> String {
	match value {
		serde_json::Value::String(s) => {
			if s.is_empty() {
				"\"\"".bright_black().to_string()
			} else if s.chars().count() > 100 {
				format!("\"{}...\"", s.chars().take(97).collect::<String>())
			} else if s.contains('\n') {
				// For multiline strings, show first line + indicator
				let lines: Vec<&str> = s.lines().collect();
				let first_line = lines.first().unwrap_or(&"");
				let first_line_chars: Vec<char> = first_line.chars().collect();
				if first_line_chars.len() > 80 {
					format!(
						"\"{}...\" [+{} lines]",
						first_line_chars.into_iter().take(77).collect::<String>(),
						lines.len().saturating_sub(1)
					)
				} else if lines.len() > 1 {
					format!(
						"\"{}\" [+{} lines]",
						first_line,
						lines.len().saturating_sub(1)
					)
				} else {
					format!("\"{}\"", first_line)
				}
			} else {
				format!("\"{}\"", s)
			}
		}
		serde_json::Value::Bool(b) => b.to_string(),
		serde_json::Value::Number(n) => n.to_string(),
		serde_json::Value::Array(arr) => {
			if arr.is_empty() {
				"[]".to_string()
			} else if arr.len() > 3 {
				format!("[{} items]", arr.len())
			} else {
				// Show small arrays inline
				let items: Vec<String> = arr
					.iter()
					.take(3)
					.map(|item| match item {
						serde_json::Value::String(s) => format!(
							"\"{}\"",
							if s.chars().count() > 20 {
								format!("{}...", s.chars().take(17).collect::<String>())
							} else {
								s.clone()
							}
						),
						_ => item.to_string(),
					})
					.collect();
				format!("[{}]", items.join(", "))
			}
		}
		serde_json::Value::Object(obj) => {
			if obj.is_empty() {
				"{}".to_string()
			} else {
				let obj_str = serde_json::to_string(value).unwrap_or_default();
				if obj_str.chars().count() > 100 {
					format!("{{...}} ({} keys)", obj.len())
				} else {
					obj_str
				}
			}
		}
		serde_json::Value::Null => "null".bright_black().to_string(),
	}
}

/// Format a parameter value for full display (debug mode)
fn format_parameter_value_full(value: &serde_json::Value) -> String {
	// Debug mode: Show everything without truncation
	match value {
		serde_json::Value::String(s) => format!("\"{}\"", s),
		_ => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
	}
}

// ─── Block rendering for CLI commands ──────────────────────────────────────
//
// Mirrors the tool block format (`╭ … │ … ╰`) so command outputs read like
// tool result blocks. One block per command invocation; sections inside are
// introduced by dim subheaders on the rail. The opening title is colored
// bright_cyan and the closing line carries a ✓/✗ glyph + the command name.

const RAIL: &str = "│";
const CORNER_OPEN: &str = "╭";
const CORNER_CLOSE: &str = "╰";

/// Open a block with the given title (typically the command name like `/info`).
/// Optionally a subtitle is appended after a dim `·` separator.
pub fn block_open(title: &str, subtitle: Option<&str>) {
	if let Some(sub) = subtitle {
		println!(
			"{} {} {} {}",
			CORNER_OPEN.bright_cyan(),
			title.bright_cyan(),
			"·".bright_black(),
			sub.bright_white(),
		);
	} else {
		println!("{} {}", CORNER_OPEN.bright_cyan(), title.bright_cyan());
	}
}

/// Render a dim subheader inside the block. Used to introduce sections.
/// Format: `│ name`  or  `│ name · value`.
pub fn block_section(name: &str) {
	println!("{} {}", RAIL.bright_black(), name.bright_cyan(),);
}

pub fn block_section_with(name: &str, value: &str) {
	println!(
		"{} {} {} {}",
		RAIL.bright_black(),
		name.bright_cyan(),
		"·".bright_black(),
		value.bright_white(),
	);
}

/// Render a key/value row indented under a section header. Keys are padded
/// to `key_width` and dimmed; values use the caller-provided color.
pub fn block_row(key: &str, value: &str, key_width: usize) {
	println!(
		"{}   {}  {}",
		RAIL.bright_black(),
		format!("{:width$}", key, width = key_width).bright_black(),
		value,
	);
}

/// Render a free-form indented line under a section. No key/value alignment.
pub fn block_row_text(text: &str) {
	println!("{}   {}", RAIL.bright_black(), text);
}

/// Render a top-level line on the rail (no indent — same level as section
/// headers). Use sparingly for prose like notes or empty-state messages.
pub fn block_line(text: &str) {
	println!("{} {}", RAIL.bright_black(), text);
}

/// Blank rail line — a `│` with nothing after. Useful as a section gap.
pub fn block_blank() {
	println!("{}", RAIL.bright_black());
}

/// Close the block with a success marker: `╰ ✓ <title>` and optional suffix.
pub fn block_close_ok(title: &str, suffix: Option<&str>) {
	if let Some(s) = suffix {
		println!(
			"{} {} {} {} {}",
			CORNER_CLOSE.bright_cyan(),
			"✓".bright_green(),
			title.bright_cyan(),
			"·".bright_black(),
			s,
		);
	} else {
		println!(
			"{} {} {}",
			CORNER_CLOSE.bright_cyan(),
			"✓".bright_green(),
			title.bright_cyan(),
		);
	}
}

/// Close the block with an error marker: `╰ ✗ <title> · <summary>`.
pub fn block_close_err(title: &str, summary: &str) {
	println!(
		"{} {} {} {} {}",
		CORNER_CLOSE.bright_cyan(),
		"✗".bright_red(),
		title.bright_red(),
		"·".bright_black(),
		summary.bright_white(),
	);
}

/// Compute the max key length for a slice of (key, value) tuples, clamped to
/// 20 columns to keep alignment readable when one key is wildly long.
pub fn key_width<'a, I, S>(keys: I) -> usize
where
	I: IntoIterator<Item = S>,
	S: AsRef<str> + 'a,
{
	keys.into_iter()
		.map(|k| k.as_ref().chars().count())
		.max()
		.unwrap_or(0)
		.min(20)
}
