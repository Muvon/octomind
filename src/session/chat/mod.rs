// Chat session module
mod animation;
mod commands;
mod context_reduction;
mod context_truncation;
mod editorconfig_formatter;
mod input;
mod layered_response;
mod response;
mod session;

// Re-export main structures and functions
pub use session::{ChatSession, run_interactive_session};
pub use commands::{COMMANDS, HELP_COMMAND, EXIT_COMMAND, QUIT_COMMAND, COPY_COMMAND, CLEAR_COMMAND, SAVE_COMMAND, CACHE_COMMAND, DONE_COMMAND};
pub use input::read_user_input;
pub use response::process_response;
pub use layered_response::process_layered_response;
pub use animation::show_loading_animation;
pub use context_reduction::perform_context_reduction;
pub use context_truncation::check_and_truncate_context;
pub use editorconfig_formatter::apply_editorconfig_formatting;

// Model constants
pub const CLAUDE_MODEL: &str = "anthropic/claude-sonnet-4";
pub const DEFAULT_MODEL: &str = CLAUDE_MODEL;