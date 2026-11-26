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

use super::*;
use std::str::FromStr;
use termimad::FmtText;

const RICH_DOC: &str = r#"# Title

Some **bold** and *italic* text with `inline code`.

## Section

- item one
- item two

| col a | col b |
|-------|-------|
| 1     | 2     |

```rust
fn main() {
    println!("hello");
}
```

```unknownlang
opaque body
```

```
no language fence
```

> a quote line
"#;

#[test]
fn test_theme_parsing_roundtrip() {
	for name in MarkdownTheme::all_themes() {
		let theme = MarkdownTheme::from_str(name).expect("known theme parses");
		assert_eq!(theme.as_str(), name);
		// Every theme maps to a syntax-highlighting theme
		assert!(!theme.get_syntax_theme_name().is_empty());
	}
	// Case-insensitive
	assert!(MarkdownTheme::from_str("OCEAN").is_ok());
	assert!(MarkdownTheme::from_str("no-such-theme").is_err());
}

#[test]
fn test_render_rich_document_in_every_theme() {
	for name in MarkdownTheme::all_themes() {
		let theme = MarkdownTheme::from_str(name).expect("theme");
		let renderer = MarkdownRenderer::with_theme(theme);
		let rendered = renderer
			.render(RICH_DOC)
			.unwrap_or_else(|e| panic!("render fails for theme {name}: {e}"));
		// Content survives styling in every theme
		assert!(rendered.contains("Title"), "theme {name}");
		assert!(rendered.contains("item one"), "theme {name}");
		assert!(rendered.contains("hello"), "theme {name}");
		assert!(rendered.contains("opaque body"), "theme {name}");
		assert!(rendered.contains("no language fence"), "theme {name}");
	}
}

#[test]
fn test_set_theme_switches_in_place() {
	let mut renderer = MarkdownRenderer::new();
	assert_eq!(renderer.get_theme().as_str(), "default");
	renderer.set_theme(MarkdownTheme::Monokai);
	assert_eq!(renderer.get_theme().as_str(), "monokai");
	assert!(renderer.render("**bold**").expect("render").contains("bold"));
}

#[test]
fn test_render_plain_and_empty_inputs() {
	let renderer = MarkdownRenderer::new();
	// Empty input renders without error
	renderer.render("").expect("empty renders");
	let plain = renderer.render("just words").expect("plain renders");
	assert!(plain.contains("just words"));
	// Unclosed fence must not panic or lose content
	let unclosed = renderer.render("```rust\nlet x = 1;").expect("unclosed fence renders");
	assert!(unclosed.contains("let x = 1;"));
}

#[test]
fn test_printed_document_keeps_code_and_prose_on_separate_lines() {
	let renderer = MarkdownRenderer::new();
	let rendered = renderer
		.render_with_syntax_highlighting("## Diagram\n\n```text\n┌────┐\n└────┘\n```\n\nAfter the diagram.")
		.expect("printable Markdown renders");
	let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
		.expect("valid ANSI style regex")
		.replace_all(&rendered, "");
	assert!(plain.contains("Diagram\n"));
	assert!(plain.contains("\n┌─ text ─\n┌────┐\n└────┘\n└─────\n\n"));
	assert!(plain.ends_with("After the diagram.\n"));
	assert!(rendered.ends_with('\n'));
}

#[test]
fn test_is_markdown_content_heuristics() {
	assert!(is_markdown_content("## Heading"));
	assert!(is_markdown_content("- a list item\n- another"));
	assert!(is_markdown_content("```\ncode\n```"));
	assert!(is_markdown_content("some **bold** claim"));
	assert!(!is_markdown_content("plain sentence with nothing special"));
	assert!(!is_markdown_content(""));
}

const SQUEEZED_WORK_TABLE: &str = r#"| Phase / owner | Work | Gate |
|---|---|---|
| 1 · Discovery (alice) | ✓ | Scope, budget and success metrics signed off by the steering committee and recorded in the decision log before any design work starts |
| 2 · Design (bob) | Architecture review, threat model and rollback plan sign-off | Architecture review passed with no blocking findings, threat model reviewed and approved by security, data flows documented |
| 3 · Build (carol) | ✓ | Unit and integration tests green, coverage at or above eighty percent on changed code, no clippy warnings, migrations rehearsed |
| 4 · Rollout (dan) | ✓ | Canary healthy for forty-eight hours with error budget intact, rollback rehearsed, on-call briefed and dashboard reviewed |"#;

#[test]
fn test_wide_table_keeps_short_columns_readable() {
	let renderer = MarkdownRenderer::new();
	let rendered = renderer.styled_text(SQUEEZED_WORK_TABLE, 79);
	let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
		.expect("valid ANSI style regex")
		.replace_all(&rendered, "");
	let lines: Vec<&str> = plain.lines().collect();
	let widths: Vec<usize> = lines[1]
		.trim_matches(['├', '┤'])
		.split('┼')
		.map(|segment| segment.chars().count())
		.collect();
	assert_eq!(widths.len(), 3, "{plain}");
	assert_eq!(
		widths.iter().sum::<usize>() + widths.len() + 1,
		79,
		"table keeps the viewport width: {plain}"
	);
	assert!(widths[1] >= 18, "Work column squeezed to {:?}: {plain}", widths);
	let start = lines
		.iter()
		.position(|line| line.contains("Architecture review"))
		.expect("row");
	let end = lines.iter().position(|line| line.contains("sign-off")).expect("row");
	assert!(
		end - start < 4,
		"long Work cell wraps over {} lines: {plain}",
		end - start + 1
	);
	for token in SQUEEZED_WORK_TABLE
		.lines()
		.filter(|line| !line.starts_with("|---"))
		.flat_map(|line| line.split('|'))
		.flat_map(str::split_whitespace)
	{
		assert!(plain.contains(token), "lost {token:?}: {plain}");
	}
}

#[test]
fn test_fitting_table_keeps_termimad_layout() {
	let renderer = MarkdownRenderer::new();
	let markdown = "| Phase / owner | Work | Gate |\n|---|---|---|\n| 1 · Discovery (alice) | ✓ | Scope signed |";
	let ours = renderer.styled_text(markdown, 79);
	let reference = FmtText::from_text(&renderer.skin, Text::from(markdown), Some(79)).to_string();
	assert_eq!(ours, reference);
}

const PATH_TABLE: &str = r#"| Step | Detail | Notes |
|---|---|---|
| 1 | /srv/app/deployments/production/config-templates/default.toml | first table that overflows the viewport so its columns must be balanced |
| 2 | /opt/builds/artifact-cache/f7e8d9c0b1a2/linux-x86_64 | second row of the first table |"#;

fn without_ansi(rendered: &str) -> String {
	regex::Regex::new(r"\x1b\[[0-9;]*m")
		.expect("valid ANSI style regex")
		.replace_all(rendered, "")
		.to_string()
}

fn column_widths(plain: &str) -> Vec<usize> {
	plain
		.lines()
		.find(|line| line.contains('┼'))
		.expect("table rule line")
		.trim_matches(['├', '┤'])
		.split('┼')
		.map(|segment| segment.chars().count())
		.collect()
}

// A column pinned by one long unbreakable word must not pay for the balanced
// layout with more lines than termimad's own allocation.
#[test]
fn test_long_path_table_is_never_taller_than_termimad() {
	let renderer = MarkdownRenderer::new();
	for width in [79, 119] {
		let ours = without_ansi(&renderer.styled_text(PATH_TABLE, width));
		let reference =
			without_ansi(&FmtText::from_text(&renderer.skin, Text::from(PATH_TABLE), Some(width)).to_string());
		assert!(
			ours.lines().count() <= reference.lines().count(),
			"balanced layout is taller at {width}: {} vs {}\n{ours}",
			ours.lines().count(),
			reference.lines().count()
		);
		if width == 79 {
			assert_eq!(ours, reference, "termimad's shorter layout must be kept verbatim");
		}
	}
}

// Equal height: the balanced layout still wins, because the squeezed column
// becomes readable without costing a single line.
#[test]
fn test_balanced_layout_wins_a_height_tie() {
	let renderer = MarkdownRenderer::new();
	let ours = without_ansi(&renderer.styled_text(SQUEEZED_WORK_TABLE, 79));
	let reference =
		without_ansi(&FmtText::from_text(&renderer.skin, Text::from(SQUEEZED_WORK_TABLE), Some(79)).to_string());
	assert_eq!(ours.lines().count(), reference.lines().count(), "{ours}");
	assert_ne!(ours, reference, "the balanced layout must win the tie");
	let ours_widths = column_widths(&ours);
	let reference_widths = column_widths(&reference);
	assert!(ours_widths[1] > reference_widths[1], "{ours}");
	assert_eq!(ours_widths.iter().sum::<usize>() + ours_widths.len() + 1, 79);
}

// Fewer lines: the balanced layout is used and the table keeps the viewport width.
#[test]
fn test_balanced_layout_used_when_it_shortens_the_table() {
	let renderer = MarkdownRenderer::new();
	let ours = without_ansi(&renderer.styled_text(SQUEEZED_WORK_TABLE, 119));
	let reference =
		without_ansi(&FmtText::from_text(&renderer.skin, Text::from(SQUEEZED_WORK_TABLE), Some(119)).to_string());
	assert!(
		ours.lines().count() < reference.lines().count(),
		"119 must keep the height gain: {} vs {}\n{ours}",
		ours.lines().count(),
		reference.lines().count()
	);
	let widths = column_widths(&ours);
	assert!(widths[1] >= 30, "Work squeezed to {widths:?}: {ours}");
	assert_eq!(widths.iter().sum::<usize>() + widths.len() + 1, 119);
}
