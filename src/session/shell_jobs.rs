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
use std::time::SystemTime;

#[derive(Debug, Clone)]
struct WatchedResource {
	server_name: String,
	label: String,
	delivering: bool,
	started_at: SystemTime,
}

/// Point-in-time metadata for one MCP resource-backed background job.
///
/// The resource itself remains the authority for live status/output; this
/// local record supplies ownership and elapsed time even if `resources/read`
/// is temporarily unavailable.
#[derive(Debug, Clone)]
pub(crate) struct PendingResource {
	pub server_name: String,
	pub uri: String,
	pub label: String,
	pub delivering: bool,
	pub started_at: SystemTime,
}

// session id -> resources advertised but not yet delivered into the inbox.
static WATCHED: RwLock<Option<HashMap<String, HashMap<String, WatchedResource>>>> =
	RwLock::new(None);

/// Lifecycle events for watched resources. Subscription tasks (which hold a
/// `subscriptions/listen` stream open) listen for these so they can end the
/// stream when the update was already delivered another way — e.g. the
/// legacy unsolicited push winning the race with stream establishment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
	/// The resource's update arrived and was delivered.
	Completed { session_id: String, uri: String },
	/// The session's watched set was wiped (session teardown).
	Cleared { session_id: String },
}

lazy_static::lazy_static! {
	static ref WATCH_EVENTS: tokio::sync::broadcast::Sender<WatchEvent> =
		tokio::sync::broadcast::channel(64).0;
}

/// Subscribe to watched-resource lifecycle events. A receiver that lags
/// misses events — acceptable, as these only end a stream early; a missed
/// hint leaves the subscription open until its next notification or the
/// job's update, which still terminates it.
pub fn subscribe_events() -> tokio::sync::broadcast::Receiver<WatchEvent> {
	WATCH_EVENTS.subscribe()
}

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
pub fn note_watched_from_result(server_name: &str, result: &CallToolResult) {
	let links = resource_links_in(result);
	if links.is_empty() {
		return;
	}
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	for (uri, label) in links {
		register_for_session(&session_id, server_name, &uri, &label);
	}
}

pub fn register_for_session(session_id: &str, server_name: &str, uri: &str, label: &str) {
	let mut guard = WATCHED.write().unwrap();
	guard
		.get_or_insert_with(HashMap::new)
		.entry(session_id.to_string())
		.or_default()
		.insert(
			uri.to_string(),
			WatchedResource {
				server_name: server_name.to_string(),
				label: label.to_string(),
				delivering: false,
				started_at: SystemTime::now(),
			},
		);
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

/// Atomically claim one watched resource for delivery while keeping it
/// pending until its inbox message exists. Duplicate update paths therefore
/// cannot race two reads, and graceful shutdown cannot observe a false idle.
pub fn begin_delivery_for_session(session_id: &str, uri: &str) -> bool {
	let mut guard = WATCHED.write().unwrap();
	let Some(resource) = guard
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(uri))
	else {
		return false;
	};
	if resource.delivering {
		return false;
	}
	resource.delivering = true;
	true
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
			if was_watched {
				drop(guard);
				let _ = WATCH_EVENTS.send(WatchEvent::Completed {
					session_id: session_id.to_string(),
					uri: uri.to_string(),
				});
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
		.map(|(uri, resource)| format!("{} ({uri})", resource.label))
		.collect();
	labels.sort();
	labels
}

/// Snapshot outstanding resource-backed jobs for a session without holding
/// the registry lock while callers perform MCP `resources/read` requests.
pub(crate) fn pending_resources_for_session(session_id: &str) -> Vec<PendingResource> {
	let guard = WATCHED.read().unwrap();
	let Some(jobs) = guard.as_ref().and_then(|registry| registry.get(session_id)) else {
		return Vec::new();
	};
	let mut resources: Vec<PendingResource> = jobs
		.iter()
		.map(|(uri, resource)| PendingResource {
			server_name: resource.server_name.clone(),
			uri: uri.clone(),
			label: resource.label.clone(),
			delivering: resource.delivering,
			started_at: resource.started_at,
		})
		.collect();
	resources.sort_by(|a, b| a.uri.cmp(&b.uri));
	resources
}

pub fn clear_for_session(session_id: &str) {
	let removed = WATCHED
		.write()
		.unwrap()
		.as_mut()
		.is_some_and(|registry| registry.remove(session_id).is_some());
	if removed {
		let _ = WATCH_EVENTS.send(WatchEvent::Cleared {
			session_id: session_id.to_string(),
		});
	}
}

#[cfg(test)]
#[path = "shell_jobs_tests.rs"]
mod shell_jobs_tests;
