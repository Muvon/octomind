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

#[test]
fn icon_constants_match_the_asset() {
	assert_eq!(ICON_WIDTH, 7);
	assert_eq!(ICON_ROWS.len(), 4);
}

#[test]
fn icon_lines_renders_four_ansi_rows() {
	let lines = icon_lines();
	assert_eq!(lines.len(), 4);
	for (i, line) in lines.iter().enumerate() {
		assert!(line.contains("\x1b["), "row {i} lacks ANSI codes");
		assert!(line.ends_with("\x1b[0m"), "row {i} must end with a reset");
	}
}

#[test]
fn icon_lines_uses_the_brand_body_color() {
	let all = icon_lines().concat();
	// BODY = (0xA8, 0x55, 0xF7) renders as a truecolor SGR sequence.
	assert!(all.contains("\x1b[38;2;168;85;247m"));
}
