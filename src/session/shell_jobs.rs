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
//! `resources/updated` arrives. The run loop must not treat a stdin-EOF as
//! "done" while anything is pending, or a one-shot run would exit and orphan
//! the build; the MCP client clears the entry and injects the resource's
//! contents when the update lands (see `mcp::client::on_resource_updated`).

use rmcp::model::{CallToolResult, ContentBlock};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

// session id -> set of resource URIs advertised but not yet updated.
static WATCHED: RwLock<Option<HashMap<String, HashSet<String>>>> = RwLock::new(None);

/// Every resource-link URI advertised by a tool result.
pub fn resource_links_in(result: &CallToolResult) -> Vec<String> {
	result
		.content
		.iter()
		.filter_map(|block| match block {
			ContentBlock::ResourceLink(resource) => Some(resource.uri.clone()),
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
	for uri in links {
		register_for_session(&session_id, &uri);
	}
}

pub fn register_for_session(session_id: &str, uri: &str) {
	let mut guard = WATCHED.write().unwrap();
	guard
		.get_or_insert_with(HashMap::new)
		.entry(session_id.to_string())
		.or_default()
		.insert(uri.to_string());
}

pub fn is_watched_for_session(session_id: &str, uri: &str) -> bool {
	WATCHED
		.read()
		.unwrap()
		.as_ref()
		.and_then(|registry| registry.get(session_id))
		.map(|set| set.contains(uri))
		.unwrap_or(false)
}

/// Clear a resource once its update has arrived. Returns true if it was watched.
pub fn complete_for_session(session_id: &str, uri: &str) -> bool {
	let mut guard = WATCHED.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some(set) = registry.get_mut(session_id) {
			let was_watched = set.remove(uri);
			if set.is_empty() {
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
		.map(|set| !set.is_empty())
		.unwrap_or(false)
}

/// Whether the current session has any outstanding watched resource.
pub fn has_pending() -> bool {
	match crate::session::context::current_session_id() {
		Some(id) => has_pending_for_session(&id),
		None => false,
	}
}

pub fn clear_for_session(session_id: &str) {
	if let Some(registry) = WATCHED.write().unwrap().as_mut() {
		registry.remove(session_id);
	}
}

#[cfg(test)]
#[path = "shell_jobs_tests.rs"]
mod shell_jobs_tests;
