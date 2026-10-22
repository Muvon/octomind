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

/// Serialises tests that rewrite this process's argv block, so a concurrent
/// `set_process_title` cannot change the area while another test inspects it.
#[cfg(unix)]
static ARGV_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn set_process_title_does_not_panic() {
	#[cfg(unix)]
	let _argv = ARGV_WRITE.lock().unwrap_or_else(|e| e.into_inner());
	// Smoke test: calling set_process_title with various inputs must not panic.
	set_process_title("test-title");
	set_process_title("");
	set_process_title("a-very-long-title-that-exceeds-argv-capacity-to-test-padding-behavior");
}

/// Bytes `ps` shows as the command line, read from `KERN_PROCARGS2` the way
/// macOS `ps`/`pgrep` do: skip `argc` and the exec path, skip the NULs that
/// pad it, then take `argc` NUL-terminated strings. Returns the byte range of
/// those strings inside `procargs`.
#[cfg(target_os = "macos")]
fn ps_command_line(procargs: &[u8]) -> std::ops::Range<usize> {
	let argc = i32::from_ne_bytes(procargs[..4].try_into().unwrap()) as usize;
	let exec_path_end = 4 + procargs[4..].iter().position(|&b| b == 0).unwrap();
	let start = exec_path_end + procargs[exec_path_end..].iter().position(|&b| b != 0).unwrap();
	let mut end = start;
	for _ in 0..argc {
		end += procargs[end..].iter().position(|&b| b == 0).unwrap() + 1;
	}
	start..end
}

#[cfg(target_os = "macos")]
fn own_procargs() -> Vec<u8> {
	let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, std::process::id() as libc::c_int];
	let mut buf = vec![0u8; 1 << 20];
	let mut len = buf.len();
	let rc = unsafe {
		libc::sysctl(
			mib.as_mut_ptr(),
			mib.len() as libc::c_uint,
			buf.as_mut_ptr() as *mut libc::c_void,
			&mut len,
			std::ptr::null_mut(),
			0,
		)
	};
	assert_eq!(rc, 0, "sysctl(KERN_PROCARGS2) failed");
	buf.truncate(len);
	buf
}

#[cfg(target_os = "macos")]
#[test]
fn title_longer_than_argv_keeps_ps_out_of_the_environment() {
	let _argv = ARGV_WRITE.lock().unwrap_or_else(|e| e.into_inner());
	let span = ARGV_SPAN
		.get_or_init(|| unsafe { capture_argv_span() })
		.as_ref()
		.expect("argv span");
	// A title that outgrows argv is truncated; whatever survives must leave
	// `ps` enough NULs to stop before the environment that follows argv.
	set_process_title(&"t".repeat(span.capacity * 2));

	let procargs = own_procargs();
	let shown = ps_command_line(&procargs);
	// Report sizes only: the bytes past argv are this process's environment.
	assert!(
		shown.len() <= span.capacity,
		"ps reads {} bytes past the {}-byte argv area into the environment",
		shown.len() - span.capacity,
		span.capacity
	);
	assert!(procargs[shown].starts_with(b"ttt"), "ps does not show the title first");
}

#[cfg(target_os = "linux")]
#[test]
fn title_longer_than_argv_keeps_cmdline_out_of_the_environment() {
	let _argv = ARGV_WRITE.lock().unwrap_or_else(|e| e.into_inner());
	let span = ARGV_SPAN
		.get_or_init(|| unsafe { capture_argv_span() })
		.as_ref()
		.expect("argv span");
	set_process_title(&"t".repeat(span.capacity * 2));

	// The kernel reads past arg_end into the environment only when the last
	// byte of the argv area is not NUL.
	let cmdline = std::fs::read("/proc/self/cmdline").unwrap();
	assert!(
		cmdline.len() <= span.capacity,
		"/proc/self/cmdline reads {} bytes past the argv area into the environment",
		cmdline.len() - span.capacity
	);
	assert!(cmdline.starts_with(b"ttt"), "cmdline does not show the title first");
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
