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

// Pre-model pipe execution — runs a matching `[[pipe]]` from guardrails
// on the raw user input before the main model sees it.
//
// Replaces the old deterministic pipeline system. Pipes are defined
// per-project in `.agents/guardrails.toml`.

use crate::log_info;
use anyhow::Result;

/// Run a matching `[[pipe]]` on `input` and return the transformed input.
/// Returns the original input when no pipe matched. Errors are hard stops
/// (non-zero exit, timeout, etc.).
pub async fn run_pipe_if_enabled(
	input: &str,
	role: &str,
	first_message_processed: bool,
) -> Result<String> {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return Ok(input.to_string());
	};

	match crate::session::pipe::run_pipe(
		&session_id,
		role,
		input,
		first_message_processed,
	)
	.await
	{
		Ok(Some(transformed)) => {
			log_info!("Pipe transformed input ({} → {} bytes)", input.len(), transformed.len());
			Ok(transformed)
		}
		Ok(None) => Ok(input.to_string()),
		Err(e) => Err(e),
	}
}
