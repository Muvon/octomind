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

//! Regression tests for the spinner-aware CLI tracing writer.
//!
//! A tracing event emitted while the working spinner is live used to write
//! straight to stderr, desynchronizing indicatif's line accounting and
//! leaving a stale bar above later output (e.g. `· Supervisor:` notices).
//! The writer must therefore route every write through
//! `with_suspended_spinner` — clear before, redraw after.

use super::*;
use indicatif::{ProgressBar, ProgressDrawTarget, TermLike};
use serial_test::serial;
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter as _;

#[test]
fn cli_stderr_factory_builds_subscriber() {
	let _subscriber = tracing_subscriber::fmt()
		.with_writer(SpinnerAwareMakeWriter(std::io::stderr))
		.with_env_filter(tracing_subscriber::EnvFilter::new("info"))
		.finish();
}

/// TermLike that only records the escape/command stream — enough to observe
/// the clear/redraw ordering around a suspended write.
#[derive(Clone, Debug)]
struct RecordingTerm {
	calls: Arc<Mutex<Vec<String>>>,
}

impl RecordingTerm {
	fn get(&self, marker: &str) -> Vec<usize> {
		self.calls
			.lock()
			.unwrap()
			.iter()
			.enumerate()
			.filter(|(_, c)| c.contains(marker))
			.map(|(i, _)| i)
			.collect()
	}
}

impl TermLike for RecordingTerm {
	fn width(&self) -> u16 {
		80
	}
	fn height(&self) -> u16 {
		24
	}
	fn move_cursor_up(&self, n: usize) -> std::io::Result<()> {
		self.calls.lock().unwrap().push(format!("[up {n}]"));
		Ok(())
	}
	fn move_cursor_down(&self, n: usize) -> std::io::Result<()> {
		self.calls.lock().unwrap().push(format!("[down {n}]"));
		Ok(())
	}
	fn move_cursor_right(&self, n: usize) -> std::io::Result<()> {
		self.calls.lock().unwrap().push(format!("[right {n}]"));
		Ok(())
	}
	fn move_cursor_left(&self, n: usize) -> std::io::Result<()> {
		self.calls.lock().unwrap().push(format!("[left {n}]"));
		Ok(())
	}
	fn write_str(&self, s: &str) -> std::io::Result<()> {
		self.calls.lock().unwrap().push(format!("[write {s:?}]"));
		Ok(())
	}
	fn write_line(&self, s: &str) -> std::io::Result<()> {
		self.calls
			.lock()
			.unwrap()
			.push(format!("[write_line {s:?}]"));
		Ok(())
	}
	fn flush(&self) -> std::io::Result<()> {
		Ok(())
	}
	fn clear_line(&self) -> std::io::Result<()> {
		self.calls.lock().unwrap().push("[clear_line]".into());
		Ok(())
	}
}

/// Shared byte sink standing in for stderr.
#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedSink {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		self.0.lock().unwrap().extend_from_slice(buf);
		Ok(buf.len())
	}
	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}

#[serial]
#[test]
fn cli_writer_suspends_spinner_around_each_write() {
	let manager = crate::session::chat::animation_manager::get_animation_manager();
	let term = RecordingTerm {
		calls: Arc::new(Mutex::new(Vec::new())),
	};
	let pb =
		ProgressBar::with_draw_target(None, ProgressDrawTarget::term_like(Box::new(term.clone())));
	pb.set_style(indicatif::ProgressStyle::default_spinner());
	pb.set_message("working");
	pb.tick(); // initial draw
	manager.install_test_spinner(Some(pb.clone()));

	let sink = SharedSink::default();
	let mut writer = SpinnerAwareMakeWriter(|| sink.clone()).make_writer();
	writer
		.write_all(b"2026 INFO log line\n")
		.expect("write log line");
	writer.flush().expect("flush");

	// The log bytes arrived…
	assert!(
		sink.0
			.lock()
			.unwrap()
			.windows(b"log line".len())
			.any(|w| w == b"log line"),
		"log bytes must reach the sink"
	);
	// …and the bar was cleared BEFORE a redraw AFTER the write: with the old
	// raw-stderr writer the term would record nothing around the write.
	let clears = term.get("[clear_line]");
	let draws = term.get("working");
	assert!(
		!clears.is_empty() && draws.len() >= 2,
		"suspend must clear and redraw the bar: calls={:?}",
		term.calls.lock().unwrap()
	);
	// The redraw following the clear must come after the initial draw.
	assert!(
		draws.last().unwrap() > &draws[0],
		"bar must be redrawn after the suspended write: calls={:?}",
		term.calls.lock().unwrap()
	);

	pb.finish_and_clear();
	manager.install_test_spinner(None);
}

#[serial]
#[test]
fn cli_writer_passes_through_when_no_spinner_is_live() {
	let manager = crate::session::chat::animation_manager::get_animation_manager();
	manager.install_test_spinner(None);

	let sink = SharedSink::default();
	let mut writer = SpinnerAwareMakeWriter(|| sink.clone()).make_writer();
	writer.write_all(b"plain\n").expect("write without spinner");
	assert_eq!(sink.0.lock().unwrap().as_slice(), b"plain\n");
}
