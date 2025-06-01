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

// Context reduction functionality
// Stub implementation since indexer has been removed

use anyhow::Result;
use std::sync::{Arc, atomic::AtomicBool};
use super::session::ChatSession;
use crate::config::Config;

/// Perform context reduction for the chat session
/// This is a stub implementation since the indexer has been removed
pub async fn perform_context_reduction(
	_chat_session: &mut ChatSession,
	_config: &Config,
	_cancelled: Arc<AtomicBool>
) -> Result<()> {
	// This functionality has been removed along with the indexer
	// Users should rely on external MCP servers for advanced context management
	println!("Context reduction is no longer available in this version.");
	println!("Consider using an external MCP server like 'octocode' for advanced codebase analysis.");
	Ok(())
}
