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

//! Working directory management for multi-session concurrency.
//!
//! Each session gets its own isolated working directory state.
//! For WebSocket sessions, state is stored in session-keyed registries.
//! For CLI mode, thread-local storage is used as fallback.
//!
//! # Architecture
//!
//! Before: Thread-local WORKDIR (single session per process)
//! After: Session-keyed registry + task-local session ID propagation.
//!
//! The session-scoped lookup pattern: `get_thread_working_directory` first
//! reads `current_session_id` (task-local) and looks up
//! `crate::session::context::get_current_workdir(&id)`; on miss it falls
//! back to the per-thread `WORKDIR` cell.

use std::path::PathBuf;

/// Thread-local storage for CLI mode (single session).
struct WorkDir {
	session: PathBuf,
	current: PathBuf,
}

thread_local! {
	static WORKDIR: std::cell::RefCell<Option<WorkDir>> = const { std::cell::RefCell::new(None) };
}

/// Set the session working directory. Call at every session boundary.
/// Resets both the active directory and the reset anchor to `path`.
///
/// For WebSocket sessions, stores in session-scoped registry.
/// For CLI mode, stores in thread-local storage.
pub fn set_session_working_directory(path: PathBuf) {
	// Try session-scoped first (WebSocket mode)
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::set_session_workdir(&session_id, path);
		return;
	}
	// Fall back to thread-local (CLI mode)
	WORKDIR.with(|w| {
		*w.borrow_mut() = Some(WorkDir {
			session: path.clone(),
			current: path,
		});
	});
}

/// Override the active directory mid-session (workdir tool). Does not move the reset anchor.
///
/// For WebSocket sessions, updates session-scoped registry.
/// For CLI mode, updates thread-local storage.
pub fn set_thread_working_directory(path: PathBuf) {
	// Try session-scoped first (WebSocket mode)
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::set_current_workdir(&session_id, path);
		return;
	}
	// Fall back to thread-local (CLI mode)
	WORKDIR.with(|w| {
		let mut w = w.borrow_mut();
		if let Some(ref mut wd) = *w {
			wd.current = path;
		}
	});
}

/// Active working directory for the current session/thread.
///
/// For WebSocket sessions, returns from session-scoped registry.
/// For CLI mode, returns from thread-local storage.
/// Falls back to current_dir() if neither is set.
pub fn get_thread_working_directory() -> PathBuf {
	// Try session-scoped first (WebSocket mode)
	if let Some(session_id) = crate::session::context::current_session_id() {
		if let Some(path) = crate::session::context::get_current_workdir(&session_id) {
			return path;
		}
	}
	// Fall back to thread-local (CLI mode)
	WORKDIR.with(|w| {
		w.borrow()
			.as_ref()
			.map(|wd| wd.current.clone())
			.unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
	})
}

/// Session anchor — the directory to return to on workdir reset.
///
/// For WebSocket sessions, returns from session-scoped registry.
/// For CLI mode, returns from thread-local storage.
/// Falls back to current_dir() if neither is set.
pub fn get_thread_original_working_directory() -> PathBuf {
	// Try session-scoped first (WebSocket mode)
	if let Some(session_id) = crate::session::context::current_session_id() {
		if let Some(path) = crate::session::context::get_session_workdir_anchor(&session_id) {
			return path;
		}
	}
	// Fall back to thread-local (CLI mode)
	WORKDIR.with(|w| {
		w.borrow()
			.as_ref()
			.map(|wd| wd.session.clone())
			.unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	// Each #[test] runs on a fresh thread, so the `thread_local!(WORKDIR)`
	// cell is independently empty per test. No cross-test interference
	// even under parallel execution.

	#[test]
	fn set_session_then_get_returns_that_path_in_thread_local_mode() {
		let p = PathBuf::from("/tmp/octomind-test-session");
		set_session_working_directory(p.clone());
		assert_eq!(get_thread_working_directory(), p);
		assert_eq!(get_thread_original_working_directory(), p);
	}

	#[test]
	fn override_active_does_not_move_session_anchor() {
		let session = PathBuf::from("/tmp/octomind-test-anchor");
		let active = PathBuf::from("/tmp/octomind-test-active");
		set_session_working_directory(session.clone());
		set_thread_working_directory(active.clone());

		assert_eq!(get_thread_working_directory(), active);
		assert_eq!(get_thread_original_working_directory(), session);
	}

	#[test]
	fn unset_workdir_falls_back_to_current_dir() {
		let cwd = std::env::current_dir().unwrap_or_default();
		assert_eq!(get_thread_working_directory(), cwd);
		assert_eq!(get_thread_original_working_directory(), cwd);
	}

	#[test]
	fn override_without_prior_session_does_not_materialize_record() {
		// `set_thread_working_directory` only mutates `current` when a
		// WorkDir already exists. Without a prior `set_session_working_directory`,
		// the call is a no-op — anchor stays at the process cwd.
		let cwd = std::env::current_dir().unwrap_or_default();
		set_thread_working_directory(PathBuf::from("/tmp/should-be-ignored"));
		assert_eq!(get_thread_original_working_directory(), cwd);
	}
}
