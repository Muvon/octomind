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

//! /schedule session command — thin wrapper over the MCP `schedule` tool.
//!
//! Mirrors the MCP tool's commands so the user can drive it directly from chat:
//!
//! `/schedule`                                       → list pending entries
//! `/schedule list`                                  → list pending entries
//! `/schedule remove <id>`                           → cancel an entry
//! `/schedule add when="in 5m" message="..."`        → schedule a one-shot
//! `/schedule add when="9am" message="..." every="1h" description="..."`
//! `/schedule edit <id> when="..." message="..."`    → update an entry
//!
//! Key=value tokens accept shell-style quoting so multi-word values work:
//! `when="in 1h 30m"`, `message="hello world"`, `description='daily standup'`.

use super::{CommandOutput, CommandResult};
use crate::mcp::McpToolCall;
use anyhow::Result;
use serde_json::{json, Map, Value};

pub async fn handle_schedule(input: &str, params: &[&str]) -> Result<CommandResult> {
	let subcommand = params.first().copied().unwrap_or("list");

	let mut json_params: Map<String, Value> = Map::new();

	match subcommand {
		"list" => {
			json_params.insert("command".to_string(), json!("list"));
		}
		"remove" | "rm" | "delete" | "del" => {
			let id = match params.get(1) {
				Some(id) => *id,
				None => {
					return Ok(CommandResult::HandledWithOutput(Box::new(
						CommandOutput::Schedule {
							data: json!({
								"subcommand": "error",
								"message": "usage: /schedule remove <id>",
							}),
						},
					)))
				}
			};
			json_params.insert("command".to_string(), json!("remove"));
			json_params.insert("id".to_string(), json!(id));
		}
		"add" => {
			json_params.insert("command".to_string(), json!("add"));
			match parse_kv_args(input, "add", &mut json_params) {
				Ok(()) => {}
				Err(e) => {
					return Ok(CommandResult::HandledWithOutput(Box::new(
						CommandOutput::Schedule {
							data: json!({
								"subcommand": "error",
								"message": format!("parse error: {e}"),
							}),
						},
					)))
				}
			}
		}
		"edit" => {
			json_params.insert("command".to_string(), json!("edit"));
			if let Some(id) = params.get(1) {
				// First positional after `edit` is treated as the id when it doesn't contain `=`.
				if !id.contains('=') {
					json_params.insert("id".to_string(), json!(*id));
				}
			}
			match parse_kv_args(input, "edit", &mut json_params) {
				Ok(()) => {}
				Err(e) => {
					return Ok(CommandResult::HandledWithOutput(Box::new(
						CommandOutput::Schedule {
							data: json!({
								"subcommand": "error",
								"message": format!("parse error: {e}"),
							}),
						},
					)))
				}
			}
		}
		"help" | "?" => {
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Schedule {
					data: json!({"subcommand": "help"}),
				},
			)))
		}
		other => {
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Schedule {
					data: json!({
						"subcommand": "error",
						"message": format!("unknown subcommand '{other}' — use: list, add, remove, edit, help"),
					}),
				},
			)))
		}
	}

	let call = McpToolCall {
		tool_name: "schedule".to_string(),
		tool_id: format!("cmd_schedule_{}", uuid::Uuid::new_v4().simple()),
		parameters: Value::Object(json_params),
	};

	match crate::mcp::core::schedule::execute_schedule_tool(&call).await {
		Ok(result) => {
			let text = result.extract_content();
			let is_error = result.is_error();
			Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Schedule {
					data: json!({
						"subcommand": subcommand,
						"is_error": is_error,
						"message": text,
					}),
				},
			)))
		}
		Err(e) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Schedule {
				data: json!({
					"subcommand": "error",
					"message": format!("schedule tool failed: {e}"),
				}),
			},
		))),
	}
}

/// Tokenize the part of `input` that follows the `<subcommand>` keyword (and the
/// optional positional id for `edit`), parsing `key=value` tokens with shell-style
/// quoting into `json_params`. Unknown keys are passed through verbatim so future
/// MCP-tool fields keep working without code changes.
fn parse_kv_args(
	input: &str,
	subcommand: &str,
	json_params: &mut Map<String, Value>,
) -> Result<()> {
	let after_sub = slice_after_token(input, subcommand).unwrap_or("");
	let tokens = tokenize_shell_like(after_sub)?;

	let mut consumed_first_positional = false;
	for tok in tokens {
		match tok.find('=') {
			Some(idx) => {
				let key = tok[..idx].trim().to_string();
				let val = tok[idx + 1..].to_string();
				if key.is_empty() {
					anyhow::bail!("empty key in token '{tok}'");
				}
				json_params.insert(key, Value::String(val));
			}
			None => {
				// First bare positional after `edit` is the id (already inserted by caller).
				if subcommand == "edit" && !consumed_first_positional {
					consumed_first_positional = true;
					continue;
				}
				anyhow::bail!(
					"expected key=value token, got '{tok}' (use key=\"value with spaces\")"
				);
			}
		}
	}
	Ok(())
}

/// Find `token` as a whole word in `input` and return the slice immediately after it.
/// Returns None if not found.
fn slice_after_token<'a>(input: &'a str, token: &str) -> Option<&'a str> {
	let mut search_start = 0;
	while let Some(idx) = input[search_start..].find(token) {
		let abs = search_start + idx;
		let before_ok = abs == 0 || input.as_bytes()[abs - 1].is_ascii_whitespace();
		let after_idx = abs + token.len();
		let after_ok =
			after_idx == input.len() || input.as_bytes()[after_idx].is_ascii_whitespace();
		if before_ok && after_ok {
			return Some(&input[after_idx..]);
		}
		search_start = abs + token.len();
	}
	None
}

/// Shell-style tokenizer: splits on whitespace but respects single/double quotes
/// and backslash escapes. Quotes are stripped from the output tokens.
fn tokenize_shell_like(input: &str) -> Result<Vec<String>> {
	let mut tokens = Vec::new();
	let mut current = String::new();
	let mut in_quote: Option<char> = None;
	let mut chars = input.chars().peekable();

	while let Some(c) = chars.next() {
		match (in_quote, c) {
			(Some(q), c) if c == q => in_quote = None,
			(None, '"') | (None, '\'') => in_quote = Some(c),
			(_, '\\') => {
				if let Some(next) = chars.next() {
					current.push(next);
				}
			}
			(None, c) if c.is_whitespace() => {
				if !current.is_empty() {
					tokens.push(std::mem::take(&mut current));
				}
			}
			_ => current.push(c),
		}
	}
	if in_quote.is_some() {
		anyhow::bail!("unterminated quote");
	}
	if !current.is_empty() {
		tokens.push(current);
	}
	Ok(tokens)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_tokenize_basic() {
		let toks = tokenize_shell_like("a b c").unwrap();
		assert_eq!(toks, vec!["a", "b", "c"]);
	}

	#[test]
	fn test_tokenize_double_quotes() {
		let toks = tokenize_shell_like(r#"when="in 5m" message="hello world""#).unwrap();
		assert_eq!(toks, vec!["when=in 5m", "message=hello world"]);
	}

	#[test]
	fn test_tokenize_single_quotes() {
		let toks = tokenize_shell_like("when='in 1h 30m' every='10m'").unwrap();
		assert_eq!(toks, vec!["when=in 1h 30m", "every=10m"]);
	}

	#[test]
	fn test_tokenize_mixed_quotes_and_bare() {
		let toks = tokenize_shell_like(r#"abc123 when="9am" message=hi"#).unwrap();
		assert_eq!(toks, vec!["abc123", "when=9am", "message=hi"]);
	}

	#[test]
	fn test_tokenize_unterminated_quote() {
		assert!(tokenize_shell_like(r#"key="value"#).is_err());
	}

	#[test]
	fn test_slice_after_token() {
		assert_eq!(
			slice_after_token("/schedule add when=5m", "add"),
			Some(" when=5m")
		);
		assert_eq!(
			slice_after_token("/schedule edit abc when=5m", "edit"),
			Some(" abc when=5m")
		);
		// Substring match should not count
		assert_eq!(slice_after_token("/schedule adder", "add"), None);
	}

	#[test]
	fn test_parse_kv_add() {
		let mut params = Map::new();
		parse_kv_args(
			r#"/schedule add when="in 5m" message="hi" every="10m""#,
			"add",
			&mut params,
		)
		.unwrap();
		assert_eq!(params.get("when").and_then(|v| v.as_str()), Some("in 5m"));
		assert_eq!(params.get("message").and_then(|v| v.as_str()), Some("hi"));
		assert_eq!(params.get("every").and_then(|v| v.as_str()), Some("10m"));
	}

	#[test]
	fn test_parse_kv_edit_skips_positional_id() {
		let mut params = Map::new();
		parse_kv_args(r#"/schedule edit abc123 when="9am""#, "edit", &mut params).unwrap();
		assert_eq!(params.get("when").and_then(|v| v.as_str()), Some("9am"));
		assert!(params.get("id").is_none()); // caller inserts id; parse_kv just skips it
	}

	#[test]
	fn test_parse_kv_rejects_bare_for_add() {
		let mut params = Map::new();
		let err = parse_kv_args("/schedule add bareword", "add", &mut params).unwrap_err();
		assert!(err.to_string().contains("expected key=value"));
	}
}
