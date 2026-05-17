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

// Session module implementation
pub mod commands;
mod core;
mod display;
mod messages;
pub mod utils;

// New modular components (extracted from runner.rs)
mod api_executor;
mod api_prep;
mod error_utils;
mod layer_processor;
mod main_loop;
mod params;
mod prompt_setup;
mod setup;

pub use api_executor::execute_api_call_and_process_response;
pub use api_prep::prepare_for_api_call;
pub use core::ChatSession;
pub use error_utils::{format_provider_error, handle_api_error, handle_followup_api_error};
pub use layer_processor::process_layers_if_enabled;
pub use main_loop::{run_interactive_session, run_interactive_session_with_input};
pub use params::GenericSessionArgs;
pub use prompt_setup::setup_system_prompt_and_cache;
pub use setup::setup_and_initialize_session;
pub use utils::{format_number, get_initial_messages};
