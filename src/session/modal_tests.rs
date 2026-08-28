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
fn test_show_overlay_invokes_content_closure() {
	let invoked = std::cell::Cell::new(false);
	show_overlay(3, || {
		invoked.set(true);
	});
	assert!(invoked.get());
}

#[test]
fn test_show_overlay_zero_lines_does_not_panic() {
	show_overlay(0, || {});
}

#[test]
fn test_clear_overlay_various_line_counts_does_not_panic() {
	clear_overlay(1);
	clear_overlay(10);
}

#[test]
fn test_overlay_show_then_clear_roundtrip() {
	// Show and clear must accept matching sizes without panicking
	show_overlay(4, || {
		println!("overlay line");
	});
	clear_overlay(4);
}
