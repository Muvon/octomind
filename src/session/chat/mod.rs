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
mod commands;
mod command_executor;
mod context_reduction;
mod context_truncation;
mod editorconfig_formatter;
mod input;
mod layered_response;
pub mod markdown;
mod response;
mod session;
mod syntax;

// Re-export main structures and functions
pub use session::{ChatSession, run_interactive_session};
pub use commands::{COMMANDS, HELP_COMMAND, EXIT_COMMAND, QUIT_COMMAND, COPY_COMMAND, CLEAR_COMMAND, SAVE_COMMAND, CACHE_COMMAND, DONE_COMMAND, RUN_COMMAND};
pub use command_executor::{execute_command_layer, list_available_commands, command_exists, get_command_help};
pub use input::read_user_input;
pub use response::{process_response, print_assistant_response};
pub use layered_response::process_layered_response;
pub use animation::show_loading_animation;
pub use context_reduction::perform_context_reduction;
pub use context_truncation::check_and_truncate_context;
pub use editorconfig_formatter::apply_editorconfig_formatting;
pub use markdown::{MarkdownRenderer, is_markdown_content, MarkdownTheme};

// Model constants
pub const CLAUDE_MODEL: &str = "openrouter:anthropic/claude-sonnet-4";
pub const DEFAULT_MODEL: &str = CLAUDE_MODEL;
