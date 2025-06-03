// Copyright 2025 Muvon Un Limited
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
mod command_executor;
mod commands;
mod context_reduction;
mod context_truncation;

mod input;
mod layered_response;
pub mod markdown;
mod response;
pub mod session;
mod syntax;

// Re-export main structures and functions
pub use animation::show_loading_animation;
pub use command_executor::{
	command_exists, execute_command_layer, get_command_help, list_available_commands,
};
pub use commands::{
	CACHE_COMMAND, CLEAR_COMMAND, COMMANDS, COPY_COMMAND, DONE_COMMAND, EXIT_COMMAND, HELP_COMMAND,
	QUIT_COMMAND, RUN_COMMAND, SAVE_COMMAND,
};
pub use context_reduction::perform_context_reduction;
pub use context_truncation::check_and_truncate_context;

pub use input::read_user_input;
pub use layered_response::process_layered_response;
pub use markdown::{is_markdown_content, MarkdownRenderer, MarkdownTheme};
pub use response::{print_assistant_response, process_response};
pub use session::{format_duration, format_number, run_interactive_session, ChatSession};

// Model constants
pub const CLAUDE_MODEL: &str = "openrouter:anthropic/claude-sonnet-4";
pub const DEFAULT_MODEL: &str = CLAUDE_MODEL;
