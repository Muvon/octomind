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

//! /learning session command — browse and manage cross-session lessons.
//!
//! `/learning`                  → list all lessons for current role+project (page 1)
//! `/learning list [page]`      → same, explicit; page defaults to 1
//! `/learning list *pattern*`   → filter by glob on content/title/tags
//! `/learning delete <index>`   → delete lesson by 1-based index from last list
//! `/learning clear`            → delete ALL lessons for current role+project

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;
use serde_json::json;

const LESSONS_PER_PAGE: usize = 15;

pub async fn handle_learning(
	session: &mut ChatSession,
	config: &Config,
	params: &[&str],
) -> Result<CommandResult> {
	match params.first().copied() {
		None | Some("list") => {
			// Determine page and optional glob pattern from remaining params.
			// Accepts: `list`, `list 2`, `list *pattern*`, `list *pattern* 2`
			let rest = if params.first().copied() == Some("list") {
				&params[1..]
			} else {
				params
			};

			let mut pattern: Option<&str> = None;
			let mut page: usize = 1;
			for tok in rest {
				if tok.contains('*') {
					pattern = Some(tok);
				} else if let Ok(n) = tok.parse::<usize>() {
					page = n;
				}
			}
			handle_list(session, config, pattern, page).await
		}
		Some("delete") | Some("rm") | Some("remove") => {
			let index_str = match params.get(1) {
				Some(s) => *s,
				None => {
					return Ok(CommandResult::HandledWithOutput(Box::new(
						CommandOutput::Learning {
							data: json!({
								"subcommand": "error",
								"message": "usage: /learning delete <index>",
							}),
						},
					)))
				}
			};
			let index: usize = match index_str.parse() {
				Ok(n) if n > 0 => n,
				_ => {
					return Ok(CommandResult::HandledWithOutput(Box::new(
						CommandOutput::Learning {
							data: json!({
								"subcommand": "error",
								"message": format!("invalid index '{}' — must be a positive integer", index_str),
							}),
						},
					)))
				}
			};
			handle_delete(session, config, index).await
		}
		Some("clear") => handle_clear(session, config).await,
		Some(other) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Learning {
				data: json!({
					"subcommand": "error",
					"message": format!("unknown subcommand '{}' — use: list, delete, clear", other),
				}),
			},
		))),
	}
}

async fn handle_list(
	session: &ChatSession,
	config: &Config,
	pattern: Option<&str>,
	page: usize,
) -> Result<CommandResult> {
	if page == 0 {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Learning {
				data: json!({
					"subcommand": "error",
					"message": "page number must be a positive integer",
				}),
			},
		)));
	}

	let (role, project) = role_and_project(session);
	let backend = crate::learning::backend::create_backend(&config.learning);
	let all = all_lessons(&*backend, &role, &project, config).await?;

	// Apply glob filter if present.
	let filtered: Vec<_> = if let Some(pat) = pattern {
		all.into_iter()
			.filter(|l| {
				glob_match(pat, &l.content)
					|| glob_match(pat, &l.title)
					|| l.tags.iter().any(|t| glob_match(pat, t))
			})
			.collect()
	} else {
		all
	};

	let total = filtered.len();
	let total_pages = if total == 0 {
		0
	} else {
		total.div_ceil(LESSONS_PER_PAGE)
	};

	if page > total_pages && total > 0 {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Learning {
				data: json!({
					"subcommand": "error",
					"message": format!("page {} not found — total pages: {}", page, total_pages),
				}),
			},
		)));
	}

	let start = (page - 1) * LESSONS_PER_PAGE;
	let end = std::cmp::min(start + LESSONS_PER_PAGE, total);

	let lessons: Vec<serde_json::Value> = filtered[start..end]
		.iter()
		.enumerate()
		.map(|(i, l)| {
			json!({
				"index": start + i + 1,
				"id": l.file_id(),
				"content": l.content,
				"title": l.title,
				"importance": l.importance,
				"confidence": l.confidence,
				"scope": l.scope,
				"tags": l.tags,
				"created": l.created,
			})
		})
		.collect();

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Learning {
			data: json!({
				"subcommand": "list",
				"role": role,
				"project": project,
				"lessons": lessons,
				"total": total,
				"page": page,
				"total_pages": total_pages,
				"pattern": pattern,
			}),
		},
	)))
}

async fn handle_delete(
	session: &ChatSession,
	config: &Config,
	index: usize,
) -> Result<CommandResult> {
	let (role, project) = role_and_project(session);
	let backend = crate::learning::backend::create_backend(&config.learning);
	let all = all_lessons(&*backend, &role, &project, config).await?;

	if index > all.len() || all.is_empty() {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Learning {
				data: json!({
					"subcommand": "error",
					"message": format!("index {} out of range — {} lesson(s) total", index, all.len()),
				}),
			},
		)));
	}

	let lesson = &all[index - 1];
	let id = lesson.file_id();
	let content_preview: String = lesson.content.chars().take(60).collect();

	match backend.delete(&id, &role, &project, config).await {
		Ok(()) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Learning {
				data: json!({
					"subcommand": "delete",
					"index": index,
					"id": id,
					"content_preview": content_preview,
				}),
			},
		))),
		Err(e) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Learning {
				data: json!({
					"subcommand": "error",
					"message": format!("delete failed: {}", e),
				}),
			},
		))),
	}
}

async fn handle_clear(session: &ChatSession, config: &Config) -> Result<CommandResult> {
	let (role, project) = role_and_project(session);
	let backend = crate::learning::backend::create_backend(&config.learning);
	let all = all_lessons(&*backend, &role, &project, config).await?;

	if all.is_empty() {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Learning {
				data: json!({
					"subcommand": "clear",
					"deleted": 0,
					"message": "no lessons to clear",
				}),
			},
		)));
	}

	let total = all.len();
	let mut deleted = 0;
	let mut errors: Vec<String> = Vec::new();

	for lesson in &all {
		let id = lesson.file_id();
		match backend.delete(&id, &role, &project, config).await {
			Ok(()) => deleted += 1,
			Err(e) => errors.push(format!("{}: {}", id, e)),
		}
	}

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Learning {
			data: json!({
				"subcommand": "clear",
				"deleted": deleted,
				"total": total,
				"errors": errors,
			}),
		},
	)))
}

fn role_and_project(session: &ChatSession) -> (String, String) {
	let role = session.role.clone();
	let project = std::env::current_dir()
		.ok()
		.and_then(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
		.unwrap_or_else(|| "unknown".to_string());
	(role, project)
}

/// All lessons the `/learning` command operates on: scoped (current
/// role+project) followed by global (user-wide). Stable order so list/delete
/// indices stay aligned across calls.
async fn all_lessons(
	backend: &dyn crate::learning::backend::LearningBackend,
	role: &str,
	project: &str,
	config: &Config,
) -> Result<Vec<crate::learning::Lesson>> {
	let mut all = backend.retrieve_all(role, project, config).await?;
	all.extend(backend.retrieve_global(config).await?);
	Ok(all)
}

/// Simple glob matching: `*` matches any sequence of characters. Case-insensitive.
fn glob_match(pattern: &str, text: &str) -> bool {
	let pattern = pattern.to_lowercase();
	let text = text.to_lowercase();
	let parts: Vec<&str> = pattern.split('*').collect();
	if parts.len() == 1 {
		return text == pattern;
	}
	let mut pos = 0;
	for (i, part) in parts.iter().enumerate() {
		if part.is_empty() {
			continue;
		}
		match text[pos..].find(part) {
			Some(idx) => {
				if i == 0 && idx != 0 {
					return false;
				}
				pos += idx + part.len();
			}
			None => return false,
		}
	}
	if !pattern.ends_with('*') {
		if let Some(last) = parts.last() {
			if !last.is_empty() {
				return text.ends_with(last);
			}
		}
	}
	true
}
