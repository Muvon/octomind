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

//! Suppress the tty driver's `^C` echo on SIGINT for the lifetime of an
//! interactive session.
//!
//! Why this is needed: when the user presses Ctrl+C in a cooked-mode terminal,
//! the tty driver echoes the literal characters `^C` to stdout at the current
//! cursor position *before* delivering SIGINT. indicatif's spinner pads its
//! line to terminal width on each draw, leaving the cursor at the very last
//! column. The `^` of the echo therefore auto-wraps to a new row, and the
//! subsequent `finish_and_clear` (which clears `bar_count = 1` rows starting
//! from the current cursor row) wipes the wrapped echo line — leaving the
//! actual spinner content stuck in scrollback above.
//!
//! Disabling `ECHOCTL` removes the echo entirely; SIGINT still fires
//! normally and the spinner is cleared from the row it was actually drawn on.

#[cfg(unix)]
pub struct CtrlCEchoGuard {
	saved_lflag: libc::tcflag_t,
}

#[cfg(unix)]
impl CtrlCEchoGuard {
	/// Clear `ECHOCTL` on stdin and return a guard that restores the original
	/// value on Drop. Returns `None` if stdin isn't a tty or the tcgetattr /
	/// tcsetattr calls fail.
	pub fn install() -> Option<Self> {
		let fd = libc::STDIN_FILENO;
		// SAFETY: passing a valid stack pointer to libc::tcgetattr.
		let mut termios: libc::termios = unsafe { std::mem::zeroed() };
		if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
			return None;
		}
		let saved_lflag = termios.c_lflag;
		termios.c_lflag &= !libc::ECHOCTL;
		// SAFETY: termios is fully initialized by tcgetattr above.
		if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
			return None;
		}
		Some(Self { saved_lflag })
	}
}

#[cfg(unix)]
impl Drop for CtrlCEchoGuard {
	fn drop(&mut self) {
		let fd = libc::STDIN_FILENO;
		// SAFETY: stack-allocated termios passed to libc.
		let mut termios: libc::termios = unsafe { std::mem::zeroed() };
		if unsafe { libc::tcgetattr(fd, &mut termios) } == 0 {
			termios.c_lflag = self.saved_lflag;
			unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) };
		}
	}
}

#[cfg(not(unix))]
pub struct CtrlCEchoGuard;

#[cfg(not(unix))]
impl CtrlCEchoGuard {
	pub fn install() -> Option<Self> {
		// Windows console does not echo `^C` for Ctrl+C events; no-op.
		None
	}
}
