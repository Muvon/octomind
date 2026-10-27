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

// Chat commands module

// Chat commands
pub const HELP_COMMAND: &str = "/help";
pub const HELP_COMMAND_ALT: &str = "/?";
pub const EXIT_COMMAND: &str = "/exit";
pub const QUIT_COMMAND: &str = "/quit";
pub const COPY_COMMAND: &str = "/copy";
pub const REPLY_COMMAND: &str = "/reply";
pub const CLEAR_COMMAND: &str = "/clear";
pub const LIST_COMMAND: &str = "/list";
pub const NEW_COMMAND: &str = "/new";
pub const INFO_COMMAND: &str = "/info";
pub const DONE_COMMAND: &str = "/done";
pub const LOGLEVEL_COMMAND: &str = "/loglevel";
pub const MODEL_COMMAND: &str = "/model";
pub const RUN_COMMAND: &str = "/run";
pub const MCP_COMMAND: &str = "/mcp";
pub const REPORT_COMMAND: &str = "/report";
pub const IMAGE_COMMAND: &str = "/image";
pub const VIDEO_COMMAND: &str = "/video";
pub const CONTEXT_COMMAND: &str = "/context";
pub const ROLE_COMMAND: &str = "/role";
pub const PROMPT_COMMAND: &str = "/prompt";
pub const PLAN_COMMAND: &str = "/plan";
pub const SKILL_COMMAND: &str = "/skill";
pub const EFFORT_COMMAND: &str = "/effort";
pub const SCHEDULE_COMMAND: &str = "/schedule";
pub const STATUS_COMMAND: &str = "/status";
pub const LEARNING_COMMAND: &str = "/learning";
pub const SHARE_COMMAND: &str = "/share";
pub const ANALYZE_COMMAND: &str = "/analyze";
pub const USAGE_COMMAND: &str = "/usage";
pub const LOGIN_COMMAND: &str = "/login";
pub const RENAME_COMMAND: &str = "/rename";
pub const WORKFLOW_COMMAND: &str = "/workflow";
/// Where a built-in command can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
	/// The CLI and WebSocket sessions alike.
	Anywhere,
	/// Acts on the host's terminal, clipboard or CLI prompt, or ends the process
	/// from inside: a WebSocket client has `close` and message attachments for those.
	CliOnly,
}

/// Every built-in command in `/help` order: (name, argument hint, description, reach).
/// The hint is empty only when the command takes no argument. `/help`, the
/// completer and the WebSocket bind ack all list from here (the ACP menu in
/// `acp/agent.rs` keeps its own list).
pub const BUILTIN_COMMANDS: [(&str, &str, &str, Reach); 31] = [
	(HELP_COMMAND, "", "Show this help message", Reach::Anywhere),
	(
		COPY_COMMAND,
		"[last|assistant|user|all]",
		"Copy the last response, or a scope",
		Reach::CliOnly,
	),
	(
		REPLY_COMMAND,
		"[N|words]",
		"Quote a reply into the prompt, Tab picks",
		Reach::CliOnly,
	),
	(CLEAR_COMMAND, "", "Clear the screen", Reach::CliOnly),
	(LIST_COMMAND, "[page]", "List all available sessions", Reach::Anywhere),
	(NEW_COMMAND, "[title]", "Start a fresh session", Reach::CliOnly),
	(
		RENAME_COMMAND,
		"[title]",
		"Set a display title for this session (no title clears)",
		Reach::Anywhere,
	),
	(INFO_COMMAND, "", "Detailed token and cost breakdown", Reach::Anywhere),
	(
		DONE_COMMAND,
		"[instructions]",
		"Finalize task with memorize/summarize/commit",
		Reach::Anywhere,
	),
	(
		LOGLEVEL_COMMAND,
		"[none|info|debug]",
		"View or set the logging level",
		Reach::Anywhere,
	),
	(
		RUN_COMMAND,
		"[name] [input]",
		"Execute a command layer",
		Reach::Anywhere,
	),
	(
		CONTEXT_COMMAND,
		"[all|assistant|user|tool|large]",
		"Display session context",
		Reach::Anywhere,
	),
	(
		MODEL_COMMAND,
		"[provider:model]",
		"View or change current AI model",
		Reach::Anywhere,
	),
	(
		EFFORT_COMMAND,
		"[level]",
		"View or change reasoning effort",
		Reach::Anywhere,
	),
	(ROLE_COMMAND, "[name]", "View or change current role", Reach::Anywhere),
	(
		MCP_COMMAND,
		"[list|info|full|health|dump|validate]",
		"MCP server management",
		Reach::Anywhere,
	),
	(IMAGE_COMMAND, "<path>", "Attach image to next message", Reach::CliOnly),
	(VIDEO_COMMAND, "<path>", "Attach video to next message", Reach::CliOnly),
	(PROMPT_COMMAND, "[name]", "Manage prompt templates", Reach::Anywhere),
	(PLAN_COMMAND, "", "Display current plan", Reach::Anywhere),
	(
		SKILL_COMMAND,
		"[name]",
		"List skills or toggle by name",
		Reach::Anywhere,
	),
	(
		SCHEDULE_COMMAND,
		"[list|add|edit|remove]",
		"Schedule a message to be injected later",
		Reach::Anywhere,
	),
	(
		WORKFLOW_COMMAND,
		"[name] [input]",
		"List tap workflows or run one",
		Reach::Anywhere,
	),
	(
		STATUS_COMMAND,
		"[agents|monitors|jobs]",
		"Show active agents, MCP jobs, and command monitors",
		Reach::Anywhere,
	),
	(
		LEARNING_COMMAND,
		"[list|show|delete|clear|evolution]",
		"Manage role/project lessons",
		Reach::Anywhere,
	),
	(
		REPORT_COMMAND,
		"[day|week|month] [here]",
		"Usage report with human time and energy",
		Reach::Anywhere,
	),
	(
		SHARE_COMMAND,
		"",
		"Upload session and print shareable URL",
		Reach::Anywhere,
	),
	// The bridge listens on the host's loopback, out of a remote client's reach.
	(
		ANALYZE_COMMAND,
		"",
		"Open this session in the web viewer (local-only)",
		Reach::CliOnly,
	),
	(
		USAGE_COMMAND,
		"",
		"Show account usage, quotas, and balance",
		Reach::Anywhere,
	),
	(LOGIN_COMMAND, "", "Sign in to an Octomind account", Reach::Anywhere),
	(EXIT_COMMAND, "", "Exit the session", Reach::CliOnly),
];

/// The second name a command answers to, which `/help` shows beside it.
pub fn alias_of(command: &str) -> Option<&'static str> {
	match command {
		HELP_COMMAND => Some(HELP_COMMAND_ALT),
		EXIT_COMMAND => Some(QUIT_COMMAND),
		_ => None,
	}
}

/// Every name a built-in answers to, aliases included: what the completer offers
/// and command routing accepts.
pub static COMMANDS: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
	BUILTIN_COMMANDS
		.iter()
		.flat_map(|&(name, ..)| std::iter::once(name).chain(alias_of(name)))
		.collect()
});
