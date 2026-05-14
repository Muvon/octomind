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

//! Shared "session status" body used by the interactive prompt and the
//! working-spinner message. Body format (no leading marker):
//!
//!   `$0.01 ▰▰▱▱▱ 13.8%`
//!
//! The prompt prepends `▍` as its line-leader marker; the spinner uses the
//! tick character (`⠋⠙⠹⠸…`) as its leader instead, so the two share the
//! body but each owns its own left-edge identity.
//!
//! - `▰`/`▱` cells (5 total, ~20% each, `ceil(pct/20)` filled) make context
//!   usage scan-able without reading the number.
//! - `· ∞` is shown in place of the bar when there is no max threshold.
//!
//! The chevron `〉` is the prompt's input indicator and is NOT part of this
//! body — animation messages append their own label ("Working …",
//! "Validating …", etc.) after it.

use colored::Colorize;

/// Render the 5-cell context-usage bar. Filled cells bright blue, empty cells
/// dim. Glyphs: `▰` (filled) / `▱` (empty).
fn build_context_bar(pct: f64) -> String {
	const CELLS: usize = 5;
	let filled = ((pct / 100.0) * CELLS as f64).ceil() as usize;
	let filled = filled.min(CELLS);
	let mut out = String::with_capacity(32);
	out.push_str("\x1b[94m");
	for _ in 0..filled {
		out.push('▰');
	}
	out.push_str("\x1b[90m");
	for _ in 0..(CELLS - filled) {
		out.push('▱');
	}
	out.push_str("\x1b[39m");
	out
}

/// Build the persistent "session status" line printed above each prompt:
///
///   `▍ $0.48 (+$0.013) ▰▰▰▱▱ 54.2%`
///
/// The `(+$delta)` part shows the cost increase since the previous prompt
/// (only when positive and non-trivial). When there's no cost and no max
/// threshold to show, returns an empty string — caller should skip printing.
pub fn build_status_line(
	cost: f64,
	context_tokens: u64,
	max_threshold: u64,
	delta: Option<f64>,
) -> String {
	let pct = if max_threshold > 0 {
		Some((context_tokens as f64 / max_threshold as f64 * 100.0).min(100.0))
	} else {
		None
	};
	let has_cost = cost > 0.0;
	if !has_cost && pct.is_none() {
		return String::new();
	}

	let marker = "▍".bright_blue();
	let mut parts: Vec<String> = vec![marker.to_string()];

	if has_cost {
		parts.push(format!("${:.2}", cost).bright_blue().to_string());
		if let Some(d) = delta {
			if d > 0.0001 {
				parts.push(format!("(+${:.3})", d).bright_black().to_string());
			}
		}
	}

	match pct {
		Some(pct) => {
			parts.push(build_context_bar(pct));
			parts.push(format!("{:.1}%", pct).bright_blue().to_string());
		}
		None if has_cost => {
			parts.push("·".bright_black().to_string());
			parts.push("∞".bright_blue().to_string());
		}
		None => {}
	}

	parts.join(" ")
}

/// Same status body but with no embedded ANSI codes. Used by the spinner
/// so indicatif's `{msg:.cyan}` template directive paints the whole line
/// uniformly cyan (the original spinner color). Filled vs empty bar cells
/// rely on glyph contrast (`▰` vs `▱`) alone, since any inline ANSI here
/// would locally override the template's cyan.
pub fn build_status_body_plain(cost: f64, context_tokens: u64, max_threshold: u64) -> String {
	let pct = if max_threshold > 0 {
		Some((context_tokens as f64 / max_threshold as f64 * 100.0).min(100.0))
	} else {
		None
	};

	let plain_bar = |pct: f64| -> String {
		const CELLS: usize = 5;
		let filled = (((pct / 100.0) * CELLS as f64).ceil() as usize).min(CELLS);
		let mut out = String::with_capacity(CELLS * 3);
		for _ in 0..filled {
			out.push('▰');
		}
		for _ in 0..(CELLS - filled) {
			out.push('▱');
		}
		out
	};

	match (cost > 0.0, pct) {
		(true, Some(pct)) => format!("${:.2} {} {:.1}%", cost, plain_bar(pct), pct),
		(true, None) => format!("${:.2} · ∞", cost),
		(false, Some(pct)) => format!("{} {:.1}%", plain_bar(pct), pct),
		(false, None) => String::new(),
	}
}
