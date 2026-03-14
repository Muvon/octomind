// Copyright 2025 Muvon Un Limited
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

//! ACP-specific error sink.
//!
//! When running as an ACP agent, stderr is used for JSON-RPC protocol.
//! This module provides a dedicated file-based error sink for ACP mode.

use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Global ACP error sink instance.
static ACP_ERROR_SINK: std::sync::OnceLock<Arc<AcpErrorSink>> = std::sync::OnceLock::new();

/// ACP error sink for logging errors to a dedicated file.
///
/// This is used when running in ACP mode where stderr is reserved
/// for JSON-RPC protocol communication.
///
/// Thread-safe and can be accessed globally via [`get_global`].
pub struct AcpErrorSink {
	file: Mutex<Option<std::fs::File>>,
	path: PathBuf,
}

impl AcpErrorSink {
	/// Create a new ACP error sink.
	fn new(path: PathBuf) -> Result<Self> {
		// Ensure parent directory exists
		if let Some(parent) = path.parent() {
			if !parent.exists() {
				std::fs::create_dir_all(parent)
					.with_context(|| format!("Failed to create logs directory: {:?}", parent))?;
			}
		}

		Ok(Self {
			file: Mutex::new(None),
			path,
		})
	}

	/// Initialize the global ACP error sink.
	///
	/// This should be called once at startup when running in ACP mode.
	/// The sink writes to `~/.local/share/octomind/logs/acp-errors.jsonl`.
	pub fn initialize() -> Result<Arc<Self>> {
		let logs_dir = crate::directories::get_logs_dir()?;
		let path = logs_dir.join("acp-errors.jsonl");

		let sink = Arc::new(Self::new(path)?);

		// Try to set as global, but don't fail if already set
		let _ = ACP_ERROR_SINK.set(sink.clone());

		Ok(sink)
	}

	/// Get the global ACP error sink, if initialized.
	pub fn get_global() -> Option<Arc<Self>> {
		ACP_ERROR_SINK.get().cloned()
	}

	/// Log a simple error message (convenience method for macros).
	///
	/// This is a simplified version of `log_error` that doesn't require context.
	pub fn log_error_simple(&self, error: &str) -> Result<()> {
		let timestamp = chrono::Utc::now().to_rfc3339();
		let session_id = std::env::current_dir()
			.ok()
			.and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
			.unwrap_or_else(|| "unknown".to_string());

		let entry = serde_json::json!({
			"timestamp": timestamp,
			"session_id": session_id,
			"error": error
		});

		self.write_entry(&entry)
	}

	/// Write an entry to the log file.
	fn write_entry(&self, entry: &JsonValue) -> Result<()> {
		let mut file_guard = self.file.lock().unwrap();

		if file_guard.is_none() {
			let file = OpenOptions::new()
				.create(true)
				.append(true)
				.open(&self.path)
				.with_context(|| format!("Failed to open ACP error log: {:?}", self.path))?;
			*file_guard = Some(file);
		}

		let file = file_guard.as_mut().unwrap();
		let content =
			serde_json::to_string(entry).with_context(|| "Failed to serialize error entry")?;

		writeln!(file, "{}", content).with_context(|| "Failed to write error entry")?;

		// Flush immediately for errors
		file.flush().with_context(|| "Failed to flush error log")?;

		Ok(())
	}
}

impl Drop for AcpErrorSink {
	fn drop(&mut self) {
		// Ensure file is flushed on drop
		if let Ok(mut file_guard) = self.file.lock() {
			if let Some(file) = file_guard.as_mut() {
				let _ = file.flush();
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::tempdir;

	#[test]
	fn test_acp_error_sink_path() {
		let dir = tempdir().unwrap();
		let logs_dir = dir.path().join("logs");
		std::fs::create_dir_all(&logs_dir).unwrap();

		let path = logs_dir.join("acp-errors.jsonl");
		let sink = AcpErrorSink::new(path.clone()).unwrap();

		sink.log_error_simple("Test error").unwrap();

		let content = std::fs::read_to_string(&path).unwrap();
		assert!(content.contains("Test error"));
	}
}
