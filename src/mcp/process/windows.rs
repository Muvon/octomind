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

//! Windows process-tree tracking for stdio MCP servers.
//!
//! Each spawned server goes into its own Job Object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Processes the server starts later
//! (`npx` → `node`) inherit the job, so terminating it takes down the whole
//! tree — the Windows counterpart of the Unix process group. Only octomind
//! holds the job handle, so the OS also kills the tree when octomind dies
//! without running cleanup.

use std::collections::HashMap;
use std::sync::RwLock;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
use windows_sys::Win32::System::JobObjects::{
	AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
	TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
	OpenProcess, WaitForSingleObject, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

/// Exit code given to every process in a terminated job (mirrors a SIGKILL).
const KILLED_EXIT_CODE: u32 = 1;

lazy_static::lazy_static! {
	static ref SERVER_JOBS: RwLock<HashMap<String, ServerJob>> = RwLock::new(HashMap::new());
}

/// A server's Job Object plus a handle to the server process itself. Both are
/// closed on drop; closing the job handle kills whatever is still inside it.
struct ServerJob {
	job: HANDLE,
	process: HANDLE,
}

// SAFETY: kernel handles are process-wide and valid from any thread.
unsafe impl Send for ServerJob {}
unsafe impl Sync for ServerJob {}

impl ServerJob {
	// The server is assigned right after spawn instead of being created
	// suspended: std/tokio expose no handle to the primary thread to resume it.
	// Anything the server starts before assignment escapes the job, but that
	// window is the few microseconds between CreateProcess and this call,
	// far shorter than any interpreter or shim (npx, uvx, python) takes to
	// start its first child.
	fn assign(pid: u32) -> std::io::Result<Self> {
		// SAFETY: plain Win32 calls; every handle is checked before use and
		// owned by the returned value (or closed by its Drop on error).
		unsafe {
			let mut entry = Self {
				job: std::ptr::null_mut(),
				process: OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid),
			};
			if entry.process.is_null() {
				return Err(std::io::Error::last_os_error());
			}
			// Null security attributes: the handle is not inheritable, so no
			// child process can keep the job (and its tree) alive.
			entry.job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
			if entry.job.is_null() {
				return Err(std::io::Error::last_os_error());
			}
			let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
			limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
			if SetInformationJobObject(
				entry.job,
				JobObjectExtendedLimitInformation,
				&limits as *const _ as *const std::ffi::c_void,
				std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
			) == 0
			{
				return Err(std::io::Error::last_os_error());
			}
			if AssignProcessToJobObject(entry.job, entry.process) == 0 {
				return Err(std::io::Error::last_os_error());
			}
			Ok(entry)
		}
	}
}

impl Drop for ServerJob {
	fn drop(&mut self) {
		// SAFETY: handles are owned by this value and closed exactly once.
		unsafe {
			if !self.job.is_null() {
				CloseHandle(self.job);
			}
			if !self.process.is_null() {
				CloseHandle(self.process);
			}
		}
	}
}

/// Put a freshly spawned server into its own kill-on-close Job Object.
/// Re-registering a name drops the previous job, killing that old tree —
/// also when the new assignment fails, so liveness never reports a stale
/// process for the new server.
pub(crate) fn register_pgid(server_name: &str, pid: u32) {
	match ServerJob::assign(pid) {
		Ok(job) => {
			SERVER_JOBS.write().unwrap().insert(server_name.to_string(), job);
		}
		Err(e) => {
			SERVER_JOBS.write().unwrap().remove(server_name);
			crate::log_debug!(
				"Could not put MCP server '{}' (pid {}) into a job object: {}",
				server_name,
				pid,
				e
			);
		}
	}
}

/// Check whether the server process (not its children) is still running.
/// Returns `None` when the server was never registered here.
pub(crate) fn is_stdio_process_alive(server_name: &str) -> Option<bool> {
	let jobs = SERVER_JOBS.read().unwrap();
	jobs.get(server_name)
		// SAFETY: the process handle stays open while the entry exists.
		.map(|job| unsafe { WaitForSingleObject(job.process, 0) == WAIT_TIMEOUT })
}

/// Terminate every process in the server's job and forget the job.
pub(crate) fn kill_process_group(server_name: &str) {
	let job = SERVER_JOBS.write().unwrap().remove(server_name);
	if let Some(job) = job {
		terminate(server_name, &job);
	}
}

/// Final shutdown backstop: terminate every tracked job.
pub(crate) fn kill_all_process_groups() {
	let jobs: Vec<(String, ServerJob)> = SERVER_JOBS.write().unwrap().drain().collect();
	for (name, job) in &jobs {
		terminate(name, job);
	}
}

fn terminate(server_name: &str, job: &ServerJob) {
	// SAFETY: the job handle is open for the lifetime of `job`.
	if unsafe { TerminateJobObject(job.job, KILLED_EXIT_CODE) } == 0 {
		crate::log_debug!(
			"TerminateJobObject failed for server '{}': {}",
			server_name,
			std::io::Error::last_os_error()
		);
	} else {
		crate::log_debug!("Terminated job object for server '{}'", server_name);
	}
}
