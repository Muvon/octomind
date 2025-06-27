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

// Session continuation module - handles automatic session reset when token limits are reached

pub mod constants;
pub mod detection;
pub mod file_context;
pub mod injection;
pub mod processing;

// Re-export main public API
pub use detection::{is_continuation_in_progress, should_trigger_continuation, ContinuationParams};
pub use injection::inject_summary_request;
pub use processing::{check_and_handle_continuation, process_continuation_response};
