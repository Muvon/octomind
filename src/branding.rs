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

//! Startup banner: compact pixel-art octobrain icon (block glyphs in a single
//! color, transparent background) next to a small info block.

use std::path::Path;

use colored::*;

// 4×7 octobrain mirroring the SVG asset. Eyes are 2-sub-pixel-wide cyan bars
// that span the right half of one cell + the left half of the next — rendered
// as `▝▘` pairs on a purple background so the cyan top-quadrants merge into a
// continuous horizontal bar. Row 2 fuses the head's bottom curves with the
// leg tops (each ▀ caps a gap, each █ is the start of a leg). Row 3's ▛
// glyphs combine leg-shaft bottoms with left-aligned tips into one cell.
type Rgb = (u8, u8, u8);
type Segment = (Rgb, &'static str);
type Row = (Option<Rgb>, &'static [Segment]);

const BODY: Rgb = (0xA8, 0x55, 0xF7);
const EYE: Rgb = (0x22, 0xD3, 0xEE);

const ICON_ROWS: [Row; 4] = [
	(None, &[(BODY, "▟█████▙")]),
	(
		Some(BODY),
		&[
			(BODY, "█"),
			(EYE, "▝▘"),
			(BODY, "█"),
			(EYE, "▝▘"),
			(BODY, "█"),
		],
	),
	(None, &[(BODY, "▟▀█▀█▀▙")]),
	(None, &[(BODY, "▛ ▛ ▛ ▛")]),
];
const ICON_WIDTH: usize = 7;

fn icon_lines() -> Vec<String> {
	ICON_ROWS
		.iter()
		.map(|(bg, segments)| {
			let mut s = String::new();
			match bg {
				Some((r, g, b)) => s.push_str(&format!("\x1b[48;2;{r};{g};{b}m")),
				None => s.push_str("\x1b[49m"),
			}
			for &((r, g, b), text) in *segments {
				s.push_str(&format!("\x1b[38;2;{r};{g};{b}m{text}"));
			}
			s.push_str("\x1b[0m");
			s
		})
		.collect()
}

/// Print the startup banner: pixel icon on the left, info block on the right.
/// `extra` lines (already styled) are appended below the standard role/model/cwd
/// trio so callers can attach the tip + shortcuts hint inline with the icon
/// instead of dumping them on separate rows below.
pub fn print_startup_banner(role: &str, model: &str, cwd: &Path, extra: &[String]) {
	let icon = icon_lines();
	let pad = "  ";

	let mut info: Vec<String> = vec![
		format!(
			"{} {}",
			"Octomind".bright_white().bold(),
			format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
		),
		format!(
			"{} {} {} {} {}",
			"Role:".dimmed(),
			role.bright_cyan(),
			"·".dimmed(),
			"Model:".dimmed(),
			model.bright_magenta()
		),
		format!("{}", cwd.display().to_string().dimmed()),
	];
	info.extend(extra.iter().cloned());

	let total = icon.len().max(info.len());
	let blank_icon = " ".repeat(ICON_WIDTH);
	for i in 0..total {
		let icon_line = icon.get(i).map(String::as_str).unwrap_or(&blank_icon);
		let info_line = info.get(i).map(String::as_str).unwrap_or("");
		println!("{icon_line}{pad}{info_line}");
	}
}
