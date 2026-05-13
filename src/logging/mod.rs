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

//! Logging infrastructure for Octomind.
//!
//! This module provides:
//! - **ACP error sink** (`acp_error`): Dedicated error logging for ACP mode
//! - **Tracing setup** (`tracing_setup`): Mode-aware logging initialization
//!
//! ## Architecture
//!
//! ```text
//! ~/.local/share/octomind/logs/
//! ├── acp-debug.log       ← ACP tracing output
//! ├── acp-errors.jsonl    ← ACP-specific error sink
//! └── websocket-debug.log ← WebSocket tracing output
//! ```
//!
//! ## Usage
//!
//! At startup (ACP mode): call `init_tracing(LoggingMode::Acp, "debug")` and
//! `AcpErrorSink::initialize()`. In code, use the `log_debug!` / `log_error!`
//! macros — they route to `tracing::debug!` and `tracing::error!` (plus
//! AcpErrorSink for errors in ACP mode).

pub mod acp_error;
pub mod tracing_setup;

pub use acp_error::AcpErrorSink;
