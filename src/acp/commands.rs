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

//! ACP command handling - exposes session commands via ACP extension methods.

use std::cell::RefCell;
use std::rc::Rc;

use agent_client_protocol::schema::{ExtRequest, ExtResponse};
use agent_client_protocol::Error;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::config::Config;
use crate::session::cancellation::SessionCancellation;
use crate::session::chat::session::commands::{process_command, CommandResult};
use crate::session::chat::session::ChatSession;

/// Command namespace for ACP extension methods.
/// Note: the ACP library strips the leading '_' from the method name before routing to
/// ext_method(), so we match against the name without the underscore prefix.
pub const COMMAND_NAMESPACE: &str = "octomind/command";

/// Request payload for command execution
#[derive(Debug, Deserialize)]
pub struct CommandRequest {
	/// The session ID to execute the command in
	pub session_id: String,
	/// The command to execute (e.g., "/help", "/info")
	pub command: String,
	/// Optional arguments for the command
	#[serde(default)]
	pub args: Vec<String>,
}

/// Response payload for command execution
#[derive(Debug, Serialize)]
pub struct CommandResponse {
	/// Whether the command was executed successfully
	pub success: bool,
	/// The command output (structured JSON)
	pub output: Option<serde_json::Value>,
	/// Error message if success is false
	pub error: Option<String>,
}
/// Execute an ACP extension command
pub async fn execute_command(
	request: &CommandRequest,
	sessions: &Rc<RefCell<std::collections::HashMap<String, (ChatSession, std::path::PathBuf)>>>,
	config: &RefCell<Config>,
	role: &str,
	cancellations: &Rc<RefCell<std::collections::HashMap<String, SessionCancellation>>>,
) -> CommandResponse {
	// Take session out of map for exclusive access
	let (mut chat_session, session_cwd) = match sessions.borrow_mut().remove(&request.session_id) {
		Some(s) => s,
		None => {
			return CommandResponse {
				success: false,
				output: None,
				error: Some(format!("session not found: {}", request.session_id)),
			};
		}
	};

	// Restore working directory for this session
	crate::mcp::set_session_working_directory(session_cwd.clone());

	// Get cancellation handle
	let operation_rx = {
		let mut cancellations = cancellations.borrow_mut();
		if let Some(c) = cancellations.get_mut(&request.session_id) {
			c.new_operation()
		} else {
			let c = SessionCancellation::new();
			cancellations.insert(request.session_id.clone(), c);
			cancellations
				.get_mut(&request.session_id)
				.unwrap()
				.new_operation()
		}
	};

	// Build the full command string
	let full_command = if request.args.is_empty() {
		request.command.clone()
	} else {
		format!("{} {}", request.command, request.args.join(" "))
	};

	// Clone config before the await point to avoid holding RefCell borrow across await
	let mut config_clone = config.borrow().clone();

	// Execute the command
	let result = process_command(
		&mut chat_session,
		&full_command,
		&mut config_clone,
		role,
		operation_rx,
	)
	.await;

	// Put session back
	sessions
		.borrow_mut()
		.insert(request.session_id.clone(), (chat_session, session_cwd));

	match result {
		Ok(CommandResult::Handled) => CommandResponse {
			success: true,
			output: None,
			error: None,
		},
		Ok(CommandResult::HandledWithOutput(output)) => CommandResponse {
			success: true,
			output: Some(output.to_json()),
			error: None,
		},
		Ok(CommandResult::Exit) => CommandResponse {
			success: true,
			output: Some(serde_json::json!({ "action": "exit" })),
			error: None,
		},
		Ok(CommandResult::TreatAsUserInput) => CommandResponse {
			success: false,
			output: None,
			error: Some(format!("Unknown command: {}", request.command)),
		},
		Err(e) => CommandResponse {
			success: false,
			output: None,
			error: Some(e.to_string()),
		},
	}
}

/// Handle ACP ext_method requests for commands
pub async fn handle_ext_method(
	request: ExtRequest,
	sessions: &Rc<RefCell<std::collections::HashMap<String, (ChatSession, std::path::PathBuf)>>>,
	config: &RefCell<Config>,
	role: &str,
	cancellations: &Rc<RefCell<std::collections::HashMap<String, SessionCancellation>>>,
) -> Result<ExtResponse, Error> {
	// Only handle commands in our namespace
	if !request.method.starts_with(COMMAND_NAMESPACE) {
		return Err(Error::method_not_found());
	}

	// Parse the request
	let command_request: CommandRequest = match serde_json::from_str(request.params.get()) {
		Ok(req) => req,
		Err(e) => {
			let response = CommandResponse {
				success: false,
				output: None,
				error: Some(format!("Invalid request: {}", e)),
			};
			let raw = RawValue::from_string(serde_json::to_string(&response).unwrap()).unwrap();
			return Ok(ExtResponse::new(std::sync::Arc::from(raw)));
		}
	};

	// Execute the command
	let response = execute_command(&command_request, sessions, config, role, cancellations).await;

	// Convert to ExtResponse
	let raw = RawValue::from_string(serde_json::to_string(&response).unwrap()).unwrap();
	Ok(ExtResponse::new(std::sync::Arc::from(raw)))
}
