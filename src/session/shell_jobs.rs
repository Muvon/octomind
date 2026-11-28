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
//! Nothing here expires by age: a job is tracked for as long as it can be
//! observed, however long it runs and whether or not it prints. Its tracking
//! ends without its outcome only when its server answers that it no longer knows
//! the job — it restarted or dropped it, so nothing can observe it again — or when
//! a continuous outage, every probe of it failing, lasts `OUTAGE_BUDGET` from the
//! first failed probe: one terminal result says the outcome is unknown
//! (`tracking_lost_for_session`). A successful probe clears the outage; no
//! outcome of this module ever cancels a job — only an explicit stop or session
//! teardown does.

use crate::config::McpServerConfig;
use rmcp::model::{CallToolResult, ContentBlock};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
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
	/// The server the job was launched on, kept from the call that registered
	/// it so an observer — detached from any session context — reaches that same
	/// server again after its connection closed. `None` for a job registered
	/// without one: observable only while its connection stays open.
	server: Option<Arc<McpServerConfig>>,
	started_at: SystemTime,
	/// When a continuous run of failed observations began (see `OUTAGE_BUDGET`);
	/// `None` while the job is observable.
	outage_since: Option<std::time::Instant>,
	/// An MCP task, not a resource: nothing to read, its poller delivers it.
	task: bool,
	/// Latest status an observer saw for it.
	status: Option<String>,
	/// The task poller's stop signal. `true` asks the poller to cancel the task
	/// on the server (an explicit stop, session teardown); dropping the entry
	/// (completion, replacement, tracking lost) only closes it, which ends the
	/// polling without touching the task.
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
	/// The registration this snapshot describes: what a status check presents.
	pub registration: Claim,
}

// session id -> resources advertised but not yet delivered into the inbox.
static WATCHED: RwLock<Option<HashMap<String, HashMap<String, WatchedResource>>>> = RwLock::new(None);

// session id -> completions waiting for the evidence ledger's next owner.
static COMPLETED: RwLock<Option<HashMap<String, Vec<CompletedJob>>>> = RwLock::new(None);

// Source of each registration's identity.
static REGISTRATIONS: AtomicU64 = AtomicU64::new(0);

/// How long a job may stay unobservable — every status probe failing or timing
/// out — before octomind stops tracking it and reports its outcome as unknown.
/// Measured from the first failure of a continuous run, never from the job's
/// start: a job observable for days is tracked for days, and one successful
/// probe starts the budget over. Far longer than any real outage, but finite,
/// so a lost job can never hang a run forever.
pub(crate) const OUTAGE_BUDGET: std::time::Duration = std::time::Duration::from_secs(2 * 3600);

/// Lifecycle events for watched resources. Subscription tasks (which hold a
/// `subscriptions/listen` stream open) listen for these so they can end the
/// stream when the update was already delivered another way — e.g. the
/// legacy unsolicited push winning the race with stream establishment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
	/// The job's tracking ended with a delivered result: its update arrived, its
	/// task's poller finished, or nobody could observe it for the whole outage
	/// budget (`tracking_lost_for_session`). Ends any subscription still open
	/// for it.
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

/// Register every resource link a tool result advertised with the server that
/// produced it: the one the job is observed through again after a reconnect.
pub(crate) fn note_watched_from_server_config(server: &McpServerConfig, result: &CallToolResult) {
	note_watched(server.name(), Some(Arc::new(server.clone())), result);
}

fn note_watched(server_name: &str, server: Option<Arc<McpServerConfig>>, result: &CallToolResult) {
	let links = resource_links_in(result);
	if links.is_empty() {
		return;
	}
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	for (uri, label) in links {
		register(&session_id, server_name, &uri, &label, None, server.clone());
	}
}

/// Register a job by its server's name alone: nothing to reconnect to once the
/// connection it was registered under is gone.
pub fn register_for_session(session_id: &str, server_name: &str, uri: &str, label: &str) {
	register(session_id, server_name, uri, label, None, None);
}

/// Registry key of the MCP task a tool call became. Keyed by the call, which is
/// unique per session, so the launching round can attach its ledger sequence
/// without knowing the server's task id.
pub fn task_key(tool_id: &str) -> String {
	format!("mcp-task:{tool_id}")
}

/// Register an MCP task. Returns its stop signal for the poller — it turns true
/// when the task is to be cancelled on the server (`stop_task_for_session`,
/// session teardown) and closes when the entry is dropped, which only ends the
/// polling — and the registration's claim, both from the same insertion so the
/// poller never acts on a registration other than the one it was started for.
pub(crate) fn register_task_for_session(
	session_id: &str,
	server_name: &str,
	key: &str,
	label: &str,
) -> (tokio::sync::watch::Receiver<bool>, Claim) {
	let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
	let claim = register(
		session_id,
		server_name,
		key,
		label,
		Some(std::sync::Arc::new(stop_tx)),
		None,
	);
	(stop_rx, claim)
}

fn register(
	session_id: &str,
	server_name: &str,
	key: &str,
	label: &str,
	stop: Option<std::sync::Arc<tokio::sync::watch::Sender<bool>>>,
	server: Option<Arc<McpServerConfig>>,
) -> Claim {
	let claim = Claim {
		registration: REGISTRATIONS.fetch_add(1, Ordering::Relaxed),
	};
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
				registration: claim.registration,
				server,
				started_at: SystemTime::now(),
				outage_since: None,
				task: stop.is_some(),
				status: None,
				stop,
			},
		);
	claim
}

/// Record the latest status an observer saw, for the registration it observes:
/// a registration launched later under the same key keeps its own. False when
/// the claim is stale.
pub(crate) fn set_status_for_claim(session_id: &str, key: &str, claim: Claim, status: &str) -> bool {
	let mut guard = WATCHED.write().unwrap();
	let Some(resource) = guard
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(key))
	else {
		return false;
	};
	if resource.registration != claim.registration {
		return false;
	}
	resource.status = Some(status.to_string());
	true
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

/// Owning session for a watched resource a server signalled, found by that
/// server and URI: URIs are unique per server only, so a signal from one server
/// is matched against that server's registrations alone — the same URI on
/// another server claims nothing.
///
/// The unsolicited `resources/updated` push arrives on the MCP connection's
/// receive task, where the session task-local is unset, so its handler can
/// only offer a connect-time snapshot that may be empty or stale.
pub(crate) fn find_session_for_server_uri(server_name: &str, uri: &str) -> Option<String> {
	WATCHED
		.read()
		.unwrap()
		.as_ref()?
		.iter()
		.find(|(_, jobs)| {
			jobs.get(uri)
				.is_some_and(|resource| resource.server_name == server_name)
		})
		.map(|(session_id, _)| session_id.clone())
}

/// A registration's handle, taken once by whoever observes a job — its poller,
/// a delivery, a watcher, a status check — and presented at every step since:
/// each acts on the registration it started with, never on a job launched later
/// under the same key, and every registry call answers "gone" (`None`, `false`)
/// once that registration is no longer there.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Claim {
	registration: u64,
}

fn claim_of(resource: &WatchedResource) -> Claim {
	Claim {
		registration: resource.registration,
	}
}

/// Claim a watched job for delivery, marking it so duplicate update paths cannot
/// race two reads; `None` when that server did not launch it under that URI in
/// that session, or a delivery is in flight. The claim names the registration:
/// a job replaced under the same key meanwhile — launched again — refuses the
/// stale delivery.
pub(crate) fn claim_delivery_for_session(session_id: &str, server_name: &str, uri: &str) -> Option<Claim> {
	claim_delivery(session_id, uri, |resource| resource.server_name == server_name)
}

/// Claim one watched resource for delivery; `claim_delivery_for_session` also
/// hands back the claim a delivery presents when it delivers.
pub fn begin_delivery_for_session(session_id: &str, uri: &str) -> bool {
	claim_delivery(session_id, uri, |_| true).is_some()
}

fn claim_delivery(session_id: &str, uri: &str, owns: impl Fn(&WatchedResource) -> bool) -> Option<Claim> {
	let mut guard = WATCHED.write().unwrap();
	let resource = guard
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(uri))
		.filter(|resource| owns(resource))?;
	if resource.delivering {
		return None;
	}
	resource.delivering = true;
	Some(claim_of(resource))
}

/// The claim naming the registration currently under `uri`, without claiming
/// delivery: an observer takes it when it starts and presents it ever since, so
/// nothing it does lands on a job launched later under the same key.
pub(crate) fn registration_for_session(session_id: &str, uri: &str) -> Option<Claim> {
	WATCHED
		.read()
		.unwrap()
		.as_ref()
		.and_then(|registry| registry.get(session_id))
		.and_then(|jobs| jobs.get(uri))
		.map(claim_of)
}

/// The server the registration `claim` names under `uri` was launched on: its
/// name as registered, and its config when the registering call carried one —
/// the one an observer reaches again after its connection closed. `None` when
/// the claim is stale.
pub(crate) fn server_for_session(
	session_id: &str,
	uri: &str,
	claim: Claim,
) -> Option<(Arc<str>, Option<Arc<McpServerConfig>>)> {
	WATCHED
		.read()
		.unwrap()
		.as_ref()
		.and_then(|registry| registry.get(session_id))
		.and_then(|jobs| jobs.get(uri))
		.filter(|resource| resource.registration == claim.registration)
		.map(|resource| (Arc::from(resource.server_name.as_str()), resource.server.clone()))
}

/// Whether `claim` still names the registration under `uri`: an observer whose
/// registration is gone — completed, replaced, lost tracking — has nothing left
/// to observe and must not reconnect, read or report for it.
pub(crate) fn observer_is_current(session_id: &str, uri: &str, claim: Claim) -> bool {
	WATCHED
		.read()
		.unwrap()
		.as_ref()
		.and_then(|registry| registry.get(session_id))
		.and_then(|jobs| jobs.get(uri))
		.is_some_and(|resource| resource.registration == claim.registration)
}

/// Deliver a claimed job's result; false when the claim is stale — the job
/// completed, lost tracking or was replaced — and nothing is queued.
pub(crate) fn deliver_for_session(session_id: &str, uri: &str, body: &str, claim: Claim, open_tag: &str) -> bool {
	finalize_for_session(
		session_id,
		uri,
		|watched| watched.registration == claim.registration,
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

/// Note that a job could not be observed: a status probe failed or timed out.
/// The budget runs from the first failure of a continuous run
/// (`OUTAGE_BUDGET`), never from the job's start; `Some(true)` once it has run
/// out without a successful observation in between, when the caller delivers
/// [`tracking_lost_for_session`] once, `Some(false)` while the outage goes on.
/// `None` when the claim no longer names a registration — completed, replaced,
/// lost tracking — so the observer stops instead of retrying a job that is not
/// its own.
pub(crate) fn observation_failed_for_session(session_id: &str, uri: &str, claim: Claim) -> Option<bool> {
	let mut guard = WATCHED.write().unwrap();
	let resource = guard
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(uri))?;
	(resource.registration == claim.registration).then(|| {
		let since = *resource.outage_since.get_or_insert_with(std::time::Instant::now);
		since.elapsed() >= OUTAGE_BUDGET
	})
}

/// Note a successful observation of a job, whatever it said — still working,
/// unchanged or silent counts: the job is observable. The next failure starts
/// a fresh budget. False when the claim is stale, so the observer stops; the
/// registration now under the key keeps its own outage.
pub(crate) fn observation_recovered_for_session(session_id: &str, uri: &str, claim: Claim) -> bool {
	let mut guard = WATCHED.write().unwrap();
	let Some(resource) = guard
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(uri))
	else {
		return false;
	};
	if resource.registration != claim.registration {
		return false;
	}
	resource.outage_since = None;
	true
}

/// Why a job's tracking ended without its outcome.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TrackingLoss {
	/// Its server answered that it does not know the job: it restarted or
	/// dropped it, so no later check can observe it. Carries the server's answer.
	#[error("its server no longer knows it — it restarted or dropped the job ({0})")]
	Forgotten(String),
	/// Every check on it failed for the whole `OUTAGE_BUDGET`. Carries the last
	/// failure.
	#[error("every check on it has failed for the whole {budget:?} outage budget ({0})", budget = OUTAGE_BUDGET)]
	Unreachable(String),
}

/// Deliver the one terminal result of a job whose tracking ended without its
/// outcome (`loss` says why): queued, banked and `Completed` like any outcome,
/// under the delivery's `claim`. Nothing is cancelled and nothing is launched
/// again — the job may well be running — so the result says the outcome is
/// unknown and where to look, instead of passing for a failure. `task` names
/// the job kind in the tag.
pub(crate) fn tracking_lost_for_session(
	session_id: &str,
	uri: &str,
	task: bool,
	claim: Claim,
	loss: &TrackingLoss,
) -> bool {
	let kind = if task { "task" } else { "resource" };
	let open_tag = format!("<background_job {kind}=\"{uri}\"{DELIVERY_ERROR_ATTR}>");
	let body = format!(
		"octomind lost track of this job: {loss}. Its outcome is unknown — it may still be running, \
		 may have finished, or may have stopped with its server. Nothing was cancelled. Check its \
		 output or logs where it runs before deciding whether to launch it again."
	);
	deliver_for_session(session_id, uri, &body, claim, &open_tag)
}

/// Test seams: the crate has no tokio `test-util` (paused clock), so a test
/// marks a registered job as started four hours ago instead of waiting, and
/// runs out an outage in progress — returning whether there was one, which is
/// also the hand-off that an observer has recorded its failure.
#[cfg(test)]
pub(crate) fn age_for_tests(session_id: &str, key: &str) {
	let mut guard = WATCHED.write().unwrap();
	let resource = guard
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(key))
		.expect("registered job");
	resource.started_at = SystemTime::now() - std::time::Duration::from_secs(4 * 3600);
}

#[cfg(test)]
pub(crate) fn exhaust_outage_for_tests(session_id: &str, key: &str) -> bool {
	let mut guard = WATCHED.write().unwrap();
	let resource = guard
		.as_mut()
		.and_then(|registry| registry.get_mut(session_id))
		.and_then(|jobs| jobs.get_mut(key))
		.expect("registered job");
	let Some(since) = resource.outage_since.as_mut() else {
		return false;
	};
	*since = std::time::Instant::now() - OUTAGE_BUDGET;
	true
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
			registration: claim_of(resource),
		})
		.collect();
	resources.sort_by(|a, b| a.uri.cmp(&b.uri));
	resources
}

pub fn clear_for_session(session_id: &str) {
	let mut watched = WATCHED.write().unwrap();
	let removed = watched.as_mut().and_then(|registry| registry.remove(session_id));
	// Deliberate teardown, the session being over: its tasks are cancelled on the
	// server, as `stop_task_for_session` would. Dropping the entries alone would
	// only end the polling — which is all that completion, replacement or
	// tracking lost may do.
	if let Some(jobs) = &removed {
		for stop in jobs.values().filter_map(|resource| resource.stop.as_ref()) {
			stop.send_replace(true);
		}
	}
	let removed = removed.is_some();
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
