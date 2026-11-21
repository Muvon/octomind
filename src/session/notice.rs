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

//! Out-of-band notices: what the user sees of work done beside the transcript.

/// Whether [`supervisor_notice`] prints: standard error is an interactive
/// terminal and the output mode is one made for a terminal.
pub fn notices_are_shown() -> bool {
	let suppress = crate::config::with_thread_config(|c| c.output_mode())
		.map(|m| m.should_suppress_cli_output())
		.unwrap_or(false);
	!suppress && std::io::IsTerminal::is_terminal(&std::io::stderr())
}

/// Out-of-band notice (`· Supervisor: …`) so the user sees what the control
/// plane is doing — mirrors the skill-activation notice: dim, stderr,
/// interactive terminals only. Continuation lines (multi-line messages) are
/// indented under the first.
pub fn supervisor_notice(message: &str) {
	if !notices_are_shown() {
		return;
	}
	use colored::Colorize;
	for (i, line) in message.lines().enumerate() {
		if i == 0 {
			eprintln!("{} {} {}", "·".bright_black(), "Supervisor:".dimmed(), line.dimmed());
		} else {
			eprintln!("  {}", line.dimmed());
		}
	}
}
