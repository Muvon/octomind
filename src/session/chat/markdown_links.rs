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

use regex::Regex;
use std::ops::Range;
use std::sync::LazyLock;
use termimad::minimad::Composite;
use termimad::{FmtComposite, FmtLine, FmtTableRow, FmtText};

static LINK_START: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\[([^\]\n]+)\]\(").expect("valid Markdown link prefix regex"));

pub(super) struct MarkdownLink {
	source: Range<usize>,
	label: Range<usize>,
	destination: String,
}

pub(super) fn extract_links(
	composite: &mut Composite<'_>,
	markdown: &str,
	tags: &[Range<usize>],
	links: &mut Vec<MarkdownLink>,
) {
	if composite.is_code() || composite.compounds.is_empty() {
		return;
	}
	let base = markdown.as_ptr() as usize;
	let first = &composite.compounds[0];
	let last = composite.compounds.last().expect("nonempty composite");
	let start = first.src.as_ptr() as usize - base;
	let end = last.src.as_ptr() as usize - base + last.src.len();
	let source = &markdown[start..end];
	let first_link = links.len();
	let mut consumed = 0;
	for captures in LINK_START.captures_iter(source) {
		let whole = captures.get(0).expect("whole link prefix");
		let label = captures.get(1).expect("link label");
		let address = base + start + whole.start();
		let prefix = &source[..whole.start()];
		if whole.start() < consumed
			|| prefix.ends_with('!')
			|| prefix.chars().rev().take_while(|c| *c == '\\').count() % 2 != 0
			|| tags.iter().any(|tag| tag.contains(&address))
			|| composite.compounds.iter().any(|compound| {
				let start = compound.src.as_ptr() as usize;
				compound.code && (start..start + compound.src.len()).contains(&address)
			}) {
			continue;
		}
		let Some((destination, length)) = link_destination(&source[whole.end()..]) else {
			continue;
		};
		consumed = whole.end() + length;
		links.push(MarkdownLink {
			source: address..base + start + consumed,
			label: base + start + label.start()..base + start + label.end(),
			destination,
		});
	}
	if first_link == links.len() {
		return;
	}
	let mut compounds = Vec::new();
	for compound in &composite.compounds {
		let start = compound.src.as_ptr() as usize;
		let end = start + compound.src.len();
		let mut cursor = start;
		for link in &links[first_link..] {
			if link.source.end <= cursor || link.source.start >= end {
				continue;
			}
			if cursor < link.source.start {
				compounds.push(compound.sub(cursor - start, link.source.start - start));
			}
			let label_start = cursor.max(link.label.start);
			let label_end = end.min(link.label.end);
			if label_start < label_end {
				compounds.push(compound.sub(label_start - start, label_end - start).code());
			}
			cursor = end.min(link.source.end);
		}
		if cursor < end {
			compounds.push(compound.tail(cursor - start));
		}
	}
	composite.compounds = compounds;
}

fn link_destination(source: &str) -> Option<(String, usize)> {
	let trimmed = source.trim_start();
	let leading = source.len() - trimmed.len();
	let (destination, end) = if let Some(angled) = trimmed.strip_prefix('<') {
		let end = angled.find('>')?;
		(&angled[..end], leading + end + 2)
	} else {
		let mut depth = 0usize;
		let mut escaped = false;
		let mut end = trimmed.len();
		for (index, c) in trimmed.char_indices() {
			if escaped {
				escaped = false;
				continue;
			}
			match c {
				'\\' => escaped = true,
				'(' => depth += 1,
				')' if depth > 0 => depth -= 1,
				')' => {
					end = index;
					break;
				}
				c if c.is_whitespace() => {
					end = index;
					break;
				}
				_ => {}
			}
		}
		if depth != 0 {
			return None;
		}
		(&trimmed[..end], leading + end)
	};
	let mut remainder = source[end..].trim_start();
	if remainder.len() < source[end..].len() {
		if let Some(quote @ ('"' | '\'')) = remainder.chars().next() {
			let title_end = remainder[1..].find(quote)? + 1;
			remainder = remainder[title_end + 1..].trim_start();
		}
	}
	if !remainder.starts_with(')')
		|| destination.is_empty()
		|| destination.chars().any(char::is_control)
	{
		return None;
	}
	// OSC 8 destinations must be URIs; local Markdown file links use file://.
	let destination = match url::Url::parse(destination) {
		Ok(url) if matches!(url.scheme(), "http" | "https" | "ftp" | "mailto" | "file") => url,
		Ok(_) => return None,
		Err(_) => {
			let path = std::path::Path::new(destination);
			let path = if path.is_absolute() {
				path.to_path_buf()
			} else {
				std::env::current_dir().ok()?.join(path)
			};
			url::Url::from_file_path(path).ok()?
		}
	};
	Some((destination.to_string(), source.len() - remainder.len() + 1))
}

pub(super) fn render_links(text: FmtText<'_, '_>, links: &[MarkdownLink]) -> String {
	if links.is_empty() {
		return text.to_string();
	}
	// Layout sees only labels. Add OSC 8 after wrapping, closing each fragment
	// separately so table padding, bullets, and subsequent lines aren't linked.
	let replacements: Vec<_> = text
		.lines
		.iter()
		.flat_map(|line| match line {
			FmtLine::Normal(composite) => composite.compounds.iter().collect::<Vec<_>>(),
			FmtLine::TableRow(row) => row.cells.iter().flat_map(|cell| &cell.compounds).collect(),
			_ => Vec::new(),
		})
		.map(|compound| {
			let start = compound.src.as_ptr() as usize;
			links
				.iter()
				.find(|link| {
					link.label.contains(&start) && start + compound.src.len() <= link.label.end
				})
				.map(|link| {
					format!(
						"\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
						link.destination, compound.src
					)
				})
		})
		.collect();
	let mut replacements = replacements.iter();
	let lines = text
		.lines
		.into_iter()
		.map(|line| match line {
			FmtLine::Normal(composite) => {
				FmtLine::Normal(apply_link_sequences(composite, &mut replacements))
			}
			FmtLine::TableRow(row) => FmtLine::TableRow(FmtTableRow {
				cells: row
					.cells
					.into_iter()
					.map(|cell| apply_link_sequences(cell, &mut replacements))
					.collect(),
			}),
			FmtLine::TableRule(rule) => FmtLine::TableRule(rule),
			FmtLine::HorizontalRule => FmtLine::HorizontalRule,
		})
		.collect();
	FmtText {
		skin: text.skin,
		lines,
		width: text.width,
	}
	.to_string()
}

fn apply_link_sequences<'s>(
	mut composite: FmtComposite<'s>,
	replacements: &mut impl Iterator<Item = &'s Option<String>>,
) -> FmtComposite<'s> {
	for compound in &mut composite.compounds {
		if let Some(replacement) = replacements.next().expect("one replacement per compound") {
			compound.src = replacement;
		}
	}
	composite
}
