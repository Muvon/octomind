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

// Chat session implementation

use super::utils::format_number;
use crate::config::Config;
use crate::session::{
	estimate_full_context_tokens, get_sessions_dir, load_session, CompressionStats, Session,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use colored::Colorize;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Parameters for chat session initialization
///
/// This struct groups all parameters needed for creating or resuming a chat session,
/// following best practices for parameter passing and future extensibility.
pub struct SessionInitParams<'a> {
	/// Optional session name (if None, generates UUID)
	pub name: Option<String>,
	/// Optional session ID to resume
	pub resume: Option<String>,
	/// Resume the most recent session for the current project
	pub resume_recent: bool,
	/// Optional model override
	pub model: Option<String>,
	/// Optional temperature override
	pub temperature: Option<f32>,
	/// Optional max tokens override
	pub max_tokens: Option<u32>,
	/// Optional max retries override
	pub max_retries: Option<u32>,
	/// Output mode: plain or jsonl (for CLI suppression)
	/// Output mode: plain or jsonl (for CLI suppression)
	pub output_mode: Option<String>,
	/// Configuration object
	pub config: &'a Config,
	/// Role for the session
	pub role: &'a str,
	/// The role was named explicitly by the caller (see
	/// [`crate::session::chat::session::GenericSessionArgs::role_explicit`]).
	pub role_explicit: bool,
	/// Optional JSON schema for structured output
	pub schema: Option<serde_json::Value>,
}

impl<'a> SessionInitParams<'a> {
	/// Create new session initialization parameters with required fields
	pub fn new(config: &'a Config, role: &'a str) -> Self {
		Self {
			name: None,
			resume: None,
			resume_recent: false,
			model: None,
			temperature: None,
			max_tokens: None,
			max_retries: None,
			output_mode: None,
			config,
			role,
			role_explicit: false,
			schema: None,
		}
	}

	/// Mark the role as explicitly named by the caller rather than inherited.
	pub fn with_role_explicit(mut self, role_explicit: bool) -> Self {
		self.role_explicit = role_explicit;
		self
	}

	/// Set session name
	pub fn with_name(mut self, name: String) -> Self {
		self.name = Some(name);
		self
	}

	/// Set session to resume
	pub fn with_resume(mut self, resume: String) -> Self {
		self.resume = Some(resume);
		self
	}

	/// Set resume recent flag
	pub fn with_resume_recent(mut self, resume_recent: bool) -> Self {
		self.resume_recent = resume_recent;
		self
	}

	/// Set model override
	pub fn with_model(mut self, model: String) -> Self {
		self.model = Some(model);
		self
	}

	/// Set temperature override
	pub fn with_temperature(mut self, temperature: f32) -> Self {
		self.temperature = Some(temperature);
		self
	}

	/// Set max tokens override
	pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
		self.max_tokens = Some(max_tokens);
		self
	}

	/// Set max retries override
	pub fn with_max_retries(mut self, max_retries: u32) -> Self {
		self.max_retries = Some(max_retries);
		self
	}

	/// Set output mode (plain or jsonl)
	pub fn with_output_mode(mut self, output_mode: String) -> Self {
		self.output_mode = Some(output_mode);
		self
	}

	/// Set JSON schema for structured output
	pub fn with_schema(mut self, schema: serde_json::Value) -> Self {
		self.schema = Some(schema);
		self
	}
}

// Generate a session name in format: YYMMDD-basename-HHMM-uuid4
pub(crate) fn generate_session_name() -> String {
	let now = chrono::Local::now();
	let date_str = now.format("%y%m%d").to_string();
	let time_str = now.format("%H%M").to_string();

	// Get current directory basename - use thread-local if set (ACP/WebSocket sessions), otherwise process cwd
	let current_dir = crate::mcp::get_thread_working_directory();
	let basename = current_dir
		.file_name()
		.unwrap_or_default()
		.to_string_lossy()
		.to_string();

	// Generate a short UUID (first 4 characters)
	let uuid = Uuid::new_v4().to_string();
	let short_uuid: String = uuid.chars().take(4).collect();

	format!("{}-{}-{}-{}", date_str, basename, time_str, short_uuid)
}

// Chat session manager for interactive coding sessions
pub struct ChatSession {
	pub session: Session,
	pub last_response: String,
	/// Monotonic start of the active genuine user turn. Abandoned/cancelled
	/// turns are replaced by the next genuine turn and never enter averages.
	pub(crate) turn_started_at: Option<std::time::Instant>,
	/// The turn's deliverable as first-class state: every final assistant
	/// message (content, no tool calls) since the latest genuine user turn,
	/// oldest first. The verify-gate judges these as ONE deliverable. State,
	/// not a context query — mid-turn compression rewrites the live message
	/// list, and the judged deliverable must not shrink because the context
	/// was compacted (a gate shown only the post-compaction tail "finds"
	/// coverage gaps for everything delivered before it). Cleared on a
	/// genuine user turn, alongside the evidence ledger.
	pub turn_answers: Vec<String>,
	pub model: String,
	pub role: String, // Role for the session
	pub temperature: f32,
	pub top_p: f32, // Top-p nucleus sampling parameter
	pub top_k: u32, // Top-k sampling parameter
	pub max_tokens: u32,
	pub estimated_cost: f64,
	pub cache_next_user_message: bool, // Flag to cache the next user message
	pub spending_threshold_checkpoint: f64, // Track spending at last threshold check
	pub request_spending_checkpoint: f64, // Track spending at start of current request
	pub pending_image: Option<crate::session::image::ImageAttachment>, // Pending image attachment
	pub pending_video: Option<crate::session::video::VideoAttachment>, // Pending video attachment
	pub max_retries: u32,              // Maximum number of retries for provider errors
	pub retry_timeout: u64,
	pub request_timeout_seconds: u64,
	pub was_resumed: bool, // Flag indicating if this session was resumed from an existing file

	pub initial_status_shown: bool, // Flag to track if initial status line was displayed
	// Token calculation cache - SINGLE SOURCE OF TRUTH for context token counting

	// This cache ensures all systems (display, compression) use identical calculations
	pub cached_tools: Option<Vec<crate::mcp::McpFunction>>, // Cached tool definitions for consistent token counting
	/// A background compaction in flight (runtime-only, never persisted): the
	/// paid fold call runs in a spawned task and is applied at a later round
	/// boundary by `conversation_compression`.
	pub fold_job: Option<crate::session::chat::conversation_compression::FoldJob>,
	/// No unforced fold attempt before this API-call index (runtime-only): set
	/// when a background fold fails or is discarded, so a broken folder is
	/// retried on the runway ladder instead of on every round.
	pub fold_cooldown_until_call: usize,
	/// Optional JSON schema for structured output (set via WebSocket/ACP protocol)
	pub schema: Option<serde_json::Value>,
	/// Critical knowledge entries extracted from compressions — persisted across
	/// cycles. Deduped on insert, then trimmed (oldest first) to
	/// `config.compression.knowledge_retention`; set that to `0` for a session
	/// that must never forget.
	pub critical_knowledge: Vec<String>,
	/// Investigation findings accumulated across compactions. Code owns the
	/// canonical set, restores it from the latest rendered summary on resume,
	/// and keeps it within `compression.analysis_findings_max_tokens` using
	/// current-task relevance, recency, and diversity.
	pub analysis_findings: Vec<String>,
	/// Whether the first hybrid scoped recall has happened. Later genuine turns
	/// rebuild the active pack with embedding-only scoped recall.
	pub learning_injected: bool,
	/// One runtime-only memory pack materialized as a non-persisted, replaceable
	/// system-managed message before specialist calls in the active user turn.
	pub active_memory_pack: Option<String>,
	/// Pack-local IDs the specialist reported materially using this turn. Unioned
	/// across tool rounds and consumed by outcome-driven reinforcement.
	pub used_memory_ids: std::collections::HashSet<String>,
	/// Set when a new user message arrives; consumed by the API executor to run
	/// per-message scoped lesson recall (embedding-only) for that turn.
	pub pending_recall: bool,
	/// Whether learning extraction already ran for this session (prevents double extraction on exit).
	pub learning_extracted: bool,
	/// Completion evidence available to detached learning extraction for the
	/// active genuine user trajectory.
	pub learning_outcome: crate::supervisor::learning::TrajectoryOutcome,
	/// Runtime override for reasoning effort (set via /effort). None = use config default.
	pub reasoning_effort: Option<crate::config::ReasoningEffortConfig>,
	/// Supervisor: agent's self-reported state for the latest turn, parsed from
	/// its `<sup>…</sup>` token. Consumed by the verify-gate and detectors.
	pub last_self_report: Option<crate::supervisor::detect::SelfReport>,
	/// Supervisor: deterministic detector state (loop / no-progress streak).
	pub detectors: crate::supervisor::detect::Detectors,
	/// Supervisor: verify-gate re-entry counter for the current turn.
	pub gate_iterations: u8,
	/// Whether the response currently being produced belongs to a genuine user
	/// turn and may therefore claim that user's task complete. System-managed
	/// inbox deliveries (monitor output, schedules, background results, validator
	/// feedback) trigger their own response but are not new user tasks; verifying
	/// those responses against the latest human request creates false positives.
	/// Supervisor repair notes preserve the existing value so a legitimate gate
	/// re-entry still verifies the original user turn.
	pub completion_gate_eligible: bool,
	/// Set when an eligible turn ends with session-owned work still in flight
	/// (a detached shell job, a tap run, a background agent). The inbox delivery
	/// that resumes the agent continues the same task, so it inherits
	/// eligibility instead of being treated as a control-plane event.
	pub gate_deferred: bool,
	/// Supervisor: re-entry counter for the FREE deterministic checks (pre-gate,
	/// plan, coverage, evidence). Deliberately separate from `gate_iterations`:
	/// a zero-cost nudge that the agent then satisfies must not spend the paid
	/// verifier's repair budget, which would fail an otherwise correct turn on
	/// the gate's first gap verdict. Same per-turn bound, own counter.
	pub nudge_iterations: u8,
	/// Supervisor: set when the verify-gate exhausted retries with gaps remaining.
	/// Distill may retain it only as an explicitly failed experience; it can never
	/// be promoted as a verified successful procedure.
	pub gate_failed: bool,
	/// Supervisor: gaps the last verify-gate pass found this task, handed to the
	/// next pass so it confirms each is closed instead of judging from scratch.
	/// Cleared on PASS and on each genuine user turn.
	pub last_gate_gaps: Vec<String>,
	/// Supervisor: queued advisory steer note (loop / no-progress), injected at
	/// the next request's safe pre-build point. None = nothing to steer.
	pub steer_pending: Option<String>,
	/// Supervisor: framing-rotation index for the steer note. When the *same*
	/// signal re-fires without breakout, this advances so `steer_note` reframes the
	/// constraint from a new angle instead of repeating identical (habituated) text.
	/// Reset when the fired signal differs from the last one or on a clean round.
	pub steer_attempt: usize,
	/// Supervisor: the signal that drove the last steer, used to decide whether the
	/// current steer continues the same run (advance framing) or starts fresh (reset).
	pub steer_last_signal: crate::supervisor::detect::DetectorSignal,
	/// Supervisor: order-independent hash (tool name + parameters, NOT result) of the
	/// last round we EMITTED a steer for. An identical hash next round ⇒ the model
	/// re-issued the same calls ⇒ it is IGNORING the steer (vs. a different call-set =
	/// trying). Drives the critical-signal adaptive backoff (ignore-vs-trying gate).
	pub last_steered_calls: Option<u64>,
	/// Supervisor: optional reason from the latest self-report token, fed to the
	/// verify-gate so it checks what the agent claims it did.
	pub last_self_report_reason: Option<String>,
	/// Supervisor: structured continuation handoff from the main agent. Used as
	/// an untrusted attention prior by conversation compression, never as evidence.
	pub last_self_report_handoff: Option<crate::supervisor::detect::SelfReportHandoff>,
	/// Supervisor: execution signal for the external plan controller. The main
	/// specialist can request planning or report phase completion, but cannot
	/// author or mutate the plan itself.
	pub pending_plan_signal: Option<crate::supervisor::plan::PlanSignal>,
	/// Supervisor: a create/no-plan decision already ran for this genuine user
	/// turn. Prevents a declined or unavailable planner from being called again
	/// after every subsequent action batch.
	pub plan_evaluated: bool,
	/// Supervisor: the external planner failed once during this genuine user
	/// turn. Subsequent plan signals are consumed without a planner call to
	/// prevent an unbounded re-emit/fail/inject loop. Reset on new user turn.
	pub planner_failed: bool,
	/// Evidence-ledger boundary for the active plan phase. Only actions at or
	/// after this checkpoint may authorize its transition.
	pub plan_evidence_checkpoint: u64,
	/// Supervisor: entries in the active pack (pack id, content, role, project).
	/// Only IDs the specialist reports using receive verify-gate outcome credit.
	pub recalled_refs: Vec<(String, String, String, String)>,
	/// Supervisor: runtime-recorded tool log for the current task — ground truth
	/// the verify-gate checks completion claims against. Reset on each genuine
	/// user turn; gate/steer re-runs (system-managed messages) keep accumulating.
	pub evidence: crate::supervisor::gate::EvidenceLedger,
	/// Supervisor: stable turn-start task resolution. Reset on each genuine user
	/// turn and cached across planning and completion-gate re-runs.
	pub gate_task: Option<crate::supervisor::resolve::ResolvedTask>,
}

/// Parameters for creating a new ChatSession
pub struct ChatSessionParams {
	pub name: String,
	pub profile: crate::config::ModelProfile,
	pub role: String,
}

impl ChatSession {
	// Create a new chat session
	pub fn new(params: ChatSessionParams) -> Self {
		let profile = params.profile;
		let model_name = profile.model.clone();

		// Create a new session with initial info
		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();

		let session_info = crate::session::SessionInfo {
			name: params.name.clone(),
			created_at: timestamp,
			model: model_name.clone(),
			role: params.role.clone(),
			input_tokens: 0,
			output_tokens: 0,
			cache_read_tokens: 0,
			cache_write_tokens: 0,
			reasoning_tokens: 0,
			total_cost: 0.0,
			duration_seconds: 0,
			layer_stats: Vec::new(), // Initialize empty layer stats
			tool_calls: 0,           // Initialize tool call counter
			// Initialize time tracking fields
			total_api_time_ms: 0,
			total_tool_time_ms: 0,
			total_layer_time_ms: 0,
			turn_timing: crate::session::TurnTimingStats::default(),
			compression_stats: CompressionStats::default(),
			anchor: crate::session::anchor::Anchor::default(),
			total_api_calls: 0,
			// Initialize cache state
			current_non_cached_tokens: 0,
			current_total_tokens: 0,
			last_cache_checkpoint_time: timestamp,
			// Initialize runtime state
			cache_next_user_message: false,
			spending_threshold_checkpoint: 0.0,

			context_tokens_after_last_compression: 0,
			turn_call_counts: Vec::new(),
			api_calls_at_turn_start: 0,
			api_calls_at_last_compression: 0,
			output_tokens_at_last_compression: 0,
			consecutive_compressions: 0,
			learning_stats: crate::session::LearningSessionStats::default(),
			verification_policy: crate::supervisor::VerificationPolicy::default(),
			authorization: crate::supervisor::authorizer::AuthorizationState::default(),
			evidence: crate::supervisor::gate::EvidenceLedger::default(),
		};

		let session = Session {
			info: session_info,
			messages: Vec::new(),
			session_file: None,
		};

		Self {
			session,
			last_response: String::new(),
			turn_started_at: None,
			turn_answers: Vec::new(),
			model: model_name,
			role: params.role,
			temperature: profile.temperature,
			top_p: profile.top_p,
			top_k: profile.top_k,
			max_tokens: profile.max_tokens,
			estimated_cost: 0.0,                // Initialize estimated cost as zero
			cache_next_user_message: false,     // Initialize cache flag
			spending_threshold_checkpoint: 0.0, // Initialize spending checkpoint
			request_spending_checkpoint: 0.0,   // Initialize request spending checkpoint
			pending_image: None,                // Initialize pending image
			pending_video: None,                // Initialize pending video
			max_retries: profile.max_retries,
			retry_timeout: profile.retry_timeout,
			request_timeout_seconds: profile.request_timeout_seconds,
			was_resumed: false,          // This is a new session
			initial_status_shown: false, // Initialize status display flag
			cached_tools: None,          // Initialize tool cache (populated on first use)
			fold_job: None,
			fold_cooldown_until_call: 0,
			schema: None, // Schema set later via CLI override
			critical_knowledge: Vec::new(),
			analysis_findings: Vec::new(),
			learning_injected: false,
			active_memory_pack: None,
			used_memory_ids: std::collections::HashSet::new(),
			pending_recall: false,
			learning_extracted: false,
			learning_outcome: crate::supervisor::learning::TrajectoryOutcome::Unknown,
			reasoning_effort: Some(profile.reasoning_effort),
			last_self_report: None,
			detectors: crate::supervisor::detect::Detectors::default(),
			gate_iterations: 0,
			completion_gate_eligible: true,
			gate_deferred: false,
			nudge_iterations: 0,
			gate_failed: false,
			last_gate_gaps: Vec::new(),
			steer_pending: None,
			steer_attempt: 0,
			steer_last_signal: crate::supervisor::detect::DetectorSignal::None,
			last_steered_calls: None,
			last_self_report_reason: None,
			last_self_report_handoff: None,
			pending_plan_signal: None,
			plan_evaluated: false,
			planner_failed: false,
			plan_evidence_checkpoint: 0,
			recalled_refs: Vec::new(),
			evidence: crate::supervisor::gate::EvidenceLedger::default(),
			gate_task: None,
		}
	}

	// Initialize a new chat session or load existing one
	pub async fn initialize(params: SessionInitParams<'_>) -> Result<Self> {
		let sessions_dir = get_sessions_dir()?;

		// Handle resume_recent flag
		let effective_resume = if params.resume_recent {
			// Get current working directory - use thread-local if set (ACP/WebSocket), otherwise process cwd
			let current_dir = crate::mcp::get_thread_working_directory();
			// Find the most recent session for this project
			match crate::session::find_most_recent_session_for_project(&current_dir)? {
				Some(session_name) => {
					use colored::*;
					println!(
						"{}",
						format!(
							"✓ Found recent session for current project: {}",
							session_name
						)
						.bright_green()
					);
					Some(session_name)
				}
				None => {
					use colored::*;
					println!(
						"{}",
						"⚠ No recent session found for current project. Creating new session."
							.yellow()
					);
					None
				}
			}
		} else {
			params.resume.clone()
		};

		// Determine session name
		let session_name = if let Some(name_arg) = &params.name {
			name_arg.clone()
		} else if let Some(resume_name) = &effective_resume {
			resume_name.clone()
		} else {
			// Generate a name using the new format
			generate_session_name()
		};

		let session_file = sessions_dir.join(format!("{}.jsonl.zst", session_name));

		let runtime_profile = crate::config::ModelProfileOverride {
			model: params.model.clone(),
			temperature: params.temperature,
			max_tokens: params.max_tokens,
			max_retries: params.max_retries,
			..Default::default()
		};
		let effective_profile =
			runtime_profile.resolve(&params.config.get_model_profile_for_role(params.role));

		// Check if we should load or create a session
		let should_resume = if effective_resume.is_some() {
			// Explicit resume request - session MUST exist
			if !session_file.exists() {
				return Err(anyhow::anyhow!(
					"Session '{}' not found. Cannot resume non-existent session.",
					session_name
				));
			}
			true
		} else if params.name.is_some() && session_file.exists() {
			// Named session that exists - resume it
			true
		} else {
			// Create new session
			false
		};

		if should_resume {
			use colored::*;

			// Try to load session
			match load_session(&session_file) {
				Ok(session) => {
					// Extract runtime state from session log
					let runtime_state =
						crate::session::extract_runtime_state_from_log(&session_file)
							.unwrap_or_default();

					// Skip CLI output in structured output modes (websocket, jsonl)
					let suppress = crate::session::output::OutputMode::from_runtime_mode(
						params.output_mode.as_deref().unwrap_or("plain"),
					)
					.should_suppress_cli_output();

					if !suppress {
						// When session is loaded successfully, show its info
						println!(
							"{}",
							format!("✓ Resuming session: {}", session_name).bright_green()
						);
						if let Some(title) = crate::session::titles::get_session_meta(&session_name)
							.and_then(|m| m.title)
						{
							println!("{} {}", "Title:".blue(), title.white());
						}

						// Show a brief summary of the session
						let created_time =
							DateTime::<Utc>::from_timestamp(session.info.created_at as i64, 0)
								.map(|dt| dt.naive_local().format("%Y-%m-%d %H:%M:%S").to_string())
								.unwrap_or_else(|| "Unknown".to_string());

						// Simplify model name
						let model_parts: Vec<&str> = session.info.model.split('/').collect();
						let model_name = if model_parts.len() > 1 {
							model_parts[1]
						} else {
							&session.info.model
						};

						// Calculate total tokens
						let total_tokens = session.info.input_tokens
							+ session.info.output_tokens
							+ session.info.cache_read_tokens
							+ session.info.cache_write_tokens;

						println!("{} {}", "Created:".blue(), created_time.white());
						println!("{} {}", "Model:".blue(), model_name.yellow());
						println!(
							"{} {}",
							"Messages:".blue(),
							session.messages.len().to_string().white()
						);
						println!(
							"{} {}",
							"Tokens:".blue(),
							format_number(total_tokens).bright_blue()
						);
						println!(
							"{} ${:.5}",
							"Cost:".blue(),
							session.info.total_cost.to_string().bright_magenta()
						);
					}

					// Create chat session from loaded session
					let restored_model = session.info.model.clone(); // Extract model before moving session
					let restored_cost = session.info.total_cost; // Extract cost before moving session

					// Restore runtime state from session.info
					let cache_next = session.info.cache_next_user_message;
					let spending_checkpoint = session.info.spending_threshold_checkpoint;
					// Restore the verify-gate's evidence ledger for the still-open
					// turn. The gate re-derives its conditions from the persisted
					// request; the recorded actions that satisfy them must survive
					// the restart too, or every resumed turn reports false gaps.
					let restored_evidence = session.info.evidence.clone();

					let mut chat_session = ChatSession {
						session,
						last_response: String::new(),
						turn_started_at: None,
						turn_answers: Vec::new(),
						model: restored_model,         // Use restored model from session
						role: params.role.to_string(), // Add role from params
						temperature: effective_profile.temperature,
						top_p: effective_profile.top_p,
						top_k: effective_profile.top_k,
						max_tokens: effective_profile.max_tokens,
						estimated_cost: restored_cost, // FIXED: Use actual cost from session
						cache_next_user_message: cache_next, // Restore from session.info
						spending_threshold_checkpoint: spending_checkpoint, // Restore from session.info
						request_spending_checkpoint: 0.0, // Initialize request spending checkpoint
						pending_image: None,           // Initialize pending image
						pending_video: None,           // Initialize pending video
						max_retries: effective_profile.max_retries,
						retry_timeout: effective_profile.retry_timeout,
						request_timeout_seconds: effective_profile.request_timeout_seconds,
						was_resumed: true,          // This session was resumed from file
						initial_status_shown: true, // Don't show status for resumed sessions
						cached_tools: None,         // Initialize tool cache (populated on first use)
						fold_job: None,
						fold_cooldown_until_call: 0,
						schema: None,                   // Schema applied after init via CLI override
						critical_knowledge: Vec::new(), // Will be restored from session log below
						analysis_findings: Vec::new(),
						learning_injected: false,
						active_memory_pack: None,
						used_memory_ids: std::collections::HashSet::new(),
						pending_recall: false,
						learning_extracted: false,
						learning_outcome: crate::supervisor::learning::TrajectoryOutcome::Unknown,
						reasoning_effort: Some(effective_profile.reasoning_effort),
						last_self_report: None,
						detectors: crate::supervisor::detect::Detectors::default(),
						gate_iterations: 0,
						completion_gate_eligible: true,
						gate_deferred: false,
						nudge_iterations: 0,
						gate_failed: false,
						last_gate_gaps: Vec::new(),
						steer_pending: None,
						steer_attempt: 0,
						steer_last_signal: crate::supervisor::detect::DetectorSignal::None,
						last_steered_calls: None,
						last_self_report_reason: None,
						last_self_report_handoff: None,
						pending_plan_signal: None,
						plan_evaluated: false,
						planner_failed: false,
						plan_evidence_checkpoint: 0,
						recalled_refs: Vec::new(),
						evidence: restored_evidence,
						gate_task: None,
					};
					// Keep session.info.role in sync with the active role
					chat_session.session.info.role = params.role.to_string();

					// Apply runtime state from session log (legacy support)
					if runtime_state.cache_next_message {
						chat_session.cache_next_user_message = true;
					}

					// Apply restored role if available. An explicitly named role is a
					// deliberate switch and outranks whatever `/role` the session
					// last logged — otherwise `run reviewer --resume x` would snap
					// straight back to the old role.
					if let Some(restored_role) =
						runtime_state.role.filter(|_| !params.role_explicit)
					{
						// Validate that the restored role still exists in config
						if params.config.roles.iter().any(|r| r.name == restored_role) {
							chat_session.role = restored_role;
							// Update temperature and model from the restored role config
							let role_profile =
								params.config.get_model_profile_for_role(&chat_session.role);
							chat_session.apply_model_profile(&role_profile);
						}
					}

					// Restore critical knowledge entries from session log
					if !runtime_state.critical_knowledge.is_empty() {
						chat_session.critical_knowledge = runtime_state.critical_knowledge;
						crate::log_debug!(
							"Session resume: Restored {} critical knowledge entries",
							chat_session.critical_knowledge.len()
						);
					}

					// Restore runtime reasoning effort override
					if let Some(effort) = runtime_state.reasoning_effort {
						chat_session.reasoning_effort = Some(effort);
					}

					// CRITICAL FIX: Recalculate token tracking from actual messages
					// After compression, persisted counters are reset to 0, but messages remain.
					// On resume, we must recalculate from actual message content to restore correct state.
					// This ensures cache thresholds and token counts reflect reality, not stale persisted values.
					let cache_manager = crate::session::cache::CacheManager::new();
					let (total_tokens, non_cached_tokens) =
						cache_manager.estimate_current_session_tokens(&chat_session.session);
					chat_session.session.info.current_total_tokens = total_tokens;
					chat_session.session.info.current_non_cached_tokens = non_cached_tokens;

					crate::log_debug!(
					"Session resume: Recalculated token state - total: {}, non-cached: {} (from {} messages)",
					total_tokens,
					non_cached_tokens,
					chat_session.session.messages.len()
				);

					// Get last assistant response if any
					for msg in chat_session.session.messages.iter().rev() {
						if msg.role == "assistant" {
							chat_session.last_response = msg.content.clone();
							break;
						}
					}

					// Seed the turn-answer ledger from the loaded transcript: the
					// resumed turn's finals were appended live in the prior process,
					// and the verify-gate must judge the same deliverable after a
					// resume as before it.
					let turn_start =
						crate::session::latest_task_turn_index(&chat_session.session.messages)
							.unwrap_or(chat_session.session.messages.len());
					chat_session.turn_answers = chat_session.session.messages[turn_start..]
						.iter()
						.filter(|m| {
							m.role == "assistant"
								&& m.tool_calls.is_none()
								&& !m.content.trim().is_empty()
						})
						.map(|m| m.content.clone())
						.collect();

					Ok(chat_session)
				}
				Err(e) => {
					// If this was an explicit resume request, return the error
					if params.resume.is_some() {
						return Err(anyhow::anyhow!(
							"Failed to load session '{}': {}. Cannot resume corrupted or invalid session.",
							session_name,
							e
						));
					}

					// If loading fails for named session, inform the user and create a new session
					println!(
						"{}: {}",
						format!("Failed to load session {}", session_name).bright_red(),
						e
					);
					println!("{}", "Creating a new session instead...".yellow());

					// Generate a new unique session name using the new format
					let new_session_name = generate_session_name();
					let new_session_file =
						sessions_dir.join(format!("{}.jsonl.zst", new_session_name));

					// Skip CLI output in structured output modes
					let suppress = crate::session::output::OutputMode::from_runtime_mode(
						params.output_mode.as_deref().unwrap_or("plain"),
					)
					.should_suppress_cli_output();
					if !suppress {
						println!(
							"{}",
							format!("Starting new session: {}", new_session_name).bright_green()
						);
					}

					let mut chat_session = ChatSession::new(ChatSessionParams {
						name: new_session_name.clone(),
						profile: effective_profile.clone(),
						role: params.role.to_string(),
					});
					chat_session.session.session_file = Some(new_session_file);

					Ok(chat_session)
				}
			}
		} else {
			// Create new session - skip CLI output in structured output modes
			let suppress = crate::session::output::OutputMode::from_runtime_mode(
				params.output_mode.as_deref().unwrap_or("plain"),
			)
			.should_suppress_cli_output();
			if !suppress {
				use colored::*;
				println!(
					"{}",
					format!("Starting new session: {}", session_name).bright_green()
				);
			}

			let mut chat_session = ChatSession::new(ChatSessionParams {
				name: session_name.clone(),
				profile: effective_profile,
				role: params.role.to_string(),
			});
			chat_session.session.session_file = Some(session_file);

			Ok(chat_session)
		}
	}

	/// Get the effective model for this session (uses session.info.model directly)
	pub fn get_effective_model(&self) -> &str {
		&self.session.info.model
	}

	pub fn model_profile(&self, config: &Config) -> crate::config::ModelProfile {
		crate::config::ModelProfile {
			model: self.model.clone(),
			reasoning_effort: self.reasoning_effort.unwrap_or(config.reasoning_effort),
			max_tokens: self.max_tokens,
			temperature: self.temperature,
			top_p: self.top_p,
			top_k: self.top_k,
			max_retries: self.max_retries,
			retry_timeout: self.retry_timeout,
			request_timeout_seconds: self.request_timeout_seconds,
		}
	}

	pub fn apply_model_profile(&mut self, profile: &crate::config::ModelProfile) {
		self.model = profile.model.clone();
		self.session.info.model = profile.model.clone();
		self.temperature = profile.temperature;
		self.top_p = profile.top_p;
		self.top_k = profile.top_k;
		self.max_tokens = profile.max_tokens;
		self.max_retries = profile.max_retries;
		self.retry_timeout = profile.retry_timeout;
		self.request_timeout_seconds = profile.request_timeout_seconds;
		self.reasoning_effort = Some(profile.reasoning_effort);
	}

	/// Attach image from file path
	pub async fn attach_image_from_path(&mut self, path: &str) -> Result<()> {
		use crate::session::image::ImageProcessor;
		use std::path::Path;

		self.ensure_model_supports_vision()?;

		if ImageProcessor::is_url(path) {
			println!("{}", "🌐 Downloading image from URL...".bright_cyan());
			let attachment = ImageProcessor::load_from_url(path).await?;
			println!("{}", "📸 Image preview:".bright_cyan());
			ImageProcessor::show_preview(&attachment)?;
			self.pending_image = Some(attachment);
			println!(
				"{}",
				"✅ Image downloaded and ready to attach!".bright_green()
			);
			return Ok(());
		}

		let file_path = Path::new(path);
		if !file_path.exists() {
			return Err(anyhow::anyhow!("Image file not found: {}", path));
		}
		if !ImageProcessor::is_supported_image(file_path) {
			return Err(anyhow::anyhow!(
				"Unsupported image format. Supported: {}",
				ImageProcessor::supported_extensions().join(", ")
			));
		}

		let attachment = ImageProcessor::load_from_path(file_path)?;
		println!("{}", "📸 Image preview:".bright_cyan());
		ImageProcessor::show_preview(&attachment)?;
		self.pending_image = Some(attachment);
		Ok(())
	}

	/// Try to attach image from clipboard
	pub async fn try_attach_from_clipboard(&mut self) -> Result<bool> {
		use crate::session::image::ImageProcessor;

		self.ensure_model_supports_vision()?;

		match ImageProcessor::load_from_clipboard()? {
			Some(image_attachment) => {
				println!("{}", "📋 Image detected in clipboard!".bright_cyan());

				// Show preview
				println!("{}", "📸 Image preview:".bright_cyan());
				ImageProcessor::show_preview(&image_attachment)?;

				// Store for next message
				self.pending_image = Some(image_attachment);

				println!("{}", "✅ Clipboard image ready to attach!".bright_green());
				Ok(true)
			}
			None => Ok(false),
		}
	}

	/// Check if there's a pending image attachment
	pub fn has_pending_image(&self) -> bool {
		self.pending_image.is_some()
	}

	/// Take the pending image (consumes it)
	pub fn take_pending_image(&mut self) -> Option<crate::session::image::ImageAttachment> {
		self.pending_image.take()
	}

	/// Refuse known text-only models while leaving unknown proxy routes permissive.
	pub fn ensure_model_supports_vision(&self) -> Result<()> {
		match crate::session::model_utils::model_supports_vision(&self.model) {
			Ok(true) => Ok(()),
			Ok(false) => Err(anyhow::anyhow!(
				"Current model '{}' does not support vision. Switch to a vision-capable model with /model before attaching an image.",
				self.model
			)),
			Err(error) => Err(anyhow::anyhow!(
				"Unable to check vision support for current model '{}': {}",
				self.model,
				error
			)),
		}
	}

	/// Attach video from file path
	pub async fn attach_video_from_path(&mut self, path: &str) -> Result<()> {
		use crate::session::video::VideoProcessor;
		use std::path::Path;

		self.ensure_model_supports_video()?;

		if VideoProcessor::is_url(path) {
			println!("{}", "🌐 Downloading video from URL...".bright_cyan());
			let attachment = VideoProcessor::load_from_url(path).await?;
			println!("{}", "🎬 Video preview:".bright_cyan());
			VideoProcessor::show_preview(&attachment)?;
			self.pending_video = Some(attachment);
			println!(
				"{}",
				"✅ Video downloaded and ready to attach!".bright_green()
			);
			return Ok(());
		}

		let file_path = Path::new(path);
		if !file_path.exists() {
			return Err(anyhow::anyhow!("Video file not found: {}", path));
		}
		if !VideoProcessor::is_supported_video(file_path) {
			return Err(anyhow::anyhow!(
				"Unsupported video format. Supported: {}",
				VideoProcessor::supported_extensions().join(", ")
			));
		}

		let attachment = VideoProcessor::load_from_path(file_path)?;
		println!("{}", "🎬 Video preview:".bright_cyan());
		VideoProcessor::show_preview(&attachment)?;
		self.pending_video = Some(attachment);
		Ok(())
	}

	/// Check if there's a pending video attachment
	pub fn has_pending_video(&self) -> bool {
		self.pending_video.is_some()
	}

	/// Take the pending video (consumes it)
	pub fn take_pending_video(&mut self) -> Option<crate::session::video::VideoAttachment> {
		self.pending_video.take()
	}

	/// Refuse known non-video models while leaving unknown proxy routes permissive.
	pub fn ensure_model_supports_video(&self) -> Result<()> {
		match crate::session::model_utils::model_supports_video(&self.model) {
			Ok(true) => Ok(()),
			Ok(false) => Err(anyhow::anyhow!(
				"Current model '{}' does not support video. Switch to a video-capable model with /model before attaching a video.",
				self.model
			)),
			Err(error) => Err(anyhow::anyhow!(
				"Unable to check video support for current model '{}': {}",
				self.model,
				error
			)),
		}
	}

	/// Process user commands
	pub async fn process_command(
		&mut self,
		input: &str,
		config: &mut Config,
		role: &str,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<super::commands::CommandResult> {
		super::commands::process_command(self, input, config, role, operation_cancelled).await
	}

	/// Get current message count (for plan compression tracking)
	pub fn get_message_count(&self) -> usize {
		self.session.messages.len()
	}

	/// Remove messages in specified range for compression
	///
	/// This method safely removes messages between start_index (exclusive) and end_index (inclusive).
	/// It preserves the message at start_index and removes everything up to and including end_index.
	/// The compressed summary will be inserted at start_index + 1.
	///
	/// # Index Semantics (CRITICAL)
	///
	/// - Uses **inclusive range** for removal: `drain(start_index + 1..=end_index)`
	/// - `end_index` must be **< messages.len()** (last valid index is `len() - 1`)
	/// - `end_index >= messages.len()` will return an error (out of bounds for inclusive range)
	///
	/// # Arguments
	/// * `start_index` - Start of range (this message is kept)
	/// * `end_index` - End of range (messages up to and including this are removed)
	///
	/// # Returns
	/// Tuple of (messages_removed, had_cached_messages)
	/// - messages_removed: Number of messages actually removed
	/// - had_cached_messages: True if any removed message had cached=true (informational only)
	///
	/// # Example
	///
	/// If start_index=5 and end_index=10:
	/// - Message 5 is kept (e.g., "Let me investigate...")
	/// - Messages 6, 7, 8, 9, 10 are removed (tool results, plan result)
	/// - Compressed summary inserted after message 5
	///
	/// # Common Pitfall
	///
	/// **DO NOT** use `messages.len()` as end_index - it will fail!
	/// - WRONG: `session.remove_messages_in_range(start, session.get_message_count());`
	/// - CORRECT: `session.remove_messages_in_range(start, session.get_message_count() - 1);`
	pub fn remove_messages_in_range(
		&mut self,
		start_index: usize,
		end_index: usize,
	) -> Result<(usize, bool)> {
		// Validate range
		if start_index >= self.session.messages.len() {
			return Err(anyhow::anyhow!(
				"Invalid start_index: {} (total messages: {})",
				start_index,
				self.session.messages.len()
			));
		}

		if end_index >= self.session.messages.len() {
			return Err(anyhow::anyhow!(
				"Invalid end_index: {} (total messages: {}). end_index must be less than total messages since removal uses inclusive range.",
				end_index,
				self.session.messages.len()
			));
		}

		if start_index >= end_index {
			return Err(anyhow::anyhow!(
				"Invalid range: start_index ({}) must be less than end_index ({})",
				start_index,
				end_index
			));
		}

		// Calculate how many messages to remove (inclusive end_index)
		let messages_to_remove = end_index - start_index;

		if messages_to_remove == 0 {
			crate::log_debug!(
				"No messages to remove in range {}-{}",
				start_index,
				end_index
			);
			return Ok((0, false));
		}

		// CRITICAL: Check if any messages being removed have cached=true
		// This preserves the 2-marker cache system during compression
		let had_cached = self.session.messages[start_index + 1..=end_index]
			.iter()
			.any(|msg| msg.cached);

		// Remove messages from start_index+1 through end_index (inclusive)
		// Using ..= for inclusive end index
		self.session.messages.drain(start_index + 1..=end_index);

		// Reset tool-result dedup state. Any of our placeholders that
		// referenced messages in the just-drained range now point at
		// vanished content; future identical results must be kept verbatim
		// again. Centralized here so every caller that drops messages
		// (task / phase / project / conversation compaction, manual
		// truncate, manual summarize, future paths) gets it for free
		// without needing to remember a separate cleanup call.
		crate::session::dedup::clear_current_session();

		crate::log_debug!(
			"Compressed {} messages (range {}-{}), had_cached={}",
			messages_to_remove,
			start_index,
			end_index,
			had_cached
		);

		Ok((messages_to_remove, had_cached))
	}

	/// Insert compressed knowledge entry as assistant message
	///
	/// This injects a structured summary of completed work into the session history,
	/// replacing the detailed tool calls and intermediate steps.
	///
	/// The compressed block is always marked `cached=true` — it is the new stable
	/// cache boundary for Anthropic's 2-marker system. Any surviving marker at
	/// `index` (start_idx kept message) remains untouched, giving us up to 2 markers.
	///
	/// # Arguments
	/// * `index` - Position to insert after (the kept start_idx message)
	/// * `content` - Formatted summary content
	pub fn insert_compressed_knowledge(&mut self, index: usize, content: String) -> Result<()> {
		use crate::session::Message;

		if index >= self.session.messages.len() {
			return Err(anyhow::anyhow!(
				"Invalid index: {} (total messages: {})",
				index,
				self.session.messages.len()
			));
		}

		// Enforce the 2-marker limit BEFORE inserting the compressed block.
		// Count existing non-system content markers; if already at 2, evict the
		// oldest one so the new compressed block can take its slot.  This prevents
		// exceeding Anthropic's hard limit of 4 cache_control blocks per request
		// (system + tools + 2 content markers).
		// Skip cache marker management entirely when the model does not support caching.
		const MAX_CONTENT_MARKERS: usize = 2;
		let supports_caching = crate::session::model_supports_caching(&self.session.info.model);

		if supports_caching {
			let existing: Vec<usize> = self
				.session
				.messages
				.iter()
				.enumerate()
				.filter(|(_, m)| m.cached && m.role != "system")
				.map(|(i, _)| i)
				.collect();

			if existing.len() >= MAX_CONTENT_MARKERS {
				// Evict the oldest marker to make room for the compressed block.
				if let Some(oldest) = existing.first() {
					if let Some(m) = self.session.messages.get_mut(*oldest) {
						m.cached = false;
					}
				}
			}
		}

		let compressed_msg = Message {
			role: "assistant".to_string(),
			content,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			// The compressed block is the new stable history boundary — cached only
			// when the model actually supports cache markers.
			cached: supports_caching,
			cache_ttl: None,
			tool_call_id: None,
			name: Some("plan_compression".to_string()),
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		};

		self.session.messages.insert(index + 1, compressed_msg);
		let compressed_idx = index + 1;

		crate::log_debug!(
			"Inserted compressed knowledge at index {} (cached={})",
			compressed_idx,
			supports_caching
		);

		// Ensure we always have 2 content markers after compression:
		// marker #1 = compressed block (just inserted above),
		// marker #2 = last eligible user/tool message in the preserved zone.
		// Without this, compression can destroy the second marker leaving only 1,
		// which means the entire preserved tail is sent uncached on the next API call.
		// Skip entirely when the model does not support caching.
		if !supports_caching {
			return Ok(());
		}

		let last_eligible = self
			.session
			.messages
			.iter()
			.enumerate()
			.rev()
			.find(|(i, m)| *i > compressed_idx && (m.role == "user" || m.role == "tool"))
			.map(|(i, _)| i);

		if let Some(target_idx) = last_eligible {
			if !self.session.messages[target_idx].cached {
				let markers: Vec<usize> = self
					.session
					.messages
					.iter()
					.enumerate()
					.filter(|(_, m)| m.cached && m.role != "system")
					.map(|(i, _)| i)
					.collect();

				if markers.len() >= MAX_CONTENT_MARKERS {
					let marker_to_remove = markers
						.iter()
						.copied()
						.find(|i| *i != compressed_idx)
						.or_else(|| markers.first().copied());

					if let Some(index) = marker_to_remove {
						if let Some(m) = self.session.messages.get_mut(index) {
							m.cached = false;
						}
					}
				}

				if let Some(m) = self.session.messages.get_mut(target_idx) {
					m.cached = true;
					crate::log_debug!(
						"Placed second cache marker at index {} (role={})",
						target_idx,
						m.role
					);
				}
			}
		}

		Ok(())
	}

	/// Reinitialize session for new role - updates system prompt and MCP servers
	pub async fn reinitialize_for_role(
		&mut self,
		new_role: &str,
		config: &crate::config::Config,
	) -> anyhow::Result<()> {
		use crate::session::create_system_prompt;
		use colored::Colorize;

		// Get current directory for system prompt processing
		// Get current directory for system prompt processing - use thread-local if set (ACP/WebSocket), otherwise process cwd
		let current_dir = crate::mcp::get_thread_working_directory();
		let config_for_role = config.get_merged_config_for_role(new_role);

		// Shutdown existing MCP servers first
		if let Err(e) = crate::mcp::process::stop_all_servers() {
			println!(
				"{}: {}",
				"Warning: Failed to stop existing MCP servers".bright_yellow(),
				e
			);
		}

		// Mirror the session-startup boot sequence so /role swaps get the same
		// tool surface a fresh session for the new role would get: static MCP
		// init + env-driven skills + env-driven capabilities. Without the
		// env-* steps, tools from OCTOMIND_SKILLS / OCTOMIND_CAPABILITIES
		// (e.g. playwright's browser_*) wouldn't register for the new role,
		// and the routing ownership check would reject calls with
		// "belongs to another session".
		if let Err(e) = crate::mcp::initialize_mcp_for_role(new_role, config).await {
			println!(
				"{}: {}",
				"Warning: Failed to initialize MCP for new role".bright_yellow(),
				e
			);
			println!("{}", "Some tools may not be available".yellow());
		} else {
			println!(
				"{}",
				"✓ MCP servers and tools updated for new role".bright_green()
			);
		}

		// Load env-driven skills and capabilities for the new role BEFORE the
		// system prompt is rebuilt so the prompt reflects the resulting tool
		// surface. Both calls are idempotent; the underlying registries guard
		// against double activation.
		crate::mcp::runtime::skill_auto::load_env_skills(self).await;
		crate::mcp::runtime::capability::load_env_capabilities(&config_for_role, None).await;

		// Create new system prompt for the role (AFTER MCP servers are initialized)
		// This ensures the tools definition reflects the new role's available tools
		let new_system_prompt =
			create_system_prompt(&current_dir, &config_for_role, new_role).await;

		// Find and replace the first system message (should be index 0)
		if let Some(first_msg) = self.session.messages.first_mut() {
			if first_msg.role == "system" {
				// Replace the system message content.
				// Persistence: the new system prompt is reflected on the next session save
				// (or by the caller's own session-file append logic for mutations).
				first_msg.content = new_system_prompt;

				println!(
					"{}",
					"✓ System prompt updated with new role's tools".bright_green()
				);
			} else {
				// This shouldn't happen in normal operation, but handle gracefully
				return Err(anyhow::anyhow!(
					"Expected first message to be system message, found: {}",
					first_msg.role
				));
			}
		} else {
			// No messages yet - add system message (shouldn't happen for role switching)
			self.add_system_message(&new_system_prompt)?;
			println!(
				"{}",
				"✓ System prompt initialized for new role".bright_green()
			);
		}

		// Save the session to persist the changes
		self.save()?;

		Ok(())
	}

	/// UNIFIED TOKEN CALCULATION - SINGLE SOURCE OF TRUTH
	///
	/// This method ensures ALL systems (display, compression, continuation, etc.) use
	/// IDENTICAL token calculations by:
	/// 1. Caching tool definitions to avoid repeated async fetches
	/// 2. Using the exact same estimate_full_context_tokens() function
	/// 3. Including system prompt + tools for accurate context size
	///
	/// **CRITICAL**: This is the ONLY method that should be used for context token counting.
	/// Direct calls to estimate_full_context_tokens() should be replaced with this method.
	///
	/// # Arguments
	/// * `config` - Configuration to fetch tools if not cached
	///
	/// # Returns
	/// Total context tokens including messages + system prompt + tool definitions
	pub async fn get_full_context_tokens(&mut self, config: &Config) -> usize {
		// Fetch and cache tools if not already cached
		if self.cached_tools.is_none() {
			self.cached_tools = Some(crate::mcp::get_available_functions(config).await);
		}

		// System prompt is already included in session.messages. The active memory
		// pack normally is not: it materializes only around the provider request, so
		// account for its bounded request cost explicitly when absent from the slice.
		let mut total =
			estimate_full_context_tokens(&self.session.messages, self.cached_tools.as_deref());
		if !self
			.session
			.messages
			.iter()
			.any(|message| message.name.as_deref() == Some("__active_memory_pack"))
		{
			if let Some(pack) = self.active_memory_pack.as_deref() {
				let content = crate::session::ensure_system_managed(pack);
				let mut message = crate::session::Session::build_message("user", &content);
				message.name = Some("__active_memory_pack".to_string());
				total = total.saturating_add(crate::session::estimate_message_tokens(&message));
			}
		}
		total
	}

	/// Invalidate tool cache (call when MCP configuration changes)
	pub fn invalidate_tool_cache(&mut self) {
		self.cached_tools = None;
	}

	pub(crate) fn begin_turn_timing(&mut self) {
		self.turn_started_at = Some(std::time::Instant::now());
	}

	pub(crate) fn finish_turn_timing(&mut self) {
		if !self.completion_gate_eligible {
			self.turn_started_at = None;
			return;
		}
		if let Some(started_at) = self.turn_started_at.take() {
			self.session.info.turn_timing.record(started_at.elapsed());
		}
	}

	pub(crate) fn abandon_turn_timing(&mut self) {
		self.turn_started_at = None;
	}
}

#[cfg(test)]
impl ChatSession {
	/// Minimal ChatSession for unit tests — no config, no providers, no IO.
	/// Shared across the crate's test modules (core, cost tracking, messages);
	/// keep new runtime fields initialized here so every test picks them up.
	pub(crate) fn for_tests(messages: Vec<crate::session::Message>) -> Self {
		let info = crate::session::SessionInfo {
			name: "test".to_string(),
			model: "anthropic/claude-3-5-sonnet".to_string(),
			..Default::default()
		};
		ChatSession {
			session: crate::session::Session {
				info,
				messages,
				session_file: None,
			},
			last_response: String::new(),
			turn_started_at: None,
			turn_answers: Vec::new(),
			model: "anthropic/claude-3-5-sonnet".to_string(),
			role: "core".to_string(),
			temperature: 0.7,
			top_p: 1.0,
			top_k: 0,
			max_tokens: 4096,
			estimated_cost: 0.0,
			cache_next_user_message: false,
			spending_threshold_checkpoint: 0.0,
			request_spending_checkpoint: 0.0,
			pending_image: None,
			pending_video: None,
			max_retries: 0,
			retry_timeout: 30,
			request_timeout_seconds: 300,

			was_resumed: false,
			initial_status_shown: false,
			cached_tools: None,
			fold_job: None,
			fold_cooldown_until_call: 0,
			schema: None,
			critical_knowledge: Vec::new(),
			analysis_findings: Vec::new(),
			learning_injected: false,
			active_memory_pack: None,
			used_memory_ids: std::collections::HashSet::new(),
			pending_recall: false,
			learning_extracted: false,
			learning_outcome: crate::supervisor::learning::TrajectoryOutcome::Unknown,
			reasoning_effort: None,
			last_self_report: None,
			detectors: crate::supervisor::detect::Detectors::default(),
			gate_iterations: 0,
			completion_gate_eligible: true,
			gate_deferred: false,
			nudge_iterations: 0,
			gate_failed: false,
			last_gate_gaps: Vec::new(),
			steer_pending: None,
			steer_attempt: 0,
			steer_last_signal: crate::supervisor::detect::DetectorSignal::None,
			last_steered_calls: None,
			last_self_report_reason: None,
			last_self_report_handoff: None,
			pending_plan_signal: None,
			plan_evaluated: false,
			planner_failed: false,
			plan_evidence_checkpoint: 0,
			recalled_refs: Vec::new(),
			evidence: crate::supervisor::gate::EvidenceLedger::default(),
			gate_task: None,
		}
	}
}

#[cfg(test)]
#[path = "core_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "core_methods_tests.rs"]
mod method_tests;
