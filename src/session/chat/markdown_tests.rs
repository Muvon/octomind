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
	assert!(renderer
		.render("**bold**")
		.expect("render")
		.contains("bold"));
}

#[test]
fn test_render_plain_and_empty_inputs() {
	let renderer = MarkdownRenderer::new();
	// Empty input renders without error
	renderer.render("").expect("empty renders");
	let plain = renderer.render("just words").expect("plain renders");
	assert!(plain.contains("just words"));
	// Unclosed fence must not panic or lose content
	let unclosed = renderer
		.render("```rust\nlet x = 1;")
		.expect("unclosed fence renders");
	assert!(unclosed.contains("let x = 1;"));
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
