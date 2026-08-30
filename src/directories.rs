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

// Directory utilities for cross-platform data directory management

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Get the system-wide data directory for octomind
///
/// `OCTOMIND_DATA_DIR` overrides the location of every piece of state (config,
/// sessions, logs, cache). This is the only portable way to sandbox octomind:
/// on Windows `dirs` resolves Known Folders, so redirecting `HOME` isolates
/// nothing.
///
/// Otherwise the location depends on the OS:
/// - macOS: ~/.local/share/octomind
/// - Linux: ~/.local/share/octomind (following XDG Base Directory specification)
/// - Windows: %LOCALAPPDATA%/octomind
pub fn get_octomind_data_dir() -> Result<PathBuf> {
	let data_dir = match std::env::var_os("OCTOMIND_DATA_DIR") {
		Some(dir) => PathBuf::from(dir),
		None => match dirs::home_dir() {
			Some(home) => {
				#[cfg(target_os = "windows")]
				let path = {
					// On Windows, use %LOCALAPPDATA%/octomind
					match dirs::data_local_dir() {
						Some(dir) => dir.join("octomind"),
						None => home.join("AppData").join("Local").join("octomind"),
					}
				};

				#[cfg(not(target_os = "windows"))]
				let path = home.join(".local").join("share").join("octomind");

				path
			}
			None => {
				return Err(anyhow::anyhow!("Unable to determine home directory"));
			}
		},
	};

	// Ensure the directory exists
	if !data_dir.exists() {
		fs::create_dir_all(&data_dir).context(format!(
			"Failed to create octomind data directory: {}",
			data_dir.display()
		))?;
	}

	Ok(data_dir)
}

/// Get the configuration directory path
pub fn get_config_dir() -> Result<PathBuf> {
	let data_dir = get_octomind_data_dir()?;
	let config_dir = data_dir.join("config");

	if !config_dir.exists() {
		fs::create_dir_all(&config_dir)?;
	}

	Ok(config_dir)
}

/// Get the sessions directory path
pub fn get_sessions_dir() -> Result<PathBuf> {
	let data_dir = get_octomind_data_dir()?;
	let sessions_dir = data_dir.join("sessions");

	if !sessions_dir.exists() {
		fs::create_dir_all(&sessions_dir)?;
	}

	Ok(sessions_dir)
}

/// Get the run directory path — holds per-session Unix socket and PID files.
///
/// Runtime state is host-local: Unix sockets can't be bound on NFS and PIDs are
/// meaningless across hosts, so this lives in system runtime/temp storage
/// (wiped on reboot), never in the data dir:
/// - `$XDG_RUNTIME_DIR/octomind` when set (per-user tmpfs, cleaned on logout)
/// - otherwise `<system tmp>/octomind-<uid>` (macOS `$TMPDIR` is already
///   per-user; the uid suffix disambiguates a shared `/tmp` on Linux)
pub fn get_run_dir() -> Result<PathBuf> {
	let run_dir = runtime_base_dir();
	ensure_private_dir(&run_dir)?;

	// Run files used to live in the data dir, where nothing wiped them after a
	// crash or reboot; clear the legacy location so stale sockets don't linger.
	let legacy = get_octomind_data_dir()?.join("run");
	if legacy.exists() {
		let _ = fs::remove_dir_all(&legacy);
	}

	Ok(run_dir)
}
/// Longest `sun_path` on supported Unix platforms, NUL included — macOS is
/// the strictest at 104 bytes (Linux allows 108).
#[cfg(unix)]
const SUN_PATH_MAX: usize = 104;

/// Path of a session's inject socket: `<run_dir>/<stem>.sock`.
///
/// `stem` is the session name when the full path fits `sun_path`; otherwise the
/// name is cut to fit and suffixed with 8 hex chars of its SHA-256 so the path
/// stays unique and stable. macOS `$TMPDIR` run dirs are long enough that full
/// session names overflow the limit and `bind` fails with "path must be shorter
/// than SUN_LEN". `octomind send` derives the same path, so both ends meet.
#[cfg(unix)]
pub fn session_socket_path(session_name: &str) -> Result<PathBuf> {
	use sha2::{Digest, Sha256};

	let run_dir = get_run_dir()?;
	// "+2" covers the '/' between dir and file and the NUL terminator.
	let budget = SUN_PATH_MAX
		.checked_sub(run_dir.as_os_str().len() + ".sock".len() + 2)
		.ok_or_else(|| {
			anyhow::anyhow!(
				"run dir {} is too long for a Unix socket path",
				run_dir.display()
			)
		})?;
	// Shortest possible shortened stem is "-<8 hex>" (9 bytes).
	if budget < 9 {
		anyhow::bail!(
			"run dir {} leaves too little room for a Unix socket path",
			run_dir.display()
		);
	}

	let stem = if session_name.len() <= budget {
		session_name.to_owned()
	} else {
		let digest = Sha256::digest(session_name.as_bytes());
		let hash: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
		let keep = budget - hash.len() - 1;
		// Cut on a char boundary — names are ASCII today, but don't panic if not.
		let keep = session_name
			.char_indices()
			.take_while(|(i, _)| *i <= keep)
			.last()
			.map(|(i, _)| i)
			.unwrap_or(0);
		format!("{}-{hash}", &session_name[..keep])
	};

	Ok(run_dir.join(format!("{stem}.sock")))
}

#[cfg(unix)]
fn runtime_base_dir() -> PathBuf {
	match std::env::var_os("XDG_RUNTIME_DIR") {
		Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("octomind"),
		_ => {
			let uid = unsafe { libc::getuid() };
			std::env::temp_dir().join(format!("octomind-{uid}"))
		}
	}
}

#[cfg(not(unix))]
fn runtime_base_dir() -> PathBuf {
	std::env::temp_dir().join("octomind")
}

/// Create `dir` with mode 0700, or validate a pre-existing one. A dir in shared
/// tmp can be squatted by another user: reject symlinks and foreign owners, and
/// repair permissions that leak to other users.
#[cfg(unix)]
fn ensure_private_dir(dir: &Path) -> Result<()> {
	use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

	match fs::DirBuilder::new().mode(0o700).create(dir) {
		Ok(()) => Ok(()),
		Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
			let meta = fs::symlink_metadata(dir)?;
			if !meta.is_dir() {
				anyhow::bail!("run dir {} exists but is not a directory", dir.display());
			}
			let uid = unsafe { libc::getuid() };
			if meta.uid() != uid {
				anyhow::bail!(
					"run dir {} is owned by uid {} (expected {}) — refusing to use it",
					dir.display(),
					meta.uid(),
					uid
				);
			}
			if meta.permissions().mode() & 0o077 != 0 {
				fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
			}
			Ok(())
		}
		Err(e) => Err(e).context(format!("failed to create run dir {}", dir.display())),
	}
}

#[cfg(not(unix))]
fn ensure_private_dir(dir: &Path) -> Result<()> {
	fs::create_dir_all(dir)?;
	Ok(())
}

/// Get the logs directory path
pub fn get_logs_dir() -> Result<PathBuf> {
	let data_dir = get_octomind_data_dir()?;
	let logs_dir = data_dir.join("logs");

	if !logs_dir.exists() {
		fs::create_dir_all(&logs_dir)?;
	}

	Ok(logs_dir)
}

/// Get the cache directory path
pub fn get_cache_dir() -> Result<PathBuf> {
	let data_dir = get_octomind_data_dir()?;
	let cache_dir = data_dir.join("cache");

	if !cache_dir.exists() {
		fs::create_dir_all(&cache_dir)?;
	}

	Ok(cache_dir)
}

/// Get the learning directory for a project and role.
/// Structure: `learning/{project}/{role_base}/` — project-first because learning
/// is project-scoped, role is a secondary filter.
/// Role uses only the base part before `:` (e.g. "developer" from "developer:general"),
/// matching how capabilities are sent to MCP servers.
pub fn get_learning_dir(role: &str, project: &str) -> Result<PathBuf> {
	let data_dir = get_octomind_data_dir()?;
	let role_base = role.split(':').next().unwrap_or(role);
	let learning_dir = data_dir.join("learning").join(project).join(role_base);

	if !learning_dir.exists() {
		fs::create_dir_all(&learning_dir)?;
	}

	Ok(learning_dir)
}

/// Get the global (user-wide) learning directory: `learning/_/`.
/// Holds cross-project, cross-role lessons — durable user preferences that
/// apply everywhere. The `_` sentinel cannot collide with a real project name
/// because project dirs are basenames of working directories.
pub fn get_global_learning_dir() -> Result<PathBuf> {
	let data_dir = get_octomind_data_dir()?;
	let global_dir = data_dir.join("learning").join("_");

	if !global_dir.exists() {
		fs::create_dir_all(&global_dir)?;
	}

	Ok(global_dir)
}

/// Machine-local registry for generated behavior artifacts. Kept beneath the
/// learning authority but outside project checkouts and ordinary recall dirs.
pub fn get_learning_evolution_dir() -> Result<PathBuf> {
	let dir = get_octomind_data_dir()?.join("learning").join(".evolution");
	if !dir.exists() {
		fs::create_dir_all(&dir)?;
	}
	Ok(dir)
}

/// Get the default configuration file path
pub fn get_config_file_path() -> Result<PathBuf> {
	let config_dir = get_config_dir()?;
	Ok(config_dir.join("config.toml"))
}

/// Display information about the data directory locations
pub fn print_directory_info() -> Result<()> {
	println!("Octomind Data Directories:");
	println!("  Data Dir:     {}", get_octomind_data_dir()?.display());
	println!("  Config Dir:   {}", get_config_dir()?.display());
	println!("  Sessions Dir: {}", get_sessions_dir()?.display());
	println!("  Logs Dir:     {}", get_logs_dir()?.display());
	println!("  Cache Dir:    {}", get_cache_dir()?.display());
	println!("  Run Dir:      {}", get_run_dir()?.display());

	Ok(())
}

#[cfg(test)]
#[path = "directories_tests.rs"]
mod tests;
