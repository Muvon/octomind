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

//! OSC 8 Markdown link extraction and rendering. `extract_links` mutates the
//! composite so layout sees only labels; `render_links` re-adds the hyperlink
//! escape sequences after wrapping. Both are exercised together here through
//! the same path `MarkdownRenderer::styled_text` uses.

use super::*;
use termimad::minimad::{Line, Text};
use termimad::MadSkin;

/// Extract links from `markdown` and render it, returning both.
fn render(markdown: &str) -> (Vec<MarkdownLink>, String) {
	let mut text = Text::from(markdown);
	let mut links = Vec::new();
	for line in &mut text.lines {
		match line {
			Line::Normal(composite) => extract_links(composite, markdown, &[], &mut links),
			Line::TableRow(row) => {
				for cell in &mut row.cells {
					extract_links(cell, markdown, &[], &mut links);
				}
			}
			_ => {}
		}
	}
	let skin = MadSkin::default();
	let rendered = render_links(FmtText::from_text(&skin, text, Some(80)), &links);
	(links, rendered)
}

#[test]
fn plain_link_is_extracted_and_rendered_as_osc8() {
	let (links, rendered) = render("see [docs](https://example.com) now");
	assert_eq!(links.len(), 1);
	assert_eq!(links[0].destination, "https://example.com/");
	assert!(rendered.contains("\x1b]8;;https://example.com/\x1b\\"), "{rendered:?}");
	assert!(rendered.contains("docs"), "{rendered:?}");
	assert!(rendered.contains("\x1b]8;;\x1b\\"), "{rendered:?}");
}

#[test]
fn image_links_are_skipped() {
	let (links, rendered) = render("![pic](https://example.com/i.png)");
	assert!(links.is_empty());
	assert!(!rendered.contains("\x1b]8;"), "{rendered:?}");
}

#[test]
fn escaped_link_prefix_is_skipped() {
	let (links, rendered) = render("\\[docs](https://example.com)");
	assert!(links.is_empty());
	assert!(!rendered.contains("\x1b]8;"), "{rendered:?}");
}

#[test]
fn links_inside_inline_code_are_skipped() {
	let (links, _) = render("run `[x](https://e.com)` now");
	assert!(links.is_empty());
}

#[test]
fn link_with_title_is_accepted() {
	let (links, _) = render("[l](https://e.com \"the title\")");
	assert_eq!(links.len(), 1);
	assert_eq!(links[0].destination, "https://e.com/");
}

#[test]
fn balanced_nested_parentheses_stay_in_the_destination() {
	let (links, _) = render("[l](https://e.com/a(b))");
	assert_eq!(links.len(), 1);
	assert_eq!(links[0].destination, "https://e.com/a(b)");
}

#[test]
fn relative_path_becomes_a_file_url() {
	let (links, _) = render("[f](README.md)");
	assert_eq!(links.len(), 1);
	assert!(links[0].destination.starts_with("file://"), "{}", links[0].destination);
	assert!(links[0].destination.ends_with("/README.md"));
}

#[test]
fn unsupported_scheme_and_empty_destination_are_skipped() {
	let (links, _) = render("[a](foo://x) [b]()");
	assert!(links.is_empty());
}

#[test]
fn multiple_links_on_one_line_are_all_extracted() {
	let (links, rendered) = render("[a](https://a.example) and [b](https://b.example)");
	assert_eq!(links.len(), 2);
	assert!(rendered.contains("https://a.example/"), "{rendered:?}");
	assert!(rendered.contains("https://b.example/"), "{rendered:?}");
}

#[test]
fn link_destination_parses_until_closing_paren() {
	let (destination, length) = link_destination("https://e.com/) tail").expect("parsed");
	assert_eq!(destination, "https://e.com/");
	assert_eq!(length, "https://e.com/)".len());
}

#[test]
fn link_destination_supports_angle_brackets() {
	let (destination, _) = link_destination("<https://e.com/a>) rest").expect("parsed");
	assert_eq!(destination, "https://e.com/a");
}

#[test]
fn link_destination_rejects_unbalanced_parens() {
	assert!(link_destination("(unbalanced").is_none());
}

#[test]
fn link_destination_rejects_empty_and_unsupported_schemes() {
	assert!(link_destination("  )").is_none());
	assert!(link_destination("foo://x)").is_none());
}
