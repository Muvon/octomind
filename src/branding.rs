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

//! Startup banner: pixel-art icon (parsed from the embedded SVG) rendered with
//! transparent ANSI half-block characters next to a small info block.

use std::path::Path;
use std::sync::OnceLock;

use colored::*;
use regex::Regex;

const ICON_SVG: &str = include_str!("../assets/octomind-icon.svg");

// SVG is 480x480 with 30px design pixels. Sample each 16x16 cell's center
// against rect bounding boxes — no intermediate grid, no color averaging.
const ICON_SIZE: usize = 16;
const CELL: i32 = 30;
const HALF: i32 = CELL / 2;

type Rgb = (u8, u8, u8);
type Cell = Option<Rgb>;

static ICON: OnceLock<Box<[[Cell; ICON_SIZE]; ICON_SIZE]>> = OnceLock::new();

fn parse_hex(s: &str) -> Rgb {
	let n = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
	(
		((n >> 16) & 0xFF) as u8,
		((n >> 8) & 0xFF) as u8,
		(n & 0xFF) as u8,
	)
}

fn rasterize() -> Box<[[Cell; ICON_SIZE]; ICON_SIZE]> {
	// Empty cells stay None → rendered as transparent (terminal bg shows through).
	let mut grid: Box<[[Cell; ICON_SIZE]; ICON_SIZE]> = Box::new([[None; ICON_SIZE]; ICON_SIZE]);

	// Path shape: M{x1} {y1}H{x2}V{y2}H{x1}V{y1}Z
	let re = Regex::new(r#"M(\d+)\s+(\d+)H(\d+)V(\d+)H\d+V\d+Z"\s+fill="(#[0-9A-Fa-f]{6})""#)
		.expect("static regex");

	for cap in re.captures_iter(ICON_SVG) {
		let x1: i32 = cap[1].parse().unwrap_or(0);
		let y1: i32 = cap[2].parse().unwrap_or(0);
		let x2: i32 = cap[3].parse().unwrap_or(0);
		let y2: i32 = cap[4].parse().unwrap_or(0);
		let color = parse_hex(&cap[5]);

		let (xa, xb) = (x1.min(x2), x1.max(x2));
		let (ya, yb) = (y1.min(y2), y1.max(y2));

		// Paint cells whose centers fall inside [xa, xb) × [ya, yb).
		// Half-open interval mirrors how the SVG's overlapping rects compose
		// (each 30px feature → exactly one 16-grid cell, no edge bleed).
		for row in 0..ICON_SIZE {
			let cy = row as i32 * CELL + HALF;
			if cy < ya || cy >= yb {
				continue;
			}
			for col in 0..ICON_SIZE {
				let cx = col as i32 * CELL + HALF;
				if cx >= xa && cx < xb {
					grid[row][col] = Some(color);
				}
			}
		}
	}

	grid
}

fn icon() -> &'static [[Cell; ICON_SIZE]; ICON_SIZE] {
	ICON.get_or_init(rasterize)
}

/// Bounding box of non-empty cells: (row_lo, row_hi_excl, col_lo, col_hi_excl).
fn content_bbox(g: &[[Cell; ICON_SIZE]; ICON_SIZE]) -> (usize, usize, usize, usize) {
	let (mut rlo, mut rhi, mut clo, mut chi) = (ICON_SIZE, 0, ICON_SIZE, 0);
	for (r, row) in g.iter().enumerate() {
		for (c, cell) in row.iter().enumerate() {
			if cell.is_some() {
				if r < rlo {
					rlo = r;
				}
				if r + 1 > rhi {
					rhi = r + 1;
				}
				if c < clo {
					clo = c;
				}
				if c + 1 > chi {
					chi = c + 1;
				}
			}
		}
	}
	if rlo == ICON_SIZE {
		(0, 0, 0, 0)
	} else {
		(rlo, rhi, clo, chi)
	}
}

/// Render the trimmed icon as half-block lines using transparent backgrounds
/// for empty cells (so terminal background shows through, no black box).
fn icon_lines() -> Vec<String> {
	let g = icon();
	let (rlo, rhi, clo, chi) = content_bbox(g);
	if rhi == rlo {
		return Vec::new();
	}
	let width = chi - clo;

	// Half-block rendering packs 2 grid rows into 1 terminal row. If content
	// height is odd, naively pairing from the top (rlo,rlo+1)(rlo+2,rlo+3)…
	// leaves the LAST row alone — its bottom = None, so bottom features
	// (feet) render as floating upper halves. Shift the start row up by 1
	// when odd and there's headroom, so the bottom row pairs with its real
	// neighbor and renders as a proper lower half block, sitting on the
	// baseline. The synthetic top row (above rlo) is treated as transparent.
	let start = if (rhi - rlo) % 2 == 1 && rlo > 0 {
		rlo - 1
	} else {
		rlo
	};

	let cell_at = |r: usize, c: usize| -> Cell {
		if r >= rlo && r < rhi {
			g[r][c]
		} else {
			None
		}
	};

	let mut lines = Vec::new();
	let mut r = start;
	while r < rhi {
		let mut s = String::with_capacity(width * 24);
		for c in clo..chi {
			let top = cell_at(r, c);
			let bot = cell_at(r + 1, c);
			match (top, bot) {
				(None, None) => s.push(' '),
				(Some((tr, tg, tb)), None) => {
					// Reset bg, set fg to top color, draw upper half block.
					s.push_str(&format!("\x1b[49m\x1b[38;2;{tr};{tg};{tb}m▀"));
				}
				(None, Some((br, bg, bb))) => {
					// Reset bg, set fg to bottom color, draw lower half block.
					s.push_str(&format!("\x1b[49m\x1b[38;2;{br};{bg};{bb}m▄"));
				}
				(Some((tr, tg, tb)), Some((br, bg, bb))) => {
					if (tr, tg, tb) == (br, bg, bb) {
						// Same color top & bottom: draw a solid full block with
						// fg only. Avoids relying on the terminal rendering ▀
						// with a matching bg as a seamless full block (some
						// fonts/terminals leave a hairline gap).
						s.push_str(&format!("\x1b[49m\x1b[38;2;{tr};{tg};{tb}m█"));
					} else {
						s.push_str(&format!(
							"\x1b[38;2;{tr};{tg};{tb}m\x1b[48;2;{br};{bg};{bb}m▀"
						));
					}
				}
			}
		}
		s.push_str("\x1b[0m");
		lines.push(s);
		r += 2;
	}
	lines
}

/// Width (in character cells) of the trimmed icon — used to align trailing info
/// when the info block has more lines than the icon has rows.
fn icon_width() -> usize {
	let (_, _, clo, chi) = content_bbox(icon());
	chi - clo
}

/// Print the startup banner: pixel icon on the left, info block on the right.
/// `extra` lines (already styled) are appended below the standard role/model/cwd
/// trio so callers can attach the tip + shortcuts hint inline with the icon
/// instead of dumping them on separate rows below.
pub fn print_startup_banner(role: &str, model: &str, cwd: &Path, extra: &[String]) {
	let icon = icon_lines();
	let icon_w = icon_width();
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
	let blank_icon = " ".repeat(icon_w);
	for i in 0..total {
		let icon_line = icon.get(i).map(String::as_str).unwrap_or(&blank_icon);
		let info_line = info.get(i).map(String::as_str).unwrap_or("");
		println!("{icon_line}{pad}{info_line}");
	}
}
