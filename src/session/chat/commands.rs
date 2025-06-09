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
pub const SUMMARIZE_COMMAND: &str = "/summarize";
pub const MODEL_COMMAND: &str = "/model";
pub const RUN_COMMAND: &str = "/run";
pub const MCP_COMMAND: &str = "/mcp";
pub const REPORT_COMMAND: &str = "/report";

// List of all available commands for autocomplete
pub const COMMANDS: [&str; 21] = [
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
	SUMMARIZE_COMMAND,
	MODEL_COMMAND,
	RUN_COMMAND,
	MCP_COMMAND,
	REPORT_COMMAND,
];
