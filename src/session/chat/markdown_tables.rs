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

// Table layout. termimad fits a too-wide table by cutting every column toward
// the average of its cells, which squeezes a column holding one long cell into
// a few characters over many lines while columns with uniformly long cells keep
// most of their width. Here such a table gives every column a share of the width
// proportional to its longest cell, so the worst cell of each column wraps to
// about the same number of lines. That layout is only used when it is not taller
// than termimad's own, which stays the baseline for every table.

use termimad::minimad::{Alignment, Compound, Line, Text};
use termimad::{fix_all_tables, wrap, FmtComposite, FmtLine, FmtTableRow, FmtText, MadSkin, RelativePosition, Spacing};

/// Narrowest column termimad ever produces; a narrower cell cannot be wrapped.
const MINIMAL_COLUMN_WIDTH: usize = 3;

/// What one column needs to stay readable.
#[derive(Debug, Default, Clone, Copy)]
struct Column {
	/// Longest cell, the column's natural width.
	longest: usize,
	/// First row cell: kept on a single line.
	header: usize,
	/// Longest unbreakable word: never cut in the middle.
	word: usize,
}

/// Lay the text out for `width`: prose through termimad, each table on its own.
pub(super) fn layout<'k, 's>(skin: &'k MadSkin, text: Text<'s>, width: usize) -> FmtText<'k, 's> {
	let mut runs: Vec<(bool, Vec<Line<'s>>)> = Vec::new();
	for line in text.lines {
		let is_table = matches!(line, Line::TableRow(_) | Line::TableRule(_));
		match runs.last_mut() {
			Some((table, lines)) if *table == is_table => lines.push(line),
			_ => runs.push((is_table, vec![line])),
		}
	}
	let mut lines = Vec::new();
	for (is_table, run) in runs {
		if is_table {
			lines.extend(layout_table(skin, run, width));
		} else {
			lines.extend(FmtText::from_text(skin, Text { lines: run }, Some(width)).lines);
		}
	}
	FmtText {
		skin,
		lines,
		width: Some(width),
	}
}

fn layout_table<'s>(skin: &MadSkin, run: Vec<Line<'s>>, width: usize) -> Vec<FmtLine<'s>> {
	// termimad's own layout is the height baseline: a balanced table is only
	// worth keeping when it doesn't need more lines than this one.
	let mut baseline: Vec<FmtLine<'s>> = run.iter().map(|line| FmtLine::from(line.clone(), skin)).collect();
	let nbcols = column_count(&baseline);
	if nbcols == 0 || width == 0 {
		return baseline;
	}
	let columns = measure(&baseline, nbcols);
	let Some(widths) = balanced_widths(&columns, width) else {
		// The table fits as is, or not even the readable minimums do: termimad's layout.
		fix_all_tables(&mut baseline, width, skin);
		return baseline;
	};
	let mut balanced: Vec<FmtLine<'s>> = run.into_iter().map(|line| FmtLine::from(line, skin)).collect();
	widen(&mut balanced, nbcols, &widths, skin);
	fix_all_tables(&mut baseline, width, skin);
	// A column pinned by one long unbreakable word can claim most of the width
	// and wrap its neighbour to pieces; never trade more lines for that.
	if balanced.len() <= baseline.len() {
		balanced
	} else {
		baseline
	}
}

fn column_count(lines: &[FmtLine<'_>]) -> usize {
	lines
		.iter()
		.filter_map(|line| match line {
			FmtLine::TableRow(FmtTableRow { cells }) => Some(cells.len()),
			FmtLine::TableRule(rule) => Some(rule.aligns.len()),
			_ => None,
		})
		.max()
		.unwrap_or(0)
}

fn measure(lines: &[FmtLine<'_>], nbcols: usize) -> Vec<Column> {
	let mut columns = vec![Column::default(); nbcols];
	let mut header_row = true;
	for line in lines {
		let FmtLine::TableRow(FmtTableRow { cells }) = line else {
			continue;
		};
		for (index, column) in columns.iter_mut().enumerate() {
			let Some(cell) = cells.get(index) else {
				continue;
			};
			column.longest = column.longest.max(cell.visible_length);
			if header_row {
				column.header = cell.visible_length;
			}
			for word in cell
				.compounds
				.iter()
				.flat_map(|compound| compound.src.split_whitespace())
			{
				column.word = column
					.word
					.max(FmtComposite::from_compound(Compound::raw_str(word)).visible_length);
			}
		}
		header_row = false;
	}
	columns
}

/// Column widths for a table too wide for `width`; `None` when termimad's fit applies.
fn balanced_widths(columns: &[Column], width: usize) -> Option<Vec<usize>> {
	let available = width.checked_sub(1 + columns.len())?;
	let natural: Vec<usize> = columns
		.iter()
		.map(|column| column.longest.max(MINIMAL_COLUMN_WIDTH))
		.collect();
	if natural.iter().sum::<usize>() <= available {
		return None;
	}
	let floors: Vec<usize> = columns
		.iter()
		.map(|column| column.header.max(column.word).max(MINIMAL_COLUMN_WIDTH))
		.collect();
	if floors.iter().sum::<usize>() > available {
		return None;
	}
	let total: usize = columns.iter().map(|column| column.longest).sum();
	let mut widths: Vec<usize> = columns
		.iter()
		.zip(&floors)
		.zip(&natural)
		.map(|((column, floor), natural)| (available * column.longest / total).max(*floor).min(*natural))
		.collect();
	// Clamping to the floors leaves width over, integer division leaves some missing:
	// hand it to or take it from the column that still wraps the most.
	loop {
		let used: usize = widths.iter().sum();
		if used == available {
			return Some(widths);
		}
		let room = |index: usize| {
			if used < available {
				natural[index] - widths[index]
			} else {
				widths[index] - floors[index]
			}
		};
		let index = (0..columns.len())
			.filter(|&index| room(index) > 0)
			.max_by_key(|&index| natural[index] - widths[index])
			.expect("a column is always wide enough to take the remaining width");
		if used < available {
			widths[index] += 1;
		} else {
			widths[index] -= 1;
		}
	}
}

/// termimad's row resizing for the given column widths: cells wider than their
/// column continue on inserted rows, and every cell is padded to the column.
fn widen<'s>(lines: &mut Vec<FmtLine<'s>>, nbcols: usize, widths: &[usize], skin: &MadSkin) {
	for line in lines.iter_mut() {
		match line {
			FmtLine::TableRow(FmtTableRow { cells }) => cells.resize_with(nbcols, FmtComposite::new),
			FmtLine::TableRule(rule) => rule.set_nbcols(nbcols),
			_ => {}
		}
	}
	wrap_rows(skin, lines, widths);
}

/// Cells wider than their column continue on inserted rows; alignment and rule
/// widths then follow the last rule above each row, as in termimad.
fn wrap_rows<'s>(skin: &MadSkin, lines: &mut Vec<FmtLine<'s>>, widths: &[usize]) {
	// Rows are visited bottom-up so the rows inserted below one stay unvisited.
	for index in (0..lines.len()).rev() {
		let FmtLine::TableRow(FmtTableRow { cells }) = &mut lines[index] else {
			continue;
		};
		let mut overflow: Vec<Vec<FmtComposite<'s>>> = vec![Vec::new(); widths.len()];
		for (column, width) in widths.iter().enumerate() {
			if cells[column].visible_length > *width {
				let mut wrapped =
					wrap::hard_wrap_composite(&cells[column], *width, skin).expect("column widths are at least 3");
				cells[column] = wrapped.remove(0);
				overflow[column] = wrapped;
			}
		}
		let extra = overflow.iter().map(Vec::len).max().unwrap_or(0);
		for wrapped_line in (0..extra).rev() {
			let cells = overflow
				.iter_mut()
				.map(|over| {
					if over.len() > wrapped_line {
						over.remove(wrapped_line)
					} else {
						FmtComposite::new()
					}
				})
				.collect();
			lines.insert(index + 1, FmtLine::TableRow(FmtTableRow { cells }));
		}
	}
	// Alignments come from the last rule line above each row, as in termimad.
	let last = lines.len() - 1;
	let mut aligns = vec![Alignment::Center; widths.len()];
	for (index, line) in lines.iter_mut().enumerate() {
		match line {
			FmtLine::TableRow(FmtTableRow { cells }) => {
				for (column, cell) in cells.iter_mut().enumerate() {
					cell.spacing = Some(Spacing {
						width: widths[column],
						align: aligns[column],
					});
				}
			}
			FmtLine::TableRule(rule) => {
				rule.widths.clone_from_slice(widths);
				aligns.clone_from_slice(&rule.aligns);
				if index == 0 {
					rule.position = RelativePosition::Top;
				} else if index == last {
					rule.position = RelativePosition::Bottom;
				}
			}
			_ => {}
		}
	}
}
