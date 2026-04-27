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
	Some(ArgvSpan {
		start: first,
		capacity,
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
	Some(ArgvSpan {
		start: arg0,
		capacity,
	})
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
unsafe fn capture_argv_span() -> Option<ArgvSpan> {
	None
}

#[cfg(unix)]
pub fn set_process_title(title: &str) {
	// Linux: also push the short comm name (visible in htop, /proc/PID/comm).
	#[cfg(target_os = "linux")]
	unsafe {
		const PR_SET_NAME: libc::c_int = 15;
		let bytes = title.as_bytes();
		let len = bytes.len().min(15);
		let mut buf = [0u8; 16];
		buf[..len].copy_from_slice(&bytes[..len]);
		libc::prctl(
			PR_SET_NAME,
			buf.as_ptr() as libc::c_ulong,
			0u64,
			0u64,
			0u64,
		);
	}

	// Overwrite the argv block in place so `ps`/Activity Monitor pick it up.
	let span = ARGV_SPAN.get_or_init(|| unsafe { capture_argv_span() });
	if let Some(span) = span {
		unsafe {
			let title_bytes = title.as_bytes();
			let copy_len = title_bytes.len().min(span.capacity.saturating_sub(1));
			std::ptr::copy_nonoverlapping(title_bytes.as_ptr(), span.start as *mut u8, copy_len);
			// Pad the remainder of the span with NULs so leftover argv text
			// doesn't show up after our title.
			std::ptr::write_bytes(span.start.add(copy_len), 0u8, span.capacity - copy_len);
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
