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

//! Cancellation management for Octomind sessions
//!
//! This module provides a robust, zero-polling cancellation system using
//! tokio's watch channel for proper async cancellation semantics.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::signal;
use tokio::sync::watch;

/// Typed sentinel for "operation cancelled" results.
///
/// Replaces the previous pattern of `anyhow::bail!("Operation cancelled")` +
/// `e.to_string().contains("Operation cancelled")`. The Display impl preserves
/// the original message text so log output is unchanged, but callers should
/// detect cancellation via [`is_cancelled`] rather than string matching.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("Operation cancelled")]
pub struct Cancelled;

/// Returns true when `err`'s root cause is a [`Cancelled`] marker or octolib's
/// `ProviderError::Cancelled` (raised when a provider request is aborted).
///
/// Walks the anyhow chain so a cancellation wrapped by `.context(...)` further
/// up the stack still matches.
pub fn is_cancelled(err: &anyhow::Error) -> bool {
	err.chain().any(|c| {
		c.is::<Cancelled>()
			|| c.downcast_ref::<octolib::ProviderError>()
				.is_some_and(|p| matches!(p, octolib::ProviderError::Cancelled))
	})
}

/// Manages cancellation state for a session with proper signal handling.
///
/// Uses per-operation watch channels to avoid a race where `reset()` sends
/// `false` before spawned tasks see the `true` — making Ctrl+C invisible.
/// Each `new_operation()` / `reset()` creates a fresh channel; old receivers
/// keep their `true` value forever so orphaned tasks always see cancellation.
pub struct SessionCancellation {
	/// Arc-wrapped sender so the signal handler always sends on the
	/// **current** operation's channel (swapped on each new_operation/reset).
	current_op_tx: Arc<Mutex<watch::Sender<bool>>>,
	/// Receiver for the current operation's channel.
	current_op_rx: watch::Receiver<bool>,
	/// Tracks if we've seen the first Ctrl+C
	first_interrupt: Arc<AtomicBool>,
}

impl Default for SessionCancellation {
	fn default() -> Self {
		Self::new()
	}
}

impl SessionCancellation {
	/// Create a new cancellation manager
	pub fn new() -> Self {
		let (tx, rx) = watch::channel(false);

		Self {
			current_op_tx: Arc::new(Mutex::new(tx)),
			current_op_rx: rx,
			first_interrupt: Arc::new(AtomicBool::new(false)),
		}
	}

	/// Get current operation receiver
	pub fn operation_receiver(&self) -> watch::Receiver<bool> {
		self.current_op_rx.clone()
	}

	/// Start signal handling for this session with error recovery
	pub fn start_signal_handler(&self) -> tokio::task::JoinHandle<()> {
		let op_tx = self.current_op_tx.clone();
		let first_interrupt = self.first_interrupt.clone();

		tokio::spawn(async move {
			// Set up signal handlers with error handling
			#[cfg(unix)]
			{
				use signal::unix::{signal, SignalKind};

				let sigint = match signal(SignalKind::interrupt()) {
					Ok(sig) => sig,
					Err(e) => {
						crate::log_error!("Warning: Failed to register SIGINT handler: {}", e);
						return;
					}
				};

				let sigterm = match signal(SignalKind::terminate()) {
					Ok(sig) => sig,
					Err(e) => {
						crate::log_error!("Warning: Failed to register SIGTERM handler: {}", e);
						return;
					}
				};

				let mut sigint = sigint;
				let mut sigterm = sigterm;

				tokio::select! {
					_ = async {
						loop {
							sigint.recv().await;
							if !handle_interrupt(&first_interrupt, &op_tx) {
								break;
							}
						}
					} => {},
					_ = sigterm.recv() => terminate(),
				}
			}

			#[cfg(windows)]
			{
				use signal::windows::{ctrl_break, ctrl_c, ctrl_close};

				let mut ctrl_c = match ctrl_c() {
					Ok(sig) => sig,
					Err(e) => {
						crate::log_error!("Warning: Failed to listen for Ctrl+C: {}", e);
						return;
					}
				};

				let mut ctrl_break = match ctrl_break() {
					Ok(sig) => sig,
					Err(e) => {
						crate::log_error!("Warning: Failed to listen for Ctrl+Break: {}", e);
						return;
					}
				};

				// Closing the console window. Tokio keeps the OS handler thread
				// parked while we clean up, so Windows waits (up to its ~5s
				// deadline) instead of killing the process mid-cleanup.
				let mut ctrl_close = match ctrl_close() {
					Ok(sig) => sig,
					Err(e) => {
						crate::log_error!("Warning: Failed to listen for console close: {}", e);
						return;
					}
				};

				tokio::select! {
					_ = async {
						while ctrl_c.recv().await.is_some() {
							if !handle_interrupt(&first_interrupt, &op_tx) {
								break;
							}
						}
					} => {},
					_ = ctrl_break.recv() => terminate(),
					_ = ctrl_close.recv() => terminate(),
				}
			}
		})
	}

	/// Create a fresh per-operation channel and return the receiver.
	///
	/// Old receivers from previous operations keep their state — if the
	/// previous operation was cancelled, its receivers still read `true`,
	/// so orphaned `tokio::spawn` tasks always see the cancellation.
	pub fn new_operation(&mut self) -> watch::Receiver<bool> {
		let (tx, rx) = watch::channel(false);
		*self.current_op_tx.lock().unwrap() = tx;
		self.current_op_rx = rx.clone();
		rx
	}

	/// Check if the current operation is cancelled
	pub fn is_cancelled(&self) -> bool {
		*self.current_op_rx.borrow()
	}

	/// Wait for cancellation (async)
	pub async fn cancelled(&mut self) {
		// Wait for the value to become true
		while !*self.current_op_rx.borrow() {
			if self.current_op_rx.changed().await.is_err() {
				break;
			}
		}
	}

	/// Reset the cancellation state for the next operation.
	///
	/// Creates a fresh channel so `is_cancelled()` returns `false`.
	/// Old receivers keep their `true` value — no race with spawned tasks.
	pub fn reset(&mut self) {
		self.first_interrupt.store(false, Ordering::SeqCst);
		let (tx, rx) = watch::channel(false);
		*self.current_op_tx.lock().unwrap() = tx;
		self.current_op_rx = rx;
	}

	/// Cancel all operations and shutdown
	pub fn shutdown(&self) {
		let _ = self.current_op_tx.lock().unwrap().send(true);
	}

	/// A cloneable trigger for this manager, for cancelling from another task
	/// (a WebSocket `cancel` frame read while the turn runs). It shares the
	/// swapped sender, so it always hits the operation that is current when it
	/// fires — including one started by `new_operation()` after it was taken.
	pub fn handle(&self) -> CancelHandle {
		CancelHandle(self.current_op_tx.clone())
	}
}

/// See [`SessionCancellation::handle`].
#[derive(Clone)]
pub struct CancelHandle(Arc<Mutex<watch::Sender<bool>>>);

impl CancelHandle {
	/// Cancel the operation that is current right now.
	pub fn cancel(&self) {
		let _ = self.0.lock().unwrap().send(true);
	}

	/// Whether both handles drive the same manager.
	pub fn same_as(&self, other: &CancelHandle) -> bool {
		Arc::ptr_eq(&self.0, &other.0)
	}
}

/// Stop MCP servers and exit on a termination request (SIGTERM, or Ctrl+Break /
/// console close on Windows).
fn terminate() -> ! {
	// stderr: stdout carries the session's data (`--format jsonl` events,
	// plain answers), so a human notice there corrupts the stream.
	eprintln!("\n🛑 Termination signal received - exiting...");
	std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());
	let _ = crate::mcp::server::cleanup_servers();
	std::process::exit(130);
}

/// Handle interrupt signal with double-Ctrl+C detection.
///
/// Takes `Arc<Mutex<watch::Sender<bool>>>` so it always sends on the
/// **current** operation's channel (swapped by `new_operation()`/`reset()`).
fn handle_interrupt(first_interrupt: &Arc<AtomicBool>, op_tx: &Arc<Mutex<watch::Sender<bool>>>) -> bool {
	if first_interrupt.load(Ordering::SeqCst) {
		// Second Ctrl+C - force exit (always visible)
		// Use std::eprintln! directly to avoid with_suspended_spinner which could deadlock
		// with the animation task's indicatif lock. stderr, not stdout: stdout carries the
		// session's data (`--format jsonl` events, plain answers).
		std::eprintln!("\n\u{1f6d1} Forcing exit...");
		std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());
		let _ = crate::mcp::server::cleanup_servers();
		std::process::exit(130);
	} else {
		// First Ctrl+C — send cancellation signal IMMEDIATELY before any IO.
		// log_debug!/println! use with_suspended_spinner which acquires the spinner
		// mutex and then indicatif's internal BarState lock via suspend().  If the
		// steady-tick thread or animation task holds BarState at that moment, the
		// signal handler blocks and cancel_tx.send() never fires — making Ctrl+C
		// appear completely unresponsive.
		first_interrupt.store(true, Ordering::SeqCst);
		let _ = op_tx.lock().unwrap().send(true);

		// Now safe to log — even if this blocks briefly, cancellation is already sent.
		// Telemetry sits below the send for the same reason: it must never be on
		// the path that makes Ctrl+C feel unresponsive.
		crate::telemetry::record_cancel();
		crate::log_debug!("Ctrl+C: Interrupting current operation...");
		crate::log_debug!("Press Ctrl+C again to force exit");
		std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());

		true // Continue handling
	}
}

#[cfg(test)]
#[path = "cancellation_tests.rs"]
mod tests;
