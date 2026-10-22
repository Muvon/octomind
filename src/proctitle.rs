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

//! Set the OS process title and the terminal/console window title.
//!
//! Process title is implemented directly via libc (no external crate):
//!   - macOS: overwrite the argv block obtained from `_NSGetArgv()`. Visible
//!     in `ps -ax`, Activity Monitor, htop.
//!   - Linux: `prctl(PR_SET_NAME)` for the short comm name (visible in htop
//!     and the kernel-tracked process name) plus argv[0] overwrite via
//!     glibc's `program_invocation_name`. Visible in `ps`/htop.
//!   - Other Unix / Windows: process title is a no-op. Windows Task Manager
//!     shows the executable filename and there is no Win32 API to change it
//!     at runtime — use `set_terminal_title()` to differentiate console tabs.
//!
//! Terminal title (OSC 0 escape on stderr) is cross-platform: xterm family,
//! macOS Terminal, iTerm2, Windows Terminal, Win10+ ConPTY.

use std::io::{IsTerminal, Write};

#[cfg(unix)]
use std::sync::OnceLock;

#[cfg(unix)]
struct ArgvSpan {
	start: *mut libc::c_char,
	capacity: usize,
	/// Strings a reader takes from the span before it stops, so the title is
	/// followed by enough of them to keep it out of the environment that
	/// directly follows argv in memory (see [`fill_argv_area`]).
	strings: usize,
}

// Safety: the argv block lives in process-static memory for the lifetime of
// the process, and we only ever write into it from a single point.
#[cfg(unix)]
unsafe impl Send for ArgvSpan {}
#[cfg(unix)]
unsafe impl Sync for ArgvSpan {}

#[cfg(unix)]
static ARGV_SPAN: OnceLock<Option<ArgvSpan>> = OnceLock::new();

#[cfg(target_os = "macos")]
extern "C" {
	fn _NSGetArgv() -> *mut *mut *mut libc::c_char;
	fn _NSGetArgc() -> *mut libc::c_int;
}

#[cfg(target_os = "macos")]
unsafe fn capture_argv_span() -> Option<ArgvSpan> {
	let argv_ptr = _NSGetArgv();
	let argc_ptr = _NSGetArgc();
	if argv_ptr.is_null() || argc_ptr.is_null() {
		return None;
	}
	let argv = *argv_ptr;
	let argc = *argc_ptr as isize;
	if argv.is_null() || argc <= 0 {
		return None;
	}
	let first = *argv;
	if first.is_null() {
		return None;
	}
	// argv strings are contiguous in memory on macOS — span from argv[0] to
	// end of argv[argc-1] (inclusive of its NUL terminator).
	let last = *argv.offset(argc - 1);
	if last.is_null() {
		return None;
	}
	let last_len = libc::strlen(last);
	let end = last.add(last_len + 1);
	let capacity = end.offset_from(first) as usize;
	// `ps` and `pgrep` (via KERN_PROCARGS2) print `argc` strings starting at
	// argv[0]; the environment strings follow argv[argc-1] directly.
	Some(ArgvSpan {
		start: first,
		capacity,
		strings: argc as usize,
	})
}

#[cfg(target_os = "linux")]
unsafe fn capture_argv_span() -> Option<ArgvSpan> {
	extern "C" {
		static mut program_invocation_name: *mut libc::c_char;
	}
	let arg0 = program_invocation_name;
	if arg0.is_null() {
		return None;
	}
	// Use the cmdline length as the available capacity. /proc/self/cmdline is
	// the original argv block joined by NULs.
	let capacity = std::fs::read("/proc/self/cmdline")
		.map(|v| v.len())
		.unwrap_or_else(|_| libc::strlen(arg0));
	// /proc/PID/cmdline reads arg_start..arg_end and continues into the
	// environment only when the last byte of that area is not NUL (the
	// setproctitle convention) — the title's own NUL keeps it inside argv.
	Some(ArgvSpan {
		start: arg0,
		capacity,
		strings: 1,
	})
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
unsafe fn capture_argv_span() -> Option<ArgvSpan> {
	None
}

#[cfg(unix)]
pub fn set_process_title(title: &str) {
	// An empty title has no non-empty string to lead the argv area, so macOS
	// `ps` and `pgrep` would skip it and read on into the environment. Keep
	// whatever title the process shows now.
	if title.is_empty() {
		return;
	}
	// Linux: also push the short comm name (visible in htop, /proc/PID/comm).
	#[cfg(target_os = "linux")]
	unsafe {
		const PR_SET_NAME: libc::c_int = 15;
		let bytes = title.as_bytes();
		let len = bytes.len().min(15);
		let mut buf = [0u8; 16];
		buf[..len].copy_from_slice(&bytes[..len]);
		libc::prctl(PR_SET_NAME, buf.as_ptr() as libc::c_ulong, 0u64, 0u64, 0u64);
	}

	// Overwrite the argv block in place so `ps`/Activity Monitor pick it up.
	let span = ARGV_SPAN.get_or_init(|| unsafe { capture_argv_span() });
	if let Some(span) = span {
		// Safety: the span is the process's own argv block, captured once at
		// first use and never freed; it is only ever written from here.
		let area = unsafe { std::slice::from_raw_parts_mut(span.start as *mut u8, span.capacity) };
		fill_argv_area(area, title.as_bytes(), span.strings);
	}
}

/// Write `title` at the start of the argv `area`, followed by `strings - 1`
/// one-space strings, and NUL-fill the rest. The title is truncated so all of
/// them fit inside the area.
///
/// The environment strings sit directly after argv in memory, and macOS
/// readers walk a fixed number of strings (`argc`): `ps` counts empty ones,
/// `pgrep` skips them. With fewer non-empty strings than that inside the area,
/// `pgrep -fl` runs past argv and prints environment strings — API keys
/// included — after the title. Leftover argv text is cleared too, so it never
/// shows up after the title.
#[cfg(unix)]
fn fill_argv_area(area: &mut [u8], title: &[u8], strings: usize) {
	let padding = strings.saturating_sub(1);
	// Title NUL, then a space and its NUL per padding string.
	let copy_len = title.len().min(area.len().saturating_sub(1 + 2 * padding));
	area[..copy_len].copy_from_slice(&title[..copy_len]);
	area[copy_len..].fill(0);
	if let Some(rest) = area.get_mut(copy_len + 1..) {
		for string in rest.as_chunks_mut::<2>().0.iter_mut().take(padding) {
			string[0] = b' ';
		}
	}
}

#[cfg(not(unix))]
pub fn set_process_title(_title: &str) {
	// Windows: process name is the .exe filename and cannot be changed at runtime.
}

/// Emit an OSC 0 escape on stderr to set the terminal window/tab title.
/// Self-gated on stderr being a TTY so pipes don't see escape garbage.
pub fn set_terminal_title(title: &str) {
	let mut stderr = std::io::stderr();
	if !stderr.is_terminal() {
		return;
	}
	let _ = write!(stderr, "\x1b]0;{title}\x07");
	let _ = stderr.flush();
}

#[cfg(test)]
#[path = "proctitle_tests.rs"]
mod tests;
