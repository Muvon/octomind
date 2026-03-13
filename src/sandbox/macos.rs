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

//! macOS sandbox backend using Apple Seatbelt (`sandbox_init(3)`).
//!
//! `sandbox_init` applies a Scheme-like policy to the current process and all
//! children it spawns. The CLI tool `sandbox-exec` is deprecated but the
//! underlying C library function remains stable across all macOS versions.
//!
//! Policy: allow everything by default, deny all file writes, then re-allow
//! writes only inside the target directory plus essential system paths.

use anyhow::{bail, Result};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};

extern "C" {
	/// Apply a Seatbelt sandbox profile to the current process.
	/// Returns 0 on success, -1 on failure (errorbuf is set).
	fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;

	/// Free the error string allocated by `sandbox_init`.
	fn sandbox_free_error(errorbuf: *mut c_char);
}

pub fn apply(dir: &std::path::Path) -> Result<()> {
	let dir_str = dir
		.to_str()
		.ok_or_else(|| anyhow::anyhow!("sandbox: working directory path is not valid UTF-8"))?;

	// Resolve home dir and XDG data home (~/.local/share) so MCP servers (e.g. octocode)
	// can write their logs/state there without being blocked.
	let home = dirs::home_dir().unwrap_or_default();
	let home_str = home.to_str().unwrap_or_default().to_owned();
	let xdg_data_home = home.join(".local").join("share");
	let xdg_data_home_str = xdg_data_home.to_str().unwrap_or_default().to_owned();

	// Build the Seatbelt profile.
	// - Allow everything by default (reads, network, process ops, etc.)
	// - Deny reads of sensitive credential dirs (ssh keys, cloud creds, gpg keys)
	// - Deny all file writes globally
	// - Re-allow writes to:
	//   • the target working directory (the whole point of the sandbox)
	//   • ~/.local/share — XDG data home; MCP servers write logs/state here
	//   • /dev — pipes, ttys, and other character devices
	//   • /tmp, /private/tmp — standard temp files
	//   • /private/var/folders — macOS per-user temp cache (used by many syscalls)
	let profile = format!(
		r#"(version 1)
(allow default)
(deny file-read* file-write*
  (subpath "{home}/.ssh")
  (subpath "{home}/.gnupg")
  (subpath "{home}/.aws")
  (subpath "{home}/.kube")
  (subpath "{home}/.config/gcloud")
  (subpath "{home}/.azure")
  (subpath "{home}/.config/op")
)
(deny file-write*
  (subpath "/")
)
(allow file-write*
  (subpath "{dir}")
  (subpath "{xdg_data_home}")
  (subpath "/dev")
  (subpath "/tmp")
  (subpath "/private/tmp")
  (subpath "/private/var/folders")
)"#,
		home = home_str,
		dir = dir_str,
		xdg_data_home = xdg_data_home_str,
	);

	let profile_cstr = CString::new(profile)
		.map_err(|_| anyhow::anyhow!("sandbox: profile contains null byte"))?;

	let mut errorbuf: *mut c_char = std::ptr::null_mut();

	let ret = unsafe { sandbox_init(profile_cstr.as_ptr(), 0, &mut errorbuf) };

	if ret != 0 {
		let msg = if errorbuf.is_null() {
			"unknown error".to_string()
		} else {
			let s = unsafe { CStr::from_ptr(errorbuf) }
				.to_string_lossy()
				.into_owned();
			unsafe { sandbox_free_error(errorbuf) };
			s
		};
		bail!("sandbox_init failed: {}", msg);
	}

	crate::log_info!(
		"Sandbox active (Seatbelt): writes locked to {}",
		dir.display()
	);
	Ok(())
}
