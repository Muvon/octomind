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

//! What the terminal really gets for a plugin's `/learning` answer: one frame,
//! semantic colours when colour is on, none when it is off.

use super::*;
use serde_json::{json, Value};
use std::process::Command;

/// Selects the case the child renders.
const CASE_ENV: &str = "OCTOMIND_RENDER_CASE";

/// Full path of the child test, as libtest names it.
const CHILD: &str = "session::chat::session::commands::display::plugin_tests::render_child";

/// The frame Besydr sends in `text` for a list answer — the shape that used to
/// be framed a second time.
const BESYDR_TEXT: &str = "╭ /learning · developer/project\n│ #1 [high] (learning)\n╰ ✓ /learning · 2 lesson(s)";

fn list_data() -> Value {
	json!({
		"subcommand": "list",
		"role": "developer",
		"project": "project",
		"total": 1,
		"page": 1,
		"total_pages": 1,
		"storage": {"hot_items": 1, "hot_tokens": 40, "cold_items": 2, "cold_tokens": 300,
			"scoped_hot": 1, "global_hot": 0, "scoped_cold": 1, "global_cold": 1,
			"by_type": {"learning": {"hot": 1, "cold": 2}}},
		"lessons": [{"index": 1, "content": "Reuse the captured answer", "title": "", "memory_type": "learning",
			"importance": 0.9, "confidence": "high", "scope": "scoped", "tags": ["display"],
			"created": "2026-11-20T10:00:00Z", "related": [], "evidence": [], "outcome": "unknown"}]
	})
}

fn plugin_output(plugin: &str, command: &str, text: &str, data: Value) -> CommandOutput {
	CommandOutput::Plugin {
		plugin: plugin.to_string(),
		command: command.to_string(),
		text: text.to_string(),
		data,
	}
}

fn case_output(case: &str) -> CommandOutput {
	match case {
		"list" => plugin_output("besydr", "/learning", BESYDR_TEXT, list_data()),
		"error" => plugin_output(
			"besydr",
			"/learning",
			"╰ ✗ /learning",
			json!({"subcommand": "error", "message": "no such lesson"}),
		),
		"evolution" => plugin_output(
			"besydr",
			"/learning",
			"╭ /learning evolution",
			json!({"subcommand": "evolution_list", "project": "p", "domain": "d",
				"records": [{"id": "a1", "name": "ship it", "kind": "rule", "state": "pending"}]}),
		),
		"unknown" => plugin_output("besydr", "/learning", BESYDR_TEXT, json!({"subcommand": "future"})),
		"text-only" => plugin_output("besydr", "/learning", BESYDR_TEXT, json!({"text": BESYDR_TEXT})),
		"show" => plugin_output(
			"besydr",
			"/learning",
			"╭ /learning",
			json!({"subcommand": "show", "index": 5, "memory_type": "learning", "title": "Title",
				"content": "Body"}),
		),
		"delete" => plugin_output(
			"besydr",
			"/learning",
			"╭ /learning",
			json!({"subcommand": "delete", "index": 5, "content_preview": "Reuse the"}),
		),
		"clear" => plugin_output(
			"besydr",
			"/learning",
			"╭ /learning",
			json!({"subcommand": "clear", "deleted": 3, "errors": []}),
		),
		"other-command" => plugin_output("besydr", "/forecast", "╭ /forecast", Value::Null),
		// A different plugin answering `/learning list` with a payload that happens
		// to hold the same keys: its own text must be shown, not our layout.
		"other-plugin" => plugin_output("acme", "/learning", "acme: 1 lesson\nrun /learning show 1", list_data()),
		_ => panic!("unknown render case: {case}"),
	}
}

/// The child side of `render`: prints one case and exits, so colours are forced
/// per process instead of toggled in the shared test process (`colored` decides
/// once per process). Markers keep libtest's own output out of the frame.
#[test]
fn render_child() {
	let Ok(case) = std::env::var(CASE_ENV) else {
		return;
	};
	crate::println!("@@begin@@");
	display_plugin(&case_output(&case));
	crate::println!("@@end@@");
}

fn render(case: &str, color: bool) -> String {
	let mut command = Command::new(std::env::current_exe().expect("test binary path"));
	command
		.args(["--exact", CHILD, "--nocapture", "--test-threads", "1"])
		.env(CASE_ENV, case)
		.env("CLICOLOR_FORCE", if color { "1" } else { "0" })
		.env("TERM", "xterm-256color");
	if color {
		// A stale `NO_COLOR` from the caller's shell would silence the frame.
		command.env_remove("NO_COLOR");
	} else {
		command.env("NO_COLOR", "1");
	}
	let out = command.output().expect("spawn the test binary");
	assert!(
		out.status.success(),
		"child failed for {case}: {}",
		String::from_utf8_lossy(&out.stderr)
	);
	let stdout = String::from_utf8(out.stdout).expect("utf-8 output");
	let (_, rest) = stdout
		.split_once("@@begin@@\n")
		.unwrap_or_else(|| panic!("child rendered nothing for {case}: {stdout:?}"));
	let (frame, _) = rest
		.split_once("@@end@@")
		.unwrap_or_else(|| panic!("child did not close the frame for {case}: {rest:?}"));
	frame.to_string()
}

/// The frame as a reader sees it: SGR sequences dropped, so glyph and text
/// checks do not depend on where the renderer places a reset.
fn visible(frame: &str) -> String {
	let mut out = String::with_capacity(frame.len());
	let mut chars = frame.chars();
	while let Some(ch) = chars.next() {
		if ch == '\x1b' {
			for next in chars.by_ref() {
				if next == 'm' {
					break;
				}
			}
		} else {
			out.push(ch);
		}
	}
	out
}

/// Frame glyphs are counted on the visible text: with colour on, a reset sits
/// between the glyph and the space, so `"╭ "` is only contiguous in plain output.
fn assert_one_frame(frame: &str) {
	let plain = visible(frame);
	assert_eq!(plain.matches('╭').count(), 1, "more than one opening frame: {plain}");
	assert_eq!(plain.matches('╰').count(), 1, "more than one closing frame: {plain}");
	assert!(
		!plain.contains("│ ╭"),
		"the frame is nested inside a plugin frame: {plain}"
	);
}

#[test]
fn besydr_list_is_one_frame_with_semantic_colors() {
	let frame = render("list", true);
	assert_one_frame(&frame);
	let plain = visible(&frame);
	assert!(plain.contains("╭ /learning · developer/project"), "{plain}");
	assert!(plain.contains("│ #1 [high] (learning)"), "{plain}");
	assert!(plain.contains("│   Reuse the captured answer"), "{plain}");
	assert!(
		plain.contains("│   confidence: high | tags: display | created: 2026-11-20"),
		"{plain}"
	);
	assert!(plain.contains("╰ ✓ /learning · 1 lesson(s)"), "{plain}");

	// Title cyan, separator dim, subtitle bright white.
	assert!(
		frame.contains("\x1b[96m╭\x1b[0m \x1b[96m/learning\x1b[0m \x1b[90m·\x1b[0m \x1b[97mdeveloper/project\x1b[0m"),
		"{frame:?}"
	);
	// Importance, content and metadata each keep their own style and are not
	// swallowed by the cyan section header they sit in.
	assert!(frame.contains("\x1b[93m[high]\x1b[0m"), "{frame:?}");
	assert!(frame.contains("\x1b[97mReuse the captured answer\x1b[0m"), "{frame:?}");
	assert!(
		frame.contains("\x1b[2mconfidence: high | tags: display | created: 2026-11-20\x1b[0m"),
		"{frame:?}"
	);
	// Close line: cyan corner, green tick, cyan command, dim separator.
	assert!(
		frame.contains("\x1b[96m╰\x1b[0m \x1b[92m✓\x1b[0m \x1b[96m/learning\x1b[0m \x1b[90m·\x1b[0m 1 lesson(s)"),
		"{frame:?}"
	);
}

#[test]
fn besydr_list_without_color_is_plain() {
	let frame = render("list", false);
	assert!(!frame.contains('\x1b'), "colour was suppressed but escaped: {frame:?}");
	assert_one_frame(&frame);
	assert!(frame.contains("│   hot: 1 item(s) / 40 tok"), "{frame}");
	assert!(frame.contains("│ #1 [high] (learning)"), "{frame}");
	assert!(frame.contains("│   Reuse the captured answer"), "{frame}");
}

#[test]
fn error_closes_red() {
	let frame = render("error", true);
	let plain = visible(&frame);
	assert_eq!(plain.matches('╭').count(), 1, "{plain}");
	assert!(plain.contains("╰ ✗ /learning · no such lesson"), "{plain}");
	assert!(
		frame.contains("\x1b[91m✗\x1b[0m \x1b[91m/learning\x1b[0m \x1b[90m·\x1b[0m \x1b[97mno such lesson\x1b[0m"),
		"{frame:?}"
	);
}

#[test]
fn evolution_keeps_its_own_title() {
	let frame = render("evolution", true);
	assert_one_frame(&frame);
	let plain = visible(&frame);
	assert!(plain.contains("╭ /learning evolution · p/d"), "{plain}");
	assert!(plain.contains("│ ship it · rule · pending"), "{plain}");
	assert!(plain.contains("╰ ✓ /learning evolution · 1 artifact(s)"), "{plain}");
	assert!(
		frame.contains("\x1b[96m/learning evolution\x1b[0m \x1b[90m·\x1b[0m \x1b[97mp/d\x1b[0m"),
		"{frame:?}"
	);
}

#[test]
fn single_record_answers_are_one_frame() {
	for (case, expected) in [
		("show", "╭ /learning · #5 · learning · Title"),
		("delete", "deleted  #5: Reuse the…"),
		("clear", "cleared   3 lesson(s)"),
	] {
		let frame = render(case, false);
		assert_one_frame(&frame);
		assert!(frame.contains(expected), "{case}: {frame}");
	}
}

/// An answer whose subcommand this build does not know cannot be laid out: the
/// plugin's own text is shown in a frame (once), as before.
#[test]
fn unknown_subcommand_keeps_the_plugin_frame() {
	let frame = render("unknown", true);
	let plain = visible(&frame);
	assert_eq!(plain.matches('╭').count(), 2, "{plain}");
	assert_eq!(plain.matches('╰').count(), 2, "{plain}");
	assert!(plain.contains("╭ /learning · besydr"), "{plain}");
	assert!(plain.contains("│ ╭ /learning · developer/project"), "{plain}");
	assert!(
		frame.contains("\x1b[96m/learning\x1b[0m \x1b[90m·\x1b[0m \x1b[97mbesydr\x1b[0m"),
		"{frame:?}"
	);
}

/// A text-only answer carries `{"text": ..}` and no subcommand: fallback, not
/// a half-filled layout.
#[test]
fn text_only_answer_falls_back_to_the_plugin_frame() {
	let frame = render("text-only", false);
	let plain = visible(&frame);
	assert_eq!(plain.matches('╭').count(), 2, "{plain}");
	assert!(plain.contains("│ ╭ /learning · developer/project"), "{plain}");
	assert!(plain.contains("╭ /learning · besydr"), "{plain}");
}

/// Another plugin answering `/learning list` with a payload that happens to use
/// the same keys is not Besydr: its text is shown, the Besydr layout is not.
#[test]
fn another_plugins_learning_list_keeps_its_own_text() {
	let frame = render("other-plugin", false);
	assert_one_frame(&frame);
	assert!(frame.contains("╭ /learning · acme"), "{frame}");
	assert!(frame.contains("│ acme: 1 lesson"), "{frame}");
	assert!(frame.contains("│ run /learning show 1"), "{frame}");
	assert!(!frame.contains("developer/project"), "Besydr layout leaked: {frame}");
	assert!(!frame.contains("hot: "), "Besydr storage summary leaked: {frame}");
	assert!(!frame.contains("[high]"), "Besydr lesson row leaked: {frame}");
}

#[test]
fn other_commands_keep_the_plugin_frame() {
	let frame = render("other-command", false);
	let plain = visible(&frame);
	assert_eq!(plain.matches('╭').count(), 2, "{plain}");
	assert!(plain.contains("╭ /forecast · besydr"), "{plain}");
}
