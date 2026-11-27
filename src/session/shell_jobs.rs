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
//!
//! An entry also carries the launching call's evidence-ledger sequence. Its
//! completion is banked per session until the verify-gate folds it into that
//! ledger line, following the same drain-at-next-owner pattern as
//! `session::external_spend`.
//!
//! A tool call that a server answers with an MCP task (SEP-2663) is tracked the
//! same way under `task_key(tool_id)`: there is no resource to read, the client
//! polls the task itself (`mcp::client::background_task`) and delivers its result.
//!
//! Every entry carries its own safety window (`SAFETY_WINDOW`): a run waiting
//! on it abandons the job once that window has run out without a completion
//! signal, measured from the job's own registration — so a job launched while an
//! older one was pending keeps its whole window. An abandoned job gets a terminal
//! result of its own in place of the one that never came (`expire_overdue_for_session`).

use rmcp::model::{CallToolResult, ContentBlock};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::SystemTime;

#[derive(Debug, Clone)]
struct WatchedResource {
	server_name: String,
	label: String,
	sequence: Option<u64>,
	delivering: bool,
	/// Identity of this registration: what a delivery's claim names when it
	/// delivers, so a result for a registration replaced under the same key is refused.
	registration: u64,
	started_at: SystemTime,
	/// Monotonic end of the job's own safety window (see `SAFETY_WINDOW`).
	expires_at: std::time::Instant,
	/// An MCP task, not a resource: nothing to read, its poller delivers it.
	task: bool,
	/// Latest status message the task's poller saw.
	status: Option<String>,
	/// The task poller's stop signal. Sending stops the task; dropping the entry
	/// (completion, session teardown) drops it, which also stops a running one.
	stop: Option<std::sync::Arc<tokio::sync::watch::Sender<bool>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct CompletedJob {
	pub sequence: Option<u64>,
	pub label: String,
	pub body: String,
	uri: String,
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
	pub task: bool,
	pub status: Option<String>,
}

// session id -> resources advertised but not yet delivered into the inbox.
static WATCHED: RwLock<Option<HashMap<String, HashMap<String, WatchedResource>>>> = RwLock::new(None);

// session id -> completions waiting for the evidence ledger's next owner.
static COMPLETED: RwLock<Option<HashMap<String, Vec<CompletedJob>>>> = RwLock::new(None);

// Source of each registration's identity.
static REGISTRATIONS: AtomicU64 = AtomicU64::new(0);

/// How long a watched job may wait for its completion signal before a run
/// waiting on it abandons the job: far longer than any real build, but finite,
/// so a lost signal (server crash, a tool that linked a resource it never
/// updates) can never hang a run forever. Measured from each job's own
/// registration, never from when the session first had a job to wait for.
pub(crate) const SAFETY_WINDOW: std::time::Duration = std::time::Duration::from_secs(2 * 3600);

/// Lifecycle events for watched resources. Subscription tasks (which hold a
/// `subscriptions/listen` stream open) listen for these so they can end the
/// stream when the update was already delivered another way — e.g. the
/// legacy unsolicited push winning the race with stream establishment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
	/// The resource's outcome was delivered: its update arrived, or the job's
	/// safety window ran out and a run delivered a terminal result in its place
	/// (see `expire_overdue_for_session`). Ends any subscription still open for it.
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

/// Closing tag of a background job's result delivery. The text between the
/// opening tag's line and this tag is exactly the MCP output, so a consumer can
/// shape it like any other tool result (see `split_delivery`).
const DELIVERY_CLOSE: &str = "\n</background_job>";

/// Opening-tag attribute marking a delivery whose output is a failure.
pub const DELIVERY_ERROR_ATTR: &str = " is_error=\"true\"";

/// A background job's result delivery: the opening tag on its own line, the
/// MCP output, then the closing tag.
pub fn delivery_message(open_tag: &str, output: &str) -> String {
	format!("{open_tag}\n{output}{DELIVERY_CLOSE}")
}

/// Split a result delivery into its opening tag and MCP output; `None` for
/// anything `delivery_message` did not build.
pub fn split_delivery(content: &str) -> Option<(&str, &str)> {
	let (open_tag, rest) = content.split_once('\n')?;
	if !open_tag.starts_with("<background_job") {
		return None;
	}
	Some((open_tag, rest.strip_suffix(DELIVERY_CLOSE)?))
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
				let label = if label.is_empty() { resource.uri.clone() } else { label };
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
	register(session_id, server_name, uri, label, None);
}

/// Registry key of the MCP task a tool call became. Keyed by the call, which is
/// unique per session, so the launching round can attach its ledger sequence
/// without knowing the server's task id.
pub fn task_key(tool_id: &str) -> String {
	format!("mcp-task:{tool_id}")
}

/// Register an MCP task and return its stop signal for the poller: it turns
/// true on `stop_task_for_session` and closes when the entry is dropped.
pub fn register_task_for_session(
	session_id: &str,
	server_name: &str,
	key: &str,
	label: &str,
) -> tokio::sync::watch::Receiver<bool> {
	let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
	register(session_id, server_name, key, label, Some(std::sync::Arc::new(stop_tx)));
	stop_rx
}

fn register(
	session_id: &str,
	server_name: &str,
	key: &str,
	label: &str,
	stop: Option<std::sync::Arc<tokio::sync::watch::Sender<bool>>>,
) {
	let mut guard = WATCHED.write().unwrap();
	guard
		.get_or_insert_with(HashMap::new)
		.entry(session_id.to_string())
		.or_default()
		.insert(
			key.to_string(),
			WatchedResource {
				server_name: server_name.to_string(),
				label: label.to_string(),
				sequence: None,
				delivering: false,
				registration: REGISTRATIONS.fetch_add(1, Ordering::Relaxed),
				started_at: SystemTime::now(),
				expires_at: std::time::Instant::now() + SAFETY_WINDOW,
				task: stop.is_some(),
				status: None,
				stop,
			},
		);
}

/// Record the latest status message a pending task's poller saw.
pub fn set_task_status_for_session(session_id: &str, key: &str, status: &str) {
	if let Some(resource) = WATCHED
		.write()
		.unwrap()
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(key))
	{
		resource.status = Some(status.to_string());
	}
}

/// Ask a pending task's poller to stop it (it sends `tasks/cancel`). False when
/// the session has no pending task under that key.
pub fn stop_task_for_session(session_id: &str, key: &str) -> bool {
	let guard = WATCHED.read().unwrap();
	let Some(stop) = guard
		.as_ref()
		.and_then(|registry| registry.get(session_id))
		.and_then(|jobs| jobs.get(key))
		.and_then(|resource| resource.stop.as_ref())
	else {
		return false;
	};
	stop.send_replace(true);
	true
}

pub fn attach_sequence_for_session(session_id: &str, uri: &str, sequence: u64) {
	let mut watched = WATCHED.write().unwrap();
	if let Some(resource) = watched
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(uri))
	{
		resource.sequence = Some(sequence);
		return;
	}
	if let Some(job) = COMPLETED
		.write()
		.unwrap()
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.iter_mut().find(|job| job.uri == uri && job.sequence.is_none()))
	{
		job.sequence = Some(sequence);
	}
}

pub fn attach_sequence(uri: &str, sequence: u64) {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	attach_sequence_for_session(&session_id, uri, sequence);
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

/// Owning session for a watched resource, found by URI alone.
///
/// The unsolicited `resources/updated` push arrives on the MCP connection's
/// receive task, where the session task-local is unset, so its handler can
/// only offer a connect-time snapshot that may be empty or stale. Job URIs are
/// unique per server, so the registry is the authority for ownership.
pub fn find_session_for_uri(uri: &str) -> Option<String> {
	WATCHED
		.read()
		.unwrap()
		.as_ref()?
		.iter()
		.find(|(_, jobs)| jobs.contains_key(uri))
		.map(|(session_id, _)| session_id.clone())
}

/// A delivery's claim on one registration, presented when it delivers.
#[derive(Clone, Copy)]
pub(crate) struct Claim(u64);

/// Claim a watched job for delivery, marking it so duplicate update paths cannot
/// race two reads; `None` when nobody watches it or a delivery is in flight. The
/// claim names the registration: a job replaced under the same key meanwhile —
/// timed out and launched again — refuses the stale delivery.
pub(crate) fn claim_delivery_for_session(session_id: &str, uri: &str) -> Option<Claim> {
	let mut guard = WATCHED.write().unwrap();
	let resource = guard
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(uri))?;
	if resource.delivering {
		return None;
	}
	resource.delivering = true;
	Some(Claim(resource.registration))
}

/// Claim one watched resource for delivery; `claim_delivery_for_session` also
/// hands back the claim a delivery presents when it delivers.
pub fn begin_delivery_for_session(session_id: &str, uri: &str) -> bool {
	claim_delivery_for_session(session_id, uri).is_some()
}

/// Deliver a claimed job's result; false when the claim is stale — the job
/// completed, timed out or was replaced — and nothing is queued.
pub(crate) fn deliver_for_session(session_id: &str, uri: &str, body: &str, claim: Claim, open_tag: &str) -> bool {
	finalize_for_session(
		session_id,
		uri,
		|watched| watched.registration == claim.0,
		body,
		Some(open_tag),
	)
}

/// Bank a resource once its update has arrived, without queueing a delivery.
/// Returns true if it was watched.
pub fn complete_for_session(session_id: &str, uri: &str, body: &str) -> bool {
	finalize_for_session(session_id, uri, |_| true, body, None)
}

/// Finish one registration under the registry lock: `admit` revalidates that the
/// entry is still the one this finalization acts for, the result is queued (when
/// `open_tag` is given) before the entry is removed so a run never sees itself idle
/// with a result on its way, then the completion is banked and `Completed`
/// published — once, by whichever finalization is admitted. Lock order is registry,
/// then inbox: the push only appends and notifies, and nothing on the inbox side
/// takes the registry lock.
fn finalize_for_session(
	session_id: &str,
	uri: &str,
	admit: impl Fn(&WatchedResource) -> bool,
	body: &str,
	open_tag: Option<&str>,
) -> bool {
	let mut guard = WATCHED.write().unwrap();
	let Some(jobs) = guard.as_mut().and_then(|registry| registry.get_mut(session_id)) else {
		return false;
	};
	if !jobs.get(uri).is_some_and(admit) {
		return false;
	}
	if let Some(open_tag) = open_tag {
		crate::session::inbox::push_inbox_message_for_session(
			session_id,
			crate::session::inbox::InboxMessage {
				source: crate::session::inbox::InboxSource::BackgroundJob { id: uri.to_string() },
				content: delivery_message(open_tag, body),
			},
		);
	}
	let watched = jobs.remove(uri).expect("admitted under the same lock");
	if jobs.is_empty() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
	COMPLETED
		.write()
		.unwrap()
		.get_or_insert_with(HashMap::new)
		.entry(session_id.to_string())
		.or_default()
		.push(CompletedJob {
			sequence: watched.sequence,
			label: watched.label,
			body: body.to_string(),
			uri: uri.to_string(),
		});
	drop(guard);
	let _ = WATCH_EVENTS.send(WatchEvent::Completed {
		session_id: session_id.to_string(),
		uri: uri.to_string(),
	});
	true
}

pub(crate) fn take_completed_for_session(session_id: &str) -> Vec<CompletedJob> {
	COMPLETED
		.write()
		.unwrap()
		.as_mut()
		.and_then(|registry| registry.remove(session_id))
		.unwrap_or_default()
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

/// Monotonic instant at which the current session's soonest-expiring job must
/// be abandoned — the wake time of a run's safety backstop. `None` when the
/// session has no job to wait for.
pub(crate) fn earliest_expiry() -> Option<std::time::Instant> {
	let session_id = crate::session::context::current_session_id()?;
	earliest_expiry_for_session(&session_id)
}

/// [`earliest_expiry`] for a session named explicitly.
pub(crate) fn earliest_expiry_for_session(session_id: &str) -> Option<std::time::Instant> {
	let guard = WATCHED.read().unwrap();
	let jobs = guard.as_ref().and_then(|registry| registry.get(session_id))?;
	jobs.values().map(|resource| resource.expires_at).min()
}

/// Abandon the session's jobs whose safety window has run out — a completion
/// signal that never arrives — each with a terminal result of its own. Only a job
/// past its own window goes, delivery in flight or not (the read behind it has no
/// deadline): younger jobs, other sessions and banked completions stay intact. The
/// scan only names candidates; each is revalidated when finalized, so a job
/// launched again under the same key keeps its fresh window. Returns `(label,
/// uri)` per abandoned job for the caller to report.
pub(crate) fn expire_overdue_for_session(session_id: &str) -> Vec<(String, String)> {
	let now = std::time::Instant::now();
	// (label, uri, is an MCP task): the tag names a task like its poller does.
	let overdue: Vec<(String, String, bool)> = {
		let guard = WATCHED.read().unwrap();
		let Some(jobs) = guard.as_ref().and_then(|registry| registry.get(session_id)) else {
			return Vec::new();
		};
		jobs.iter()
			.filter(|(_, resource)| resource.expires_at <= now)
			.map(|(uri, resource)| (resource.label.clone(), uri.clone(), resource.task))
			.collect()
	};
	overdue
		.into_iter()
		.filter_map(|(label, uri, task)| time_out_for_session(session_id, &uri, task).then_some((label, uri)))
		.collect()
}

/// Abandon one job whose window has run out — rechecked under the registry lock
/// at finalization — with a terminal result: banked under the job's own label and
/// sequence, queued as an error delivery, then `Completed`. Dropping the entry
/// closes a task's stop signal, as teardown does.
fn time_out_for_session(session_id: &str, uri: &str, task: bool) -> bool {
	let kind = if task { "task" } else { "resource" };
	let open_tag = format!("<background_job {kind}=\"{uri}\"{DELIVERY_ERROR_ATTR}>");
	// Not the command's exit status, and said so: a waiter that reads a lost
	// tracker signal as a failed build restarts a job that is fine.
	let body = format!(
		"octomind stopped tracking this job: no completion signal arrived within the {:?} safety \
			 window. This is not the command's exit status — it may still be running, or may \
			 have finished without signalling. Check its output before deciding whether to \
			 restart it.",
		SAFETY_WINDOW
	);
	finalize_for_session(
		session_id,
		uri,
		|watched| watched.expires_at <= std::time::Instant::now(),
		&body,
		Some(&open_tag),
	)
}

/// Test seam: the crate has no tokio `test-util` (paused clock), so a test marks
/// a registered job's safety window as run out instead of waiting it out.
#[cfg(test)]
pub(crate) fn age_for_tests(session_id: &str, key: &str) {
	let mut guard = WATCHED.write().unwrap();
	let resource = guard
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(key))
		.expect("registered job");
	resource.expires_at = std::time::Instant::now();
}

/// Labels of the current session's outstanding jobs, `"label (uri)"` each, for
/// deterministically reminding the model a job is still running — e.g. when a
/// compaction would otherwise drop the launch message from context.
pub fn pending_labels() -> Vec<String> {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return Vec::new();
	};
	let guard = WATCHED.read().unwrap();
	let Some(jobs) = guard.as_ref().and_then(|registry| registry.get(&session_id)) else {
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
			task: resource.task,
			status: resource.status.clone(),
		})
		.collect();
	resources.sort_by(|a, b| a.uri.cmp(&b.uri));
	resources
}

pub fn clear_for_session(session_id: &str) {
	let mut watched = WATCHED.write().unwrap();
	let removed = watched
		.as_mut()
		.is_some_and(|registry| registry.remove(session_id).is_some());
	if let Some(registry) = COMPLETED.write().unwrap().as_mut() {
		let _ = registry.remove(session_id);
	}
	drop(watched);
	if removed {
		let _ = WATCH_EVENTS.send(WatchEvent::Cleared {
			session_id: session_id.to_string(),
		});
	}
}

#[cfg(test)]
#[path = "shell_jobs_tests.rs"]
mod shell_jobs_tests;
