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

type CC<'a> = CommandCompleter<'a>;

fn test_config() -> crate::config::Config {
	toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template")
}

#[test]
fn test_find_at_query() {
	// At line start
	assert_eq!(CC::find_at_query("@src", 4), Some((0, "src")));
	// After whitespace
	assert_eq!(CC::find_at_query("look @ma", 8), Some((5, "ma")));
	// Empty query right after @
	assert_eq!(CC::find_at_query("@", 1), Some((0, "")));
	// @ glued to a word (email-like) is not a file query
	assert_eq!(CC::find_at_query("user@host", 9), None);
	// Whitespace inside the query cancels it
	assert_eq!(CC::find_at_query("see @src file", 13), None);
	// The last @ before the cursor wins
	assert_eq!(CC::find_at_query("see @a @b", 9), Some((7, "b")));
	// No @ at all
	assert_eq!(CC::find_at_query("plain text", 10), None);
	// Cursor beyond line length is clamped, not panicking
	assert_eq!(CC::find_at_query("@x", 99), Some((0, "x")));
	// Multi-byte content before the @ must not panic or shift offsets
	let line = "héllo @fi";
	assert_eq!(CC::find_at_query(line, line.len()), Some((7, "fi")));
}

#[test]
fn test_is_image_file() {
	assert!(CC::is_image_file("photo.PNG"));
	assert!(CC::is_image_file("dir/pic.jpeg"));
	assert!(CC::is_image_file("icon.svg"));
	assert!(!CC::is_image_file("doc.pdf"));
	assert!(!CC::is_image_file("main.rs"));
	assert!(!CC::is_image_file("png"));
}

#[test]
fn test_expand_tilde() {
	let home = dirs::home_dir().expect("home dir in test env");
	assert_eq!(CC::expand_tilde("~/x"), home.join("x"));
	assert_eq!(CC::expand_tilde("~"), home);
	assert_eq!(CC::expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
	assert_eq!(CC::expand_tilde("rel/path"), PathBuf::from("rel/path"));
}

fn pair(s: &str) -> Pair {
	Pair {
		display: s.to_string(),
		replacement: s.to_string(),
	}
}

#[test]
fn test_find_common_prefix() {
	assert_eq!(CC::find_common_prefix(&[]), "");
	assert_eq!(
		CC::find_common_prefix(&[pair("src/main.rs")]),
		"src/main.rs"
	);
	assert_eq!(
		CC::find_common_prefix(&[pair("src/main.rs"), pair("src/mcp")]),
		"src/m"
	);
	assert_eq!(CC::find_common_prefix(&[pair("abc"), pair("xyz")]), "");
	// Multi-byte common prefix must land on a char boundary
	assert_eq!(CC::find_common_prefix(&[pair("télé"), pair("téla")]), "tél");
}

#[test]
fn test_filter_and_limit_candidates() {
	// Empty stays empty
	assert!(CC::filter_and_limit_candidates(Vec::new(), "x").is_empty());

	// Common prefix longer than input is prepended as a partial completion
	let result = CC::filter_and_limit_candidates(vec![pair("src/main.rs"), pair("src/mcp")], "s");
	assert_eq!(result[0].replacement, "src/m");
	assert!(result[0].display.contains("(partial)"));
	assert_eq!(result.len(), 3);

	// Never more than 10 candidates
	let many: Vec<Pair> = (0..20).map(|i| pair(&format!("file{}", i))).collect();
	assert_eq!(CC::filter_and_limit_candidates(many, "file").len(), 10);
}

#[test]
fn test_complete_file_path_directory_listing() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let base = tmp.path();
	std::fs::create_dir(base.join("subdir")).expect("mkdir");
	std::fs::write(base.join("pic.png"), b"x").expect("write image");
	std::fs::write(base.join("notes.txt"), b"x").expect("write text");

	// Trailing slash lists the directory: dirs first (with trailing /),
	// image files included, non-image files excluded
	let listing = CC::complete_file_path(&format!("{}/", base.display()));
	assert_eq!(listing.len(), 2);
	assert!(listing[0].replacement.ends_with("subdir/"));
	assert!(listing[1].replacement.ends_with("pic.png"));

	// Filename prefix filtering is case-insensitive
	let filtered = CC::complete_file_path(&format!("{}/PI", base.display()));
	assert_eq!(filtered.len(), 1);
	assert!(filtered[0].replacement.ends_with("pic.png"));

	// Non-matching prefix yields nothing
	assert!(CC::complete_file_path(&format!("{}/zz", base.display())).is_empty());
}

/// Cache-dependent assertions live in ONE test: FILE_CACHE is a process
/// global, and parallel tests mutating it would race each other.
#[test]
fn test_fuzzy_match_and_at_dispatch() {
	*FILE_CACHE.lock().expect("file cache lock") =
		Some(vec!["src/main.rs".to_string(), "README.md".to_string()]);

	let matches = CC::fuzzy_match_files("main", 10);
	assert_eq!(matches.len(), 1);
	assert_eq!(matches[0].replacement, "src/main.rs");

	// max_results truncation
	assert_eq!(CC::fuzzy_match_files("m", 1).len(), 1);

	// complete() dispatches @-queries to fuzzy matching, replacing from the @
	let config = test_config();
	let completer = CommandCompleter::new(&config, "developer");
	let (start, candidates) = completer.complete("look @main", 10);
	assert_eq!(start, 5);
	assert_eq!(candidates.len(), 1);
	assert_eq!(candidates[0].replacement, "src/main.rs");
}

#[test]
fn test_complete_command_dispatch() {
	let config = test_config();
	let completer = CommandCompleter::new(&config, "developer");

	// Subcommand completion for /mcp
	let (start, candidates) = completer.complete("/mcp li", 7);
	assert_eq!(start, 5);
	assert_eq!(candidates.len(), 1);
	assert_eq!(candidates[0].replacement, "list");

	// Level completion for /loglevel
	let (_, candidates) = completer.complete("/loglevel d", 11);
	assert_eq!(candidates.len(), 1);
	assert_eq!(candidates[0].replacement, "debug");

	// Plain text (no / and no @) completes nothing
	let (start, candidates) = completer.complete("hello world", 11);
	assert_eq!(start, 0);
	assert!(candidates.is_empty());

	// Command-name completion: every candidate extends the typed prefix
	let (start, candidates) = completer.complete("/he", 3);
	assert_eq!(start, 0);
	assert!(!candidates.is_empty());
	assert!(candidates
		.iter()
		.all(|c| c.replacement.starts_with("/he") || c.display.contains("(partial)")));
}

#[test]
fn test_hint() {
	let config = test_config();
	let completer = CommandCompleter::new(&config, "developer");

	assert_eq!(completer.hint(""), None);
	assert_eq!(completer.hint("not a command"), None);
	assert_eq!(
		completer.hint("/image"),
		Some(" <path_to_image>".to_string())
	);
	assert_eq!(
		completer.hint("/mcp"),
		Some(" [list|info|full|health|dump|validate]".to_string())
	);
	assert_eq!(
		completer.hint("/loglevel"),
		Some(" [none|info|debug]".to_string())
	);
}
