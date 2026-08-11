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

// The plan tool provides structured, step-by-step task execution, progress tracking, and session integration.
// Registered here as a core MCP function. See src/mcp/core/plan/ for details and compliance patterns.
// Function definitions for the Core MCP provider

use super::super::McpFunction;
use super::plan::get_plan_function;
use super::recall::get_recall_function;

// Core builtin tools — the universal self-management primitives `plan` and
// `recall`. Delegation (`tap`) and session control (`schedule`) are the
// `orchestration` builtin server; tool-surface controls (`mcp`, `agent`,
// `skill`, `capability`) are the `runtime` builtin server.
pub fn get_all_functions(config: &crate::config::Config) -> Vec<McpFunction> {
	let mut functions = vec![get_plan_function()];
	// `recall` dereferences the PACT block registry; the sidecar index is only
	// written when the attention/governance machinery runs, so don't advertise
	// a tool that could only error.
	if config.compression.attention.enabled || config.compression.attention.governance.enabled {
		functions.push(get_recall_function());
	}
	functions
}
