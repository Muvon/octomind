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

//! Suppress the tty driver's keypress echo for the lifetime of an
//! interactive session — covers both `^C` on SIGINT and stray `\n` from
//! Enter presses while the spinner is running.
//!
//! Why this is needed: in cooked-mode terminal, the tty driver echoes typed
//! characters to stdout before passing them to the program. indicatif's
//! spinner pads its line to terminal width on each draw, leaving the cursor
//! at the very last column. Any echoed character (the `^` of `^C`, or `\n`
//! from Enter) auto-wraps to a new row, and the next indicatif redraw
//! clears the wrong row — leaving the actual spinner content stranded in
//! scrollback above.
//!
//! We clear both `ECHO` and `ECHOCTL`. Reedline puts the tty in raw mode
//! during `read_line` and draws every typed character itself, so disabling
//! the tty's own echo doesn't affect what the user sees at the prompt.

#[cfg(unix)]
pub struct CtrlCEchoGuard {
	fd: libc::c_int,
	saved_lflag: libc::tcflag_t,
}

#[cfg(unix)]
impl CtrlCEchoGuard {
	/// Clear `ECHO` and `ECHOCTL` on stdin and return a guard that restores
	/// the originals on Drop. Returns `None` if stdin isn't a tty or the
	/// tcgetattr / tcsetattr calls fail.
	pub fn install() -> Option<Self> {
		Self::install_on(libc::STDIN_FILENO)
	}

	/// Same as [`install`] but operates on an arbitrary tty fd. Use this
	/// when stdin isn't the controlling terminal — e.g. piped input — and
	/// `STDERR_FILENO` / `STDOUT_FILENO` still points at the tty.
	pub fn install_on(fd: libc::c_int) -> Option<Self> {
		// SAFETY: passing a valid stack pointer to libc::tcgetattr.
		let mut termios: libc::termios = unsafe { std::mem::zeroed() };
		if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
			return None;
		}
		let saved_lflag = termios.c_lflag;
		termios.c_lflag &= !(libc::ECHO | libc::ECHOCTL);
		// SAFETY: termios is fully initialized by tcgetattr above.
		if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
			return None;
		}
		Some(Self { fd, saved_lflag })
	}
}

#[cfg(unix)]
impl Drop for CtrlCEchoGuard {
	fn drop(&mut self) {
		// SAFETY: stack-allocated termios passed to libc.
		let mut termios: libc::termios = unsafe { std::mem::zeroed() };
		if unsafe { libc::tcgetattr(self.fd, &mut termios) } == 0 {
			termios.c_lflag = self.saved_lflag;
			unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &termios) };
		}
	}
}

#[cfg(not(unix))]
pub struct CtrlCEchoGuard;

#[cfg(not(unix))]
impl CtrlCEchoGuard {
	pub fn install() -> Option<Self> {
		None
	}
	pub fn install_on(_fd: i32) -> Option<Self> {
		None
	}
}

/// Discard any pending bytes in stdin's input queue. Call right before a
/// new prompt read so keypresses that piled up during animation (Enter,
/// arrow keys, etc.) don't get consumed as the user's actual input.
///
/// We suppress *echoing* those keypresses via `ECHO` in `CtrlCEchoGuard`,
/// but the bytes still buffer in the tty's input queue — without this
/// flush, reedline would pick them up on its next read.
pub fn drain_stdin() {
	drain_fd(0); // STDIN_FILENO
}

/// Like [`drain_stdin`] but for an arbitrary tty fd. On non-tty fds
/// `tcflush` returns ENOTTY which we silently ignore — safe to call with
/// any fd.
pub fn drain_fd(_fd: i32) {
	#[cfg(unix)]
	{
		// SAFETY: calling tcflush with a caller-supplied fd and known constant.
		unsafe {
			libc::tcflush(_fd as libc::c_int, libc::TCIFLUSH);
		}
	}
}
