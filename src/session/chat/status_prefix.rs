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

/// Build the status body (no leading marker). Returns an empty string when
/// neither cost nor a max threshold is set (callers should not add a
/// separator in that case).
///
/// Layout matrix:
/// - cost > 0,  threshold > 0:  `$0.01 ▰▰▱▱▱ 13.8%`
/// - cost > 0,  threshold == 0: `$0.01 · ∞`
/// - cost == 0, threshold > 0:  `▰▰▱▱▱ 13.8%`
/// - cost == 0, threshold == 0: `` (empty)
pub fn build_status_body(cost: f64, context_tokens: u64, max_threshold: u64) -> String {
	let pct = if max_threshold > 0 {
		Some((context_tokens as f64 / max_threshold as f64 * 100.0).min(100.0))
	} else {
		None
	};

	match (cost > 0.0, pct) {
		(true, Some(pct)) => format!(
			"{} {} {}",
			format!("${:.2}", cost).bright_blue(),
			build_context_bar(pct),
			format!("{:.1}%", pct).bright_blue(),
		),
		(true, None) => format!(
			"{} {} {}",
			format!("${:.2}", cost).bright_blue(),
			"·".bright_black(),
			"∞".bright_blue(),
		),
		(false, Some(pct)) => format!(
			"{} {}",
			build_context_bar(pct),
			format!("{:.1}%", pct).bright_blue(),
		),
		(false, None) => String::new(),
	}
}

/// Build the prompt prefix: `▍`-marker + status body. Returns just `▍ ` if
/// no cost/threshold data is available so the prompt always has its
/// identifying marker.
pub fn build_prompt_prefix(cost: f64, context_tokens: u64, max_threshold: u64) -> String {
	let marker = "▍".bright_blue();
	let body = build_status_body(cost, context_tokens, max_threshold);
	if body.is_empty() {
		String::new()
	} else {
		format!("{} {}", marker, body)
	}
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
