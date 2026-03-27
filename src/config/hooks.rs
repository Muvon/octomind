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

use serde::{Deserialize, Serialize};

/// Webhook hook configuration.
///
/// Each hook binds an HTTP server on the configured address. Incoming POST
/// requests are piped through the script: body → stdin, stdout → session inbox.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HookConfig {
	/// Unique hook identifier (referenced by `--hook` CLI flag)
	pub name: String,
	/// Address:port to bind the HTTP listener (e.g. "0.0.0.0:9876")
	pub bind: String,
	/// Path to the shebang-executable script that processes webhook payloads.
	/// stdin: raw HTTP body, stdout: message to inject, exit 0 = success.
	pub script: String,
	/// Timeout in seconds for script execution (default: 30)
	#[serde(default = "default_hook_timeout")]
	pub timeout: u64,
}

fn default_hook_timeout() -> u64 {
	30
}
