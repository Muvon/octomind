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

// Session module implementation
pub mod commands;
mod core;
mod display;
mod messages;
mod runner;
pub mod utils;

pub use core::ChatSession;
pub use runner::{
	execute_api_call_and_process_response, format_provider_error, prepare_for_api_call,
	process_layers_if_enabled, run_interactive_session, run_interactive_session_with_input,
	setup_and_initialize_session, setup_system_prompt_and_cache,
};
pub use utils::{format_number, get_initial_messages};
