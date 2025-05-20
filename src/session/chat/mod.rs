// Chat session module
mod session;
pub mod commands;
mod input;
mod response;
mod animation;
mod layered_response;

// Re-export main structures and functions
pub use session::ChatSession;
pub use commands::{COMMANDS, HELP_COMMAND, EXIT_COMMAND, QUIT_COMMAND, COPY_COMMAND, CLEAR_COMMAND, SAVE_COMMAND, CACHE_COMMAND};
pub use input::read_user_input;
pub use response::process_response;
pub use layered_response::process_layered_response;
pub use animation::show_loading_animation;

// Re-export the main run_interactive_session function
pub use session::run_interactive_session;

// Model constants
pub const CLAUDE_MODEL: &str = "anthropic/claude-3.7-sonnet";
pub const DEFAULT_MODEL: &str = CLAUDE_MODEL;