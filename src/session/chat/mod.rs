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

// Chat session module
mod animation;
pub mod animation_manager;
pub mod assistant_output;
mod command_executor;
mod commands;
pub(crate) mod conversation_compression;
mod cost_tracker;
mod edit_mode;
pub(crate) mod file_context;
pub mod formatting;
mod input;
mod layered_response;
pub mod markdown;
mod message_handler;
mod prompt;
mod reedline_adapter;
pub(crate) mod response;
pub mod session;
mod status_prefix;
mod syntax;
mod thinking_display;
mod tool_display;
mod tool_error_tracker;
mod tool_processor;
// Re-export main structures and functions
pub use animation::format_elapsed_time;
pub use animation_manager::{get_animation_manager, AnimationManager};
pub use assistant_output::print_assistant_response;
pub use command_executor::{
	command_exists, execute_command_layer, get_command_help, list_available_commands,
};
pub use commands::{
	CLEAR_COMMAND, COMMANDS, COPY_COMMAND, DONE_COMMAND, EXIT_COMMAND, HELP_COMMAND, QUIT_COMMAND,
	RUN_COMMAND,
};
pub use cost_tracker::CostTracker;
pub use edit_mode::EmacsWithShortcutHelp;
pub use formatting::format_duration;
pub use input::{read_user_input, InputResult};
pub use layered_response::process_layered_response;
pub use markdown::{is_markdown_content, MarkdownRenderer, MarkdownTheme};
pub use message_handler::MessageHandler;
pub use prompt::ChatPrompt;
pub use response::process_response;
pub use session::{
	format_number, run_interactive_session, run_interactive_session_with_input, ChatSession,
};
pub use thinking_display::display_thinking;
pub use tool_display::{
	display_individual_tool_header_with_context, display_individual_tool_header_with_params,
	display_tool_output_smart, display_tool_parameters_full,
};
pub use tool_processor::ToolProcessor;

// Model constants
pub const CLAUDE_MODEL: &str = "openrouter:anthropic/claude-sonnet-4";
pub const DEFAULT_MODEL: &str = CLAUDE_MODEL;
