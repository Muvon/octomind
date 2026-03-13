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

//! Filesystem sandbox: restrict all writes to the current working directory.
//!
//! Applies an OS-level write restriction to the current process and all its
//! children (spawned shells, MCP servers, sub-agents). Reads are unrestricted.
//!
//! Platform support:
//! - Linux: Landlock LSM (kernel 5.13+). Gracefully degrades on older kernels.
//! - macOS: Seatbelt via `sandbox_init(3)` C FFI.
//! - Other: no-op with a warning log.

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

/// Apply filesystem write sandbox, locking all writes to `dir` and its subtree.
///
/// Must be called early — before spawning any child processes — so the
/// restriction is inherited by all children automatically.
pub fn apply(dir: &std::path::Path) -> anyhow::Result<()> {
	#[cfg(target_os = "linux")]
	{
		linux::apply(dir)
	}

	#[cfg(target_os = "macos")]
	{
		macos::apply(dir)
	}

	#[cfg(not(any(target_os = "linux", target_os = "macos")))]
	{
		let _ = dir;
		crate::log_info!("Sandbox requested but not supported on this platform — running without write restrictions.");
		Ok(())
	}
}
