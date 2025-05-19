// Chat commands module

// Chat commands
pub const HELP_COMMAND: &str = "/help";
pub const EXIT_COMMAND: &str = "/exit";
pub const QUIT_COMMAND: &str = "/quit";
pub const COPY_COMMAND: &str = "/copy";
pub const CLEAR_COMMAND: &str = "/clear";
pub const SAVE_COMMAND: &str = "/save";
pub const CACHE_COMMAND: &str = "/cache";

// List of all available commands for autocomplete
pub const COMMANDS: [&str; 7] = [
	HELP_COMMAND,
	EXIT_COMMAND,
	QUIT_COMMAND,
	COPY_COMMAND,
	CLEAR_COMMAND,
	SAVE_COMMAND,
	CACHE_COMMAND,
];