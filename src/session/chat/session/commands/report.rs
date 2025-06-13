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

// Report command handler

use super::super::core::ChatSession;
use crate::config::Config;
use anyhow::Result;
use colored::Colorize;

pub fn handle_report(session: &ChatSession, config: &Config) -> Result<bool> {
	// Generate and display session usage report
	if let Some(ref session_file) = session.session.session_file {
		let session_file_str = session_file.to_string_lossy();
		match crate::session::report::SessionReport::generate_from_log(&session_file_str) {
			Ok(report) => {
				report.display(config);
			}
			Err(e) => {
				println!("{}: Failed to generate report: {}", "Error".bright_red(), e);
				println!(
					"{}: Make sure the session log file exists and is readable.",
					"Hint".bright_yellow()
				);
			}
		}
	} else {
		println!(
			"{}: No session file available for report generation.",
			"Error".bright_red()
		);
		println!(
			"{}: Save the session first with /save command.",
			"Hint".bright_yellow()
		);
	}
	Ok(false)
}
