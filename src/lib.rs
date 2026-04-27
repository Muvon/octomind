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

#![recursion_limit = "1024"]

// ============================================================================
// SPINNER-AWARE PRINT MACROS
// ============================================================================
// These macros shadow std::println!, std::print!, std::eprintln!, std::eprint!
// at the crate root level. They automatically suspend the animation spinner
// before printing to prevent output interference.
//
// IMPORTANT: These must be defined BEFORE any module declarations to ensure
// they shadow std macros in textual scope throughout the entire crate.
//
// NOTE: We use `format!` outside the closure to evaluate arguments in the
// caller's context, allowing `?` operator to work correctly.

/// Spinner-aware println! - suspends animation before printing
#[macro_export]
macro_rules! println {
	() => {
		$crate::utils::terminal_output::with_suspended_spinner(|| {
			std::println!()
		})
	};
	($($arg:tt)*) => {{
		let s = ::std::format!($($arg)*);
		$crate::utils::terminal_output::with_suspended_spinner(|| {
			std::println!("{}", s)
		})
	}};
}

/// Spinner-aware print! - suspends animation before printing
#[macro_export]
macro_rules! print {
	($($arg:tt)*) => {{
		let s = ::std::format!($($arg)*);
		$crate::utils::terminal_output::with_suspended_spinner(|| {
			std::print!("{}", s)
		})
	}};
}

/// Spinner-aware eprintln! - suspends animation before printing
#[macro_export]
macro_rules! eprintln {
	() => {
		$crate::utils::terminal_output::with_suspended_spinner(|| {
			std::eprintln!()
		})
	};
	($($arg:tt)*) => {{
		let s = ::std::format!($($arg)*);
		$crate::utils::terminal_output::with_suspended_spinner(|| {
			std::eprintln!("{}", s)
		})
	}};
}

/// Spinner-aware eprint! - suspends animation before printing
#[macro_export]
macro_rules! eprint {
	($($arg:tt)*) => {{
		let s = ::std::format!($($arg)*);
		$crate::utils::terminal_output::with_suspended_spinner(|| {
			std::eprint!("{}", s)
		})
	}};
}

// ============================================================================
// MODULE DECLARATIONS
// ============================================================================

// Main lib.rs file that exports our modules
pub mod acp;
pub mod agent;
pub mod config;
pub mod directories;
pub mod learning;
pub mod logging;
pub mod mcp;
pub mod proctitle;
pub mod providers;
pub mod sandbox;
pub mod session;
pub mod state;
pub mod utils;
pub mod websocket;

// Re-export commonly used items for convenience
pub use config::Config;

// Re-export logging types
pub use logging::AcpErrorSink;

// Re-export workflow types
pub use session::workflows::{PatternParser, StepExecutor, WorkflowOrchestrator};
