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

//! Session-scoped context for multi-session concurrency support.
//!
//! This module provides session isolation for global state that was previously
//! process-global. Each WebSocket session gets its own isolated context that
//! propagates through async task boundaries via `tokio::task_local!`.
//!
//! # Architecture
//!
//! Before: Process-global singletons (NOTIFICATION_SENDER, PLAN_STORAGE, etc.)
//! After: Session-keyed registries + task-local propagation.
//!
//! Pattern: a `RwLock<HashMap<SessionId, State>>` static stores per-session
//! state; a `tokio::task_local!` cell carries the active `SessionId` through
//! async boundaries. Accessor functions read `current_session_id()` and
//! look up the registry by that id. Sessions are torn down via
//! `cleanup_session(&id)` which removes the entry from every registry.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

/// Unique identifier for a session.
pub type SessionId = String;

/// Session-scoped state that travels through async boundaries.
///
/// This struct contains all the state that was previously process-global.
/// Each session gets its own instance, and it's propagated via task_local!
#[derive(Debug, Clone)]
pub struct SessionContext {
	/// Unique session identifier
	pub session_id: SessionId,
	/// Role name for this session
	pub role: String,
	/// Project identifier (derived from git remote or cwd)
	pub project_id: String,
	/// Session working directory anchor
	pub workdir: PathBuf,
}

impl SessionContext {
	/// Create a new session context.
	pub fn new(session_id: SessionId, role: String, project_id: String, workdir: PathBuf) -> Self {
		Self {
			session_id,
			role,
			project_id,
			workdir,
		}
	}

	/// Create a context for the current session with defaults.
	pub fn for_session(session_id: &str, role: &str) -> Self {
		let project_id = crate::mcp::process::derive_project_id();
		let workdir = std::env::current_dir().unwrap_or_default();
		Self::new(
			session_id.to_string(),
			role.to_string(),
			project_id,
			workdir,
		)
	}
}

// ---------------------------------------------------------------------------
// Task-local session ID propagation
// ---------------------------------------------------------------------------

tokio::task_local! {
	/// The current session ID, propagated through async task boundaries.
	/// Use `with_session_id()` to run code within a session context.
	pub static CURRENT_SESSION_ID: Arc<SessionId>;
}

/// Run a future within a session context.
///
/// This sets up the task-local session ID so that session-scoped state
/// can be accessed from anywhere in the async call stack.
pub async fn with_session_id<F, T>(session_id: SessionId, f: F) -> T
where
	F: std::future::Future<Output = T>,
{
	let id = Arc::new(session_id);
	CURRENT_SESSION_ID.scope(id, f).await
}

/// Get the current session ID from task-local storage.
///
/// Returns None if called outside of a session context.
pub fn current_session_id() -> Option<SessionId> {
	CURRENT_SESSION_ID.try_with(|id| (**id).clone()).ok()
}

/// Get the current session ID, panicking if not in a session context.
///
/// Use this only when you're certain you're in a session context.
pub fn expect_session_id() -> SessionId {
	current_session_id().expect("not in a session context - call with_session_id first")
}

// ---------------------------------------------------------------------------
// Session-keyed registries for global state
// ---------------------------------------------------------------------------

/// Notification sender for WebSocket/JSONL output.
pub type NotificationSender = tokio::sync::mpsc::UnboundedSender<crate::websocket::ServerMessage>;

/// Registry for notification senders, keyed by session ID.
/// Each session gets its own channel for MCP notifications.
static NOTIFICATION_SENDERS: RwLock<Option<HashMap<SessionId, NotificationSender>>> =
	RwLock::new(None);

/// Initialize the notification registry (call once at startup).
pub fn init_notification_registry() {
	let mut guard = NOTIFICATION_SENDERS.write().unwrap();
	if guard.is_none() {
		*guard = Some(HashMap::new());
	}
}

/// Register a notification sender for a session.
pub fn set_notification_sender_for_session(session_id: &SessionId, tx: NotificationSender) {
	let mut guard = NOTIFICATION_SENDERS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry.insert(session_id.clone(), tx);
}

/// Remove a notification sender when a session ends.
pub fn clear_notification_sender_for_session(session_id: &SessionId) {
	if let Ok(mut guard) = NOTIFICATION_SENDERS.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

/// Get the notification sender for the current session (from task-local).
pub fn get_notification_sender() -> Option<NotificationSender> {
	let session_id = current_session_id()?;
	let guard = NOTIFICATION_SENDERS.read().ok()?;
	let registry = guard.as_ref()?;
	registry.get(&session_id).cloned()
}

/// Send a notification through the current session's channel.
pub fn send_notification(msg: crate::websocket::ServerMessage) {
	if let Some(tx) = get_notification_sender() {
		let _ = tx.send(msg);
	}
}
/// Register a notification sender for a session (alias for set_notification_sender_for_session).
pub fn register_notification_sender(session_id: SessionId, tx: NotificationSender) {
	set_notification_sender_for_session(&session_id, tx);
}

/// Remove a notification sender when a session ends (alias for clear_notification_sender_for_session).
pub fn unregister_notification_sender(session_id: SessionId) {
	clear_notification_sender_for_session(&session_id);
}

/// Get notification sender by session ID (for use from process.rs).
pub fn get_notification_sender_by_id(session_id: &SessionId) -> Option<NotificationSender> {
	let guard = NOTIFICATION_SENDERS.read().ok()?;
	let registry = guard.as_ref()?;
	registry.get(session_id).cloned()
}

// ---------------------------------------------------------------------------
// Session-keyed working directory
// ---------------------------------------------------------------------------

/// Registry for session working directories.
/// Replaces the thread_local WORKDIR in mcp/workdir.rs for async contexts.
static SESSION_WORKDIRS: RwLock<Option<HashMap<SessionId, (PathBuf, PathBuf)>>> = RwLock::new(None); // (session_anchor, current)

/// Set the session working directory anchor.
pub fn set_session_workdir(session_id: &SessionId, path: PathBuf) {
	let mut guard = SESSION_WORKDIRS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry.insert(session_id.clone(), (path.clone(), path));
}

/// Override the active directory mid-session (workdir tool).
pub fn set_current_workdir(session_id: &SessionId, path: PathBuf) {
	if let Ok(mut guard) = SESSION_WORKDIRS.write() {
		if let Some(registry) = guard.as_mut() {
			if let Some((session, _)) = registry.get(session_id) {
				registry.insert(session_id.clone(), (session.clone(), path));
			}
		}
	}
}

/// Get the current working directory for a session.
pub fn get_current_workdir(session_id: &SessionId) -> Option<PathBuf> {
	let guard = SESSION_WORKDIRS.read().ok()?;
	let registry = guard.as_ref()?;
	registry.get(session_id).map(|(_, current)| current.clone())
}

/// Get the session anchor directory (for workdir reset).
pub fn get_session_workdir_anchor(session_id: &SessionId) -> Option<PathBuf> {
	let guard = SESSION_WORKDIRS.read().ok()?;
	let registry = guard.as_ref()?;
	registry.get(session_id).map(|(session, _)| session.clone())
}

/// Remove working directory state when a session ends.
pub fn clear_session_workdir(session_id: &SessionId) {
	if let Ok(mut guard) = SESSION_WORKDIRS.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session-keyed role storage
// ---------------------------------------------------------------------------

/// Registry for session roles.
/// Each session has its own role that can be queried by MCP tools.
static SESSION_ROLES: RwLock<Option<HashMap<SessionId, String>>> = RwLock::new(None);

/// Registry for session configs.
/// Each session has its own config for logging macros.
use crate::config::Config;
static SESSION_CONFIGS: RwLock<Option<HashMap<SessionId, Config>>> = RwLock::new(None);

/// Set the role for a session.
pub fn set_session_role(session_id: &SessionId, role: &str) {
	let mut guard = SESSION_ROLES.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry.insert(session_id.clone(), role.to_string());
}

/// Get the role for a session.
pub fn get_session_role(session_id: &SessionId) -> Option<String> {
	let guard = SESSION_ROLES.read().ok()?;
	let registry = guard.as_ref()?;
	registry.get(session_id).cloned()
}

/// Remove role when a session ends.
pub fn clear_session_role(session_id: &SessionId) {
	if let Ok(mut guard) = SESSION_ROLES.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

/// Set the config for a session.
pub fn set_session_config(session_id: &SessionId, config: &Config) {
	let mut guard = SESSION_CONFIGS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry.insert(session_id.clone(), config.clone());
}

/// Get the config for a session.
pub fn get_session_config(session_id: &SessionId) -> Option<Config> {
	let guard = SESSION_CONFIGS.read().ok()?;
	let registry = guard.as_ref()?;
	registry.get(session_id).cloned()
}

/// Remove config when a session ends.
pub fn clear_session_config(session_id: &SessionId) {
	if let Ok(mut guard) = SESSION_CONFIGS.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session-keyed plan storage
// ---------------------------------------------------------------------------

use crate::mcp::core::plan::memory_storage::MemoryPlanStorage;

/// Registry for plan storage, keyed by session ID.
/// Each session has its own plan state.
static PLAN_REGISTRIES: RwLock<Option<HashMap<SessionId, Arc<Mutex<MemoryPlanStorage>>>>> =
	RwLock::new(None);

/// Get or create plan storage for a session.
pub fn get_plan_storage(session_id: &SessionId) -> Arc<Mutex<MemoryPlanStorage>> {
	{
		let guard = PLAN_REGISTRIES.read().unwrap();
		if let Some(registry) = guard.as_ref() {
			if let Some(storage) = registry.get(session_id) {
				return storage.clone();
			}
		}
	}

	// Create new storage
	let mut guard = PLAN_REGISTRIES.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let storage = Arc::new(Mutex::new(MemoryPlanStorage::new()));
	registry.insert(session_id.clone(), storage.clone());
	storage
}

/// Remove plan storage when a session ends.
pub fn clear_plan_storage(session_id: &SessionId) {
	if let Ok(mut guard) = PLAN_REGISTRIES.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session-keyed task start index (for plan compression)
// ---------------------------------------------------------------------------

/// Registry for task start indices, keyed by session ID.
static TASK_START_INDICES: RwLock<Option<HashMap<SessionId, usize>>> = RwLock::new(None);

/// Set the task start index for a session.
pub fn set_task_start_index(session_id: &SessionId, index: usize) {
	let mut guard = TASK_START_INDICES.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry.insert(session_id.clone(), index);
	crate::log_debug!(
		"Plan task start index set to: {} for session: {}",
		index,
		session_id
	);
}

/// Get the task start index for a session.
pub fn get_task_start_index(session_id: &SessionId) -> Option<usize> {
	let guard = TASK_START_INDICES.read().ok()?;
	let registry = guard.as_ref()?;
	registry.get(session_id).copied()
}

/// Get and clear the task start index for a session.
pub fn take_task_start_index(session_id: &SessionId) -> Option<usize> {
	let mut guard = TASK_START_INDICES.write().ok()?;
	let registry = guard.as_mut()?;
	registry.remove(session_id)
}

/// Clear the task start index for a session.
pub fn clear_task_start_index(session_id: &SessionId) {
	if let Ok(mut guard) = TASK_START_INDICES.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session-keyed schedule storage
// ---------------------------------------------------------------------------

use crate::mcp::core::schedule::storage::ScheduleStore;

/// Registry for schedule storage, keyed by session ID.
static SCHEDULE_REGISTRIES: RwLock<Option<HashMap<SessionId, Arc<Mutex<ScheduleStore>>>>> =
	RwLock::new(None);

/// Get or create schedule storage for a session.
pub fn get_schedule_storage(session_id: &SessionId) -> Arc<Mutex<ScheduleStore>> {
	{
		let guard = SCHEDULE_REGISTRIES.read().unwrap();
		if let Some(registry) = guard.as_ref() {
			if let Some(storage) = registry.get(session_id) {
				return storage.clone();
			}
		}
	}

	// Create new storage
	let mut guard = SCHEDULE_REGISTRIES.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let storage = Arc::new(Mutex::new(ScheduleStore::new()));
	registry.insert(session_id.clone(), storage.clone());
	storage
}

// ---------------------------------------------------------------------------
// Session-keyed schedule notify
// ---------------------------------------------------------------------------

use tokio::sync::Notify;

/// Registry for schedule notify, keyed by session ID.
/// Each session has its own Notify that wakes up when schedules change.
static SCHEDULE_NOTIFIES: RwLock<Option<HashMap<SessionId, Arc<Notify>>>> = RwLock::new(None);

/// Get or create schedule notify for a session.
pub fn get_schedule_notify(session_id: &SessionId) -> Arc<Notify> {
	{
		let guard = SCHEDULE_NOTIFIES.read().unwrap();
		if let Some(registry) = guard.as_ref() {
			if let Some(notify) = registry.get(session_id) {
				return notify.clone();
			}
		}
	}

	// Create new notify
	let mut guard = SCHEDULE_NOTIFIES.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let notify = Arc::new(Notify::new());
	registry.insert(session_id.clone(), notify.clone());
	notify
}

/// Notify that schedules have changed for a session.
pub fn notify_schedule_change(session_id: &SessionId) {
	let guard = SCHEDULE_NOTIFIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some(notify) = registry.get(session_id) {
			notify.notify_one();
		}
	}
}

/// Remove schedule notify when a session ends.
pub fn clear_schedule_notify(session_id: &SessionId) {
	if let Ok(mut guard) = SCHEDULE_NOTIFIES.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

/// Remove schedule storage when a session ends.
pub fn clear_schedule_storage(session_id: &SessionId) {
	if let Ok(mut guard) = SCHEDULE_REGISTRIES.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session-keyed hints accumulator
// ---------------------------------------------------------------------------

/// Registry for hints accumulator, keyed by session ID.
static HINTS_REGISTRIES: RwLock<Option<HashMap<SessionId, Vec<String>>>> = RwLock::new(None);

/// Push a hint for the current session.
pub fn push_hint_for_session(session_id: &SessionId, hint: String) {
	let mut guard = HINTS_REGISTRIES.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry.entry(session_id.clone()).or_default().push(hint);
}

/// Drain hints for a session, returning deduplicated list.
pub fn drain_hints_for_session(session_id: &SessionId) -> Vec<String> {
	let mut guard = HINTS_REGISTRIES.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some(hints) = registry.get_mut(session_id) {
			let mut seen = std::collections::HashSet::new();
			return hints.drain(..).filter(|h| seen.insert(h.clone())).collect();
		}
	}
	Vec::new()
}

/// Check if there are pending hints for a session.
pub fn has_hints_for_session(session_id: &SessionId) -> bool {
	let guard = HINTS_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some(hints) = registry.get(session_id) {
			return !hints.is_empty();
		}
	}
	false
}

/// Clear hints when a session ends.
pub fn clear_hints_for_session(session_id: &SessionId) {
	if let Ok(mut guard) = HINTS_REGISTRIES.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session-keyed dynamic agent manager
// ---------------------------------------------------------------------------

/// Registry for dynamic agents, keyed by session ID.
/// Each session has its own set of registered/enabled agents.
use crate::mcp::core::dynamic_agents::DynamicAgentConfig;

/// Type alias for a session's dynamic agent state (agents map, enabled map).
type DynamicAgentState = (HashMap<String, DynamicAgentConfig>, HashMap<String, bool>);

static DYNAMIC_AGENT_REGISTRIES: RwLock<Option<HashMap<SessionId, DynamicAgentState>>> =
	RwLock::new(None);

pub fn register_dynamic_agent_for_session(session_id: &SessionId, agent: DynamicAgentConfig) {
	let agent_name = agent.name.clone();
	let mut guard = DYNAMIC_AGENT_REGISTRIES.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let (agents, enabled) = registry
		.entry(session_id.clone())
		.or_insert_with(|| (HashMap::new(), HashMap::new()));
	agents.insert(agent_name.clone(), agent);
	enabled.insert(agent_name, false);
}

/// Enable a dynamic agent for a session.
pub fn enable_dynamic_agent_for_session(session_id: &SessionId, agent_name: &str) -> bool {
	let mut guard = DYNAMIC_AGENT_REGISTRIES.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some((agents, enabled)) = registry.get_mut(session_id) {
			if agents.contains_key(agent_name) {
				enabled.insert(agent_name.to_string(), true);
				return true;
			}
		}
	}
	false
}

/// Disable a dynamic agent for a session.
pub fn disable_dynamic_agent_for_session(session_id: &SessionId, agent_name: &str) -> bool {
	let mut guard = DYNAMIC_AGENT_REGISTRIES.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some((_, enabled)) = registry.get_mut(session_id) {
			if enabled.contains_key(agent_name) {
				enabled.insert(agent_name.to_string(), false);
				return true;
			}
		}
	}
	false
}

/// Remove a dynamic agent for a session.
pub fn remove_dynamic_agent_for_session(
	session_id: &SessionId,
	agent_name: &str,
) -> Option<DynamicAgentConfig> {
	let mut guard = DYNAMIC_AGENT_REGISTRIES.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some((agents, enabled)) = registry.get_mut(session_id) {
			enabled.remove(agent_name);
			return agents.remove(agent_name);
		}
	}
	None
}

/// Get all dynamic agents for a session.
pub fn get_dynamic_agents_for_session(
	session_id: &SessionId,
) -> Vec<(String, DynamicAgentConfig, bool)> {
	let guard = DYNAMIC_AGENT_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((agents, enabled)) = registry.get(session_id) {
			return agents
				.iter()
				.map(|(name, config)| {
					let is_enabled = *enabled.get(name).unwrap_or(&false);
					(name.clone(), config.clone(), is_enabled)
				})
				.collect();
		}
	}
	Vec::new()
}

/// Check if a dynamic agent exists for a session.
pub fn has_dynamic_agent(session_id: &SessionId, agent_name: &str) -> bool {
	let guard = DYNAMIC_AGENT_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((agents, _)) = registry.get(session_id) {
			return agents.contains_key(agent_name);
		}
	}
	false
}

/// Check if a dynamic agent is enabled for a session.
pub fn is_dynamic_agent_enabled(session_id: &SessionId, agent_name: &str) -> bool {
	let guard = DYNAMIC_AGENT_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((_, enabled)) = registry.get(session_id) {
			return *enabled.get(agent_name).unwrap_or(&false);
		}
	}
	false
}

/// Clear all dynamic agents for a session.
pub fn clear_dynamic_agents_for_session(session_id: &SessionId) {
	if let Ok(mut guard) = DYNAMIC_AGENT_REGISTRIES.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session-keyed dynamic MCP server manager
// ---------------------------------------------------------------------------

use crate::config::McpServerConfig;
use crate::mcp::McpFunction;

/// Type alias for a session's dynamic server state (servers map, functions map, enabled map).
type DynamicServerState = (
	HashMap<String, McpServerConfig>,
	HashMap<String, Vec<McpFunction>>,
	HashMap<String, bool>,
);

static DYNAMIC_SERVER_REGISTRIES: RwLock<Option<HashMap<SessionId, DynamicServerState>>> =
	RwLock::new(None);

pub fn register_dynamic_server_for_session(session_id: &SessionId, server: McpServerConfig) {
	let server_name = server.name().to_string();
	let mut guard = DYNAMIC_SERVER_REGISTRIES.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let (servers, _functions, enabled) = registry
		.entry(session_id.clone())
		.or_insert_with(|| (HashMap::new(), HashMap::new(), HashMap::new()));
	servers.insert(server_name.clone(), server);
	enabled.insert(server_name, false);
}

/// Enable a dynamic MCP server for a session (stores functions).
///
/// Merges `funcs` into any existing function list for `server_name` instead
/// of overwriting. Required because multiple capabilities can share one MCP
/// server with disjoint tool filters — overwriting would erase the first
/// cap's tool record when the second one activates. Dedup is by `name`:
/// a function already present is skipped, not duplicated.
pub fn enable_dynamic_server_for_session(
	session_id: &SessionId,
	server_name: &str,
	funcs: Vec<McpFunction>,
) -> bool {
	let mut guard = DYNAMIC_SERVER_REGISTRIES.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some((servers, functions, enabled)) = registry.get_mut(session_id) {
			if servers.contains_key(server_name) {
				let entry = functions.entry(server_name.to_string()).or_default();
				let known: std::collections::HashSet<String> =
					entry.iter().map(|f| f.name.clone()).collect();
				for f in funcs {
					if !known.contains(&f.name) {
						entry.push(f);
					}
				}
				enabled.insert(server_name.to_string(), true);
				return true;
			}
		}
	}
	false
}

/// Disable a dynamic MCP server for a session.
pub fn disable_dynamic_server_for_session(session_id: &SessionId, server_name: &str) -> bool {
	let mut guard = DYNAMIC_SERVER_REGISTRIES.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some((_, functions, enabled)) = registry.get_mut(session_id) {
			if enabled.contains_key(server_name) {
				enabled.insert(server_name.to_string(), false);
				functions.remove(server_name);
				return true;
			}
		}
	}
	false
}

/// Remove a dynamic MCP server for a session.
pub fn remove_dynamic_server_for_session(
	session_id: &SessionId,
	server_name: &str,
) -> Option<McpServerConfig> {
	let mut guard = DYNAMIC_SERVER_REGISTRIES.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some((servers, functions, enabled)) = registry.get_mut(session_id) {
			functions.remove(server_name);
			enabled.remove(server_name);
			return servers.remove(server_name);
		}
	}
	None
}

/// Get all dynamic MCP servers for a session.
pub fn get_dynamic_servers_for_session(session_id: &SessionId) -> Vec<(String, Vec<String>, bool)> {
	let guard = DYNAMIC_SERVER_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((servers, _functions, enabled)) = registry.get(session_id) {
			return servers
				.iter()
				.map(|(name, config)| {
					let tools = config.tools().to_vec();
					let is_enabled = *enabled.get(name).unwrap_or(&false);
					(name.clone(), tools, is_enabled)
				})
				.collect();
		}
	}
	Vec::new()
}

/// Get functions for a dynamic MCP server for a session.
pub fn get_dynamic_server_functions_for_session(
	session_id: &SessionId,
	server_name: &str,
) -> Option<Vec<McpFunction>> {
	let guard = DYNAMIC_SERVER_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((_, functions, _)) = registry.get(session_id) {
			return functions.get(server_name).cloned();
		}
	}
	None
}

/// Get all enabled dynamic server configs for a session.
pub fn get_all_dynamic_server_configs_for_session(session_id: &SessionId) -> Vec<McpServerConfig> {
	let guard = DYNAMIC_SERVER_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((servers, _, enabled)) = registry.get(session_id) {
			return servers
				.iter()
				.filter(|(name, _)| *enabled.get(*name).unwrap_or(&false))
				.map(|(_, config)| config.clone())
				.collect();
		}
	}
	Vec::new()
}

/// Get all dynamic server functions for a session.
pub fn get_all_dynamic_server_functions_for_session(session_id: &SessionId) -> Vec<McpFunction> {
	let guard = DYNAMIC_SERVER_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((_, functions, enabled)) = registry.get(session_id) {
			return functions
				.iter()
				.filter(|(name, _)| *enabled.get(*name).unwrap_or(&false))
				.flat_map(|(_, funcs)| funcs.iter().cloned())
				.collect();
		}
	}
	Vec::new()
}

/// Check if a server name is dynamically managed for a session.
pub fn has_dynamic_server(session_id: &SessionId, server_name: &str) -> bool {
	let guard = DYNAMIC_SERVER_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((servers, _, _)) = registry.get(session_id) {
			return servers.contains_key(server_name);
		}
	}
	false
}

/// Check if a tool belongs to any dynamic server for a session.
pub fn is_dynamic_server_tool(session_id: &SessionId, tool_name: &str) -> bool {
	let guard = DYNAMIC_SERVER_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((_, functions, _)) = registry.get(session_id) {
			return functions
				.values()
				.any(|funcs| funcs.iter().any(|f| f.name == tool_name));
		}
	}
	false
}

/// Get the dynamic server name for a specific tool for a session.
pub fn get_dynamic_server_name_by_tool(session_id: &SessionId, tool_name: &str) -> Option<String> {
	let guard = DYNAMIC_SERVER_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((_, functions, _)) = registry.get(session_id) {
			for (server_name, funcs) in functions {
				if funcs.iter().any(|f| f.name == tool_name) {
					return Some(server_name.clone());
				}
			}
		}
	}
	None
}

/// Get a specific dynamic server config and its enabled status for a session.
pub fn get_dynamic_server_for_session(
	session_id: &SessionId,
	server_name: &str,
) -> Option<(McpServerConfig, bool)> {
	let guard = DYNAMIC_SERVER_REGISTRIES.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some((servers, _, enabled)) = registry.get(session_id) {
			if let Some(config) = servers.get(server_name) {
				let is_enabled = *enabled.get(server_name).unwrap_or(&false);
				return Some((config.clone(), is_enabled));
			}
		}
	}
	None
}

/// Clear all dynamic MCP servers for a session.
pub fn clear_dynamic_servers_for_session(session_id: &SessionId) {
	if let Ok(mut guard) = DYNAMIC_SERVER_REGISTRIES.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session-keyed background job manager
// ---------------------------------------------------------------------------

use crate::session::background_jobs::BackgroundJobManager;

/// Registry for session background job managers.
/// Each session has its own job manager for async agent jobs.
static JOB_MANAGERS: RwLock<Option<HashMap<SessionId, Arc<BackgroundJobManager>>>> =
	RwLock::new(None);

/// Initialize a job manager for a session.
pub fn init_job_manager_for_session(session_id: &SessionId) {
	let max_concurrent = std::thread::available_parallelism()
		.map(|p| p.get())
		.unwrap_or(4);
	let manager = BackgroundJobManager::new(max_concurrent);
	let mut guard = JOB_MANAGERS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry.insert(session_id.clone(), Arc::new(manager));
}

/// Get the job manager for the current session.
pub fn get_job_manager_for_session() -> Option<Arc<BackgroundJobManager>> {
	let session_id = current_session_id()?;
	let guard = JOB_MANAGERS.read().ok()?;
	let registry = guard.as_ref()?;
	registry.get(&session_id).cloned()
}

/// Kill all jobs for a session (called on session exit).
pub fn kill_all_jobs_for_session(session_id: &SessionId) {
	if let Ok(mut guard) = JOB_MANAGERS.write() {
		if let Some(registry) = guard.as_mut() {
			if let Some(manager) = registry.remove(session_id) {
				manager.kill_all();
			}
		}
	}
}

/// Clear job manager for a session (called during cleanup).
pub fn clear_job_manager_for_session(session_id: &SessionId) {
	kill_all_jobs_for_session(session_id);
}

// ---------------------------------------------------------------------------
// Session-keyed active skills
// ---------------------------------------------------------------------------

/// Registry for active skills per session.
/// Each entry is the skill name that has been injected into context via `skill(use)`.
static ACTIVE_SKILLS: RwLock<Option<HashMap<SessionId, Vec<String>>>> = RwLock::new(None);

/// Register a skill as active for a session.
pub fn add_active_skill(session_id: &SessionId, skill_name: &str) {
	let mut guard = ACTIVE_SKILLS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let skills = registry.entry(session_id.clone()).or_default();
	if !skills.contains(&skill_name.to_string()) {
		skills.push(skill_name.to_string());
	}
}

/// Remove a skill from active state for a session.
pub fn remove_active_skill(session_id: &SessionId, skill_name: &str) {
	let mut guard = ACTIVE_SKILLS.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some(skills) = registry.get_mut(session_id) {
			skills.retain(|s| s != skill_name);
		}
	}
}

/// Get all active skill names for a session.
pub fn get_active_skills(session_id: &SessionId) -> Vec<String> {
	let guard = ACTIVE_SKILLS.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some(skills) = registry.get(session_id) {
			return skills.clone();
		}
	}
	Vec::new()
}

/// Check if a skill is currently active for a session.
pub fn has_active_skill(session_id: &SessionId, skill_name: &str) -> bool {
	let guard = ACTIVE_SKILLS.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some(skills) = registry.get(session_id) {
			return skills.iter().any(|s| s == skill_name);
		}
	}
	false
}

/// Clear all active skills when a session ends.
pub fn clear_active_skills(session_id: &SessionId) {
	if let Ok(mut guard) = ACTIVE_SKILLS.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session-keyed env skills (OCTOMIND_SKILLS — preserved across /done compression)
// ---------------------------------------------------------------------------

/// Registry for env-injected skills per session.
/// Tracks only skills loaded via OCTOMIND_SKILLS env var, not auto-activated ones.
static ENV_SKILLS: RwLock<Option<HashMap<SessionId, Vec<String>>>> = RwLock::new(None);

/// Register a skill as env-loaded for a session.
pub fn add_env_skill(session_id: &SessionId, skill_name: &str) {
	let mut guard = ENV_SKILLS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let skills = registry.entry(session_id.clone()).or_default();
	if !skills.contains(&skill_name.to_string()) {
		skills.push(skill_name.to_string());
	}
}

/// Get all env-loaded skill names for a session.
pub fn get_env_skills(session_id: &SessionId) -> Vec<String> {
	let guard = ENV_SKILLS.read().unwrap();
	if let Some(registry) = guard.as_ref() {
		if let Some(skills) = registry.get(session_id) {
			return skills.clone();
		}
	}
	Vec::new()
}

/// Clear all env skills when a session ends.
pub fn clear_env_skills(session_id: &SessionId) {
	if let Ok(mut guard) = ENV_SKILLS.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session-keyed capability refcounts (skill → MCP server lifecycle)
// ---------------------------------------------------------------------------

/// skill_name → Vec<server_name> mapping per session.
type SkillServerMap = HashMap<SessionId, HashMap<String, Vec<String>>>;

/// Refcount per MCP server name: how many active skills loaded this server via capabilities.
/// When the count reaches 0 on skill forget, the server is disabled + removed.
static CAPABILITY_REFCOUNTS: RwLock<Option<HashMap<SessionId, HashMap<String, usize>>>> =
	RwLock::new(None);

/// Which MCP servers each skill loaded via capabilities.
/// Used by `execute_forget` to know which servers to decrement.
static SKILL_CAPABILITY_SERVERS: RwLock<Option<SkillServerMap>> = RwLock::new(None);

/// Increment the refcount for a capability-loaded server. Returns the new count.
pub fn increment_capability_refcount(session_id: &SessionId, server_name: &str) -> usize {
	let mut guard = CAPABILITY_REFCOUNTS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let counts = registry.entry(session_id.clone()).or_default();
	let count = counts.entry(server_name.to_string()).or_insert(0);
	*count += 1;
	*count
}

/// Decrement the refcount for a capability-loaded server. Returns the new count.
/// Returns 0 if the server was not tracked (safe to call unconditionally).
pub fn decrement_capability_refcount(session_id: &SessionId, server_name: &str) -> usize {
	let mut guard = CAPABILITY_REFCOUNTS.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some(counts) = registry.get_mut(session_id) {
			if let Some(count) = counts.get_mut(server_name) {
				*count = count.saturating_sub(1);
				let result = *count;
				if result == 0 {
					counts.remove(server_name);
				}
				return result;
			}
		}
	}
	0
}

/// Record which servers a skill loaded via capabilities.
pub fn set_skill_capability_servers(
	session_id: &SessionId,
	skill_name: &str,
	servers: Vec<String>,
) {
	if servers.is_empty() {
		return;
	}
	let mut guard = SKILL_CAPABILITY_SERVERS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let map = registry.entry(session_id.clone()).or_default();
	map.insert(skill_name.to_string(), servers);
}

/// Remove and return the servers a skill loaded via capabilities.
pub fn take_skill_capability_servers(session_id: &SessionId, skill_name: &str) -> Vec<String> {
	let mut guard = SKILL_CAPABILITY_SERVERS.write().unwrap();
	if let Some(registry) = guard.as_mut() {
		if let Some(map) = registry.get_mut(session_id) {
			return map.remove(skill_name).unwrap_or_default();
		}
	}
	Vec::new()
}

/// Clear capability refcounts when a session ends.
pub fn clear_capability_refcounts(session_id: &SessionId) {
	if let Ok(mut guard) = CAPABILITY_REFCOUNTS.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

/// Clear skill capability server mappings when a session ends.
pub fn clear_skill_capability_servers(session_id: &SessionId) {
	if let Ok(mut guard) = SKILL_CAPABILITY_SERVERS.write() {
		if let Some(registry) = guard.as_mut() {
			registry.remove(session_id);
		}
	}
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Clean up all session-scoped state when a session ends.
/// Call this when a WebSocket connection closes or a session is destroyed.
pub fn cleanup_session(session_id: &SessionId) {
	clear_notification_sender_for_session(session_id);
	clear_session_workdir(session_id);
	clear_session_role(session_id);
	clear_session_config(session_id);
	clear_plan_storage(session_id);
	clear_task_start_index(session_id);
	clear_schedule_storage(session_id);
	clear_hints_for_session(session_id);
	clear_dynamic_agents_for_session(session_id);
	clear_dynamic_servers_for_session(session_id);
	clear_job_manager_for_session(session_id);
	clear_active_skills(session_id);
	clear_env_skills(session_id);
	clear_capability_refcounts(session_id);
	clear_skill_capability_servers(session_id);
	crate::session::inbox::clear_inbox_for_session(session_id);
	crate::session::tap_runs::clear_for_session(session_id);
	crate::mcp::core::plan::compression::cleanup_compression_state(session_id);
	clear_schedule_notify(session_id);
}

/// Initialize all session-scoped services. Call once per session inside `with_session_id`.
/// Centralizes the init sequence so entry points don't duplicate it.
pub fn init_session_services(role: &str) {
	crate::session::inbox::init_inbox_for_session();
	crate::session::tap_runs::init_for_session();
	crate::mcp::agent::functions::init_job_manager();
	// Extract domain from role/tag (e.g., "developer:general" → "developer")
	let domain = role.split(':').next().unwrap_or(role);
	crate::mcp::core::skill_auto::init_pool(domain);
}

#[cfg(test)]
mod tests {
	use super::*;

	// Session-keyed registries are process-global. Tests use unique session
	// ids (uuid-like) so parallel-running tests don't see each other's
	// state. Each test also cleans up its own session at the end so the
	// registries don't grow unbounded across the suite.

	fn unique_id(label: &str) -> SessionId {
		format!(
			"test-{}-{}-{}",
			label,
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.map(|d| d.as_nanos())
				.unwrap_or(0)
		)
	}

	#[tokio::test]
	async fn current_session_id_returns_none_outside_scope() {
		assert!(current_session_id().is_none());
	}

	#[tokio::test]
	async fn with_session_id_propagates_id_to_inner_future() {
		let id = unique_id("propagate");
		let observed = with_session_id(id.clone(), async {
			current_session_id().expect("inside scope")
		})
		.await;
		assert_eq!(observed, id);
		// And the id is gone after the scope ends.
		assert!(current_session_id().is_none());
	}

	#[tokio::test]
	async fn with_session_id_propagates_through_spawned_task_when_inherited() {
		// `tokio::task_local!` propagates explicitly via `.scope().await`,
		// not implicitly into `tokio::spawn`. Confirm that pattern: an
		// inner async block sees the id; a detached `tokio::spawn` does not.
		let id = unique_id("scope-vs-spawn");
		with_session_id(id.clone(), async {
			// Direct child future inherits.
			let direct = current_session_id();
			assert_eq!(direct.as_deref(), Some(id.as_str()));

			// Detached spawn does NOT inherit — that's by design.
			let handle = tokio::spawn(async { current_session_id() });
			let spawned = handle.await.unwrap();
			assert!(
				spawned.is_none(),
				"task-local should not leak across tokio::spawn without explicit propagation"
			);
		})
		.await;
	}

	#[test]
	fn active_skills_are_session_scoped() {
		let a = unique_id("skills-a");
		let b = unique_id("skills-b");

		add_active_skill(&a, "programming-rust");
		add_active_skill(&a, "shell");
		add_active_skill(&b, "marketing-backlink");

		assert_eq!(
			get_active_skills(&a),
			vec!["programming-rust".to_string(), "shell".to_string()]
		);
		assert_eq!(
			get_active_skills(&b),
			vec!["marketing-backlink".to_string()]
		);

		assert!(has_active_skill(&a, "shell"));
		assert!(!has_active_skill(&a, "marketing-backlink"));
		assert!(!has_active_skill(&b, "shell"));

		clear_active_skills(&a);
		clear_active_skills(&b);
	}

	#[test]
	fn add_active_skill_is_idempotent() {
		let id = unique_id("idempotent");
		add_active_skill(&id, "shell");
		add_active_skill(&id, "shell");
		add_active_skill(&id, "shell");
		assert_eq!(get_active_skills(&id), vec!["shell".to_string()]);
		clear_active_skills(&id);
	}

	#[test]
	fn remove_active_skill_drops_only_target() {
		let id = unique_id("remove");
		add_active_skill(&id, "shell");
		add_active_skill(&id, "docker");
		add_active_skill(&id, "kubernetes");

		remove_active_skill(&id, "docker");
		assert_eq!(
			get_active_skills(&id),
			vec!["shell".to_string(), "kubernetes".to_string()]
		);
		clear_active_skills(&id);
	}

	#[test]
	fn clear_active_skills_removes_entire_session_entry() {
		let id = unique_id("clear");
		add_active_skill(&id, "shell");
		assert!(!get_active_skills(&id).is_empty());

		clear_active_skills(&id);
		assert!(get_active_skills(&id).is_empty());
		assert!(!has_active_skill(&id, "shell"));
	}

	#[test]
	fn get_active_skills_for_unknown_session_returns_empty() {
		let id = unique_id("unknown");
		assert!(get_active_skills(&id).is_empty());
		assert!(!has_active_skill(&id, "anything"));
	}
}
