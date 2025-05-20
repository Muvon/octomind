// Chat commands module

// Chat commands
pub const HELP_COMMAND: &str = "/help";
pub const EXIT_COMMAND: &str = "/exit";
pub const QUIT_COMMAND: &str = "/quit";
pub const COPY_COMMAND: &str = "/copy";
pub const CLEAR_COMMAND: &str = "/clear";
pub const SAVE_COMMAND: &str = "/save";
pub const CACHE_COMMAND: &str = "/cache";
pub const LIST_COMMAND: &str = "/list";
pub const SESSION_COMMAND: &str = "/session";
pub const LAYERS_COMMAND: &str = "/layers";
pub const INFO_COMMAND: &str = "/info";
pub const DONE_COMMAND: &str = "/done";
pub const DEBUG_COMMAND: &str = "/debug";

// List of all available commands for autocomplete
pub const COMMANDS: [&str; 13] = [
	HELP_COMMAND,
	EXIT_COMMAND,
	QUIT_COMMAND,
	COPY_COMMAND,
	CLEAR_COMMAND,
	SAVE_COMMAND,
	CACHE_COMMAND,
	LIST_COMMAND,
	SESSION_COMMAND,
	LAYERS_COMMAND,
	INFO_COMMAND,
	DONE_COMMAND,
	DEBUG_COMMAND,
];