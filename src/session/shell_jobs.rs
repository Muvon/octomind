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

//! Watched MCP resources, tracked per session.
//!
//! When a tool result carries an MCP `ResourceLink`, the tool is handing back a
//! resource for the client to follow — octofs does this for every detached
//! shell job (a build, a test suite), but the mechanism is generic: any MCP
//! server that returns a resource link works, so octomind never needs to know
//! the URI scheme or which server produced it.
//!
//! A watched resource is *pending* from the moment its link appears until its
//! `resources/updated` arrives. This registry is in-memory and independent of
//! the conversation transcript, so it survives context compaction: a job
//! launched before a fold is still delivered after it (see
//! `mcp::client::on_resource_updated`). It is deliberately NOT persisted — a
//! resumed process cannot reattach to a dead OS job — so a resume starts empty.
//! Each entry keeps a short human label (the launching command, from the
//! resource link's name) so pending jobs can be described deterministically,
//! e.g. when re-injected into a compaction summary.

use rmcp::model::{CallToolResult, ContentBlock};
use std::collections::HashMap;
use std::sync::RwLock;

// session id -> (resource URI -> label) for links advertised but not yet updated.
static WATCHED: RwLock<Option<HashMap<String, HashMap<String, String>>>> = RwLock::new(None);

/// Every resource link (URI, label) a tool result advertised. The label is the
/// link's name (octofs sets it to the launching command); falls back to the URI.
pub fn resource_links_in(result: &CallToolResult) -> Vec<(String, String)> {
	result
		.content
		.iter()
		.filter_map(|block| match block {
			ContentBlock::ResourceLink(resource) => {
				let label = resource
					.title
					.clone()
					.filter(|title| !title.is_empty())
					.unwrap_or_else(|| resource.name.clone());
				let label = if label.is_empty() {
					resource.uri.clone()
				} else {
					label
				};
				Some((resource.uri.clone(), label))
			}
			_ => None,
		})
		.collect()
}

/// Register every resource link a tool result advertised, resolving the session
/// from the task-local context. No-op outside a session or when there are none.
pub fn note_watched_from_result(result: &CallToolResult) {
	let links = resource_links_in(result);
	if links.is_empty() {
		return;
	}
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	for (uri, label) in links {
		register_for_session(&session_id, &uri, &label);
	}
}

pub fn register_for_session(session_id: &str, uri: &str, label: &str) {
	let mut guard = WATCHED.write().unwrap();
	guard
		.get_or_insert_with(HashMap::new)
		.entry(session_id.to_string())
		.or_default()
		.insert(uri.to_string(), label.to_string());
}

pub fn is_watched_for_session(session_id: &str, uri: &str) -> bool {
	WATCHED
		.read()
		.unwrap()
		.as_ref()
		.and_then(|registry| registry.get(session_id))
		.map(|jobs| jobs.contains_key(uri))
		.unwrap_or(false)
}

/// Clear a resource once its update has arrived. Returns true if it was watched.
pub fn complete_for_session(session_id: &str, uri: &str) -> bool {
	let mut guard = WATCHED.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some(jobs) = registry.get_mut(session_id) {
			let was_watched = jobs.remove(uri).is_some();
			if jobs.is_empty() {
				registry.remove(session_id);
			}
			return was_watched;
		}
	}
	false
}

pub fn has_pending_for_session(session_id: &str) -> bool {
	WATCHED
		.read()
		.unwrap()
		.as_ref()
		.and_then(|registry| registry.get(session_id))
		.map(|jobs| !jobs.is_empty())
		.unwrap_or(false)
}

/// Whether the current session has any outstanding watched resource.
pub fn has_pending() -> bool {
	match crate::session::context::current_session_id() {
		Some(id) => has_pending_for_session(&id),
		None => false,
	}
}

/// Labels of the current session's outstanding jobs, `"label (uri)"` each, for
/// deterministically reminding the model a job is still running — e.g. when a
/// compaction would otherwise drop the launch message from context.
pub fn pending_labels() -> Vec<String> {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return Vec::new();
	};
	let guard = WATCHED.read().unwrap();
	let Some(jobs) = guard
		.as_ref()
		.and_then(|registry| registry.get(&session_id))
	else {
		return Vec::new();
	};
	let mut labels: Vec<String> = jobs
		.iter()
		.map(|(uri, label)| format!("{label} ({uri})"))
		.collect();
	labels.sort();
	labels
}

pub fn clear_for_session(session_id: &str) {
	if let Some(registry) = WATCHED.write().unwrap().as_mut() {
		registry.remove(session_id);
	}
}

#[cfg(test)]
#[path = "shell_jobs_tests.rs"]
mod shell_jobs_tests;
