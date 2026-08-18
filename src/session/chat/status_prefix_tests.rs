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

fn filled_cells(bar: &str) -> usize {
	bar.chars().filter(|c| *c == '▰').count()
}

#[test]
fn test_build_context_bar_fill_levels() {
	assert_eq!(filled_cells(&build_context_bar(0.0)), 0);
	assert_eq!(filled_cells(&build_context_bar(13.8)), 1);
	assert_eq!(filled_cells(&build_context_bar(50.0)), 3);
	assert_eq!(filled_cells(&build_context_bar(100.0)), 5);
	// Never overflows the 5 cells
	assert_eq!(filled_cells(&build_context_bar(250.0)), 5);
	// Always exactly 5 cells total
	let bar = build_context_bar(50.0);
	assert_eq!(bar.chars().filter(|c| *c == '▱').count(), 2);
}

#[test]
fn test_build_status_line() {
	// Nothing to show → empty, caller skips printing
	assert_eq!(build_status_line(0.0, 0, 0, None), "");

	let line = build_status_line(0.48, 542, 1000, Some(0.013));
	assert!(line.contains("$0.48"));
	assert!(line.contains("(+$0.013)"));
	assert!(line.contains("54.2%"));

	// Tiny delta is suppressed
	let line = build_status_line(0.48, 542, 1000, Some(0.00005));
	assert!(!line.contains("+$"));

	// Cost but no threshold → infinity marker instead of a bar
	let line = build_status_line(0.50, 999, 0, None);
	assert!(line.contains("$0.50"));
	assert!(line.contains("∞"));

	// Usage above the threshold clamps at 100%
	let line = build_status_line(0.0, 2000, 1000, None);
	assert!(line.contains("100.0%"));
}

#[test]
fn test_build_status_body_plain() {
	assert_eq!(build_status_body_plain(0.0, 0, 0), "");
	assert_eq!(build_status_body_plain(0.5, 500, 1000), "$0.50 ▰▰▰▱▱ 50.0%");
	assert_eq!(build_status_body_plain(0.5, 0, 0), "$0.50 · ∞");
	assert_eq!(build_status_body_plain(0.0, 138, 1000), "▰▱▱▱▱ 13.8%");
}
