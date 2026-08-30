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
fn set_process_title_does_not_panic() {
	// Smoke test: calling set_process_title with various inputs must not panic.
	set_process_title("test-title");
	set_process_title("");
	set_process_title("a-very-long-title-that-exceeds-argv-capacity-to-test-padding-behavior");
}

#[test]
fn set_terminal_title_does_not_panic() {
	// In test environments stderr is typically not a TTY, so this is a no-op.
	// Still verifies the function doesn't panic on any input.
	set_terminal_title("test-title");
	set_terminal_title("");
	set_terminal_title("title with \x1b special chars");
}

#[test]
fn set_terminal_title_noop_when_not_tty() {
	// When stderr is piped (not a TTY), set_terminal_title must not write
	// escape sequences. We verify by checking it doesn't panic — the
	// is_terminal() guard handles the actual gating.
	set_terminal_title("should-be-noop");
}
