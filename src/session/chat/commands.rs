// Chat commands module

// Chat commands
pub const HELP_COMMAND: &str = "/help";
pub const HELP_COMMAND_ALT: &str = "/?";
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
pub const LOGLEVEL_COMMAND: &str = "/loglevel";
pub const TRUNCATE_COMMAND: &str = "/truncate";
pub const MODEL_COMMAND: &str = "/model";
pub const RUN_COMMAND: &str = "/run";

// List of all available commands for autocomplete
pub const COMMANDS: [&str; 18] = [
	HELP_COMMAND,
	HELP_COMMAND_ALT,
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
	LOGLEVEL_COMMAND,
	TRUNCATE_COMMAND,
	MODEL_COMMAND,
	RUN_COMMAND,
];
