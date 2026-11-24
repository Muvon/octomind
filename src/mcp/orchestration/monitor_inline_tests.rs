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

#[test]
fn bounded_buffer_caps_output_and_reports_omission() {
	let mut buffer = BoundedBuffer::new(4);
	buffer.push(b"abcdef");
	assert_eq!(buffer.bytes, b"abcd");
	assert_eq!(buffer.dropped_bytes(), 2);
	assert!(buffer.render().contains("2 additional bytes omitted"));
	let _ = buffer.take_rendered();
	assert!(buffer.is_empty());
	assert_eq!(buffer.dropped_bytes(), 0);
}

#[test]
fn orchestration_namespace_exposes_monitor() {
	assert!(crate::mcp::orchestration::get_all_functions()
		.iter()
		.any(|function| function.name == "monitor"));
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_batches_streamed_output_and_stops_with_session() {
	let dir = tempfile::tempdir().unwrap();

	let session_id = format!("monitor-test-{}", Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		init_for_session();
		let spec = MonitorSpec {
			description: "test events".to_string(),
			command: "printf 'event one\\nevent two\\n'; sleep 5".to_string(),
			workdir: dir.path().to_path_buf(),
			flush_interval_secs: 1,
			max_batch_bytes: 4096,
			timeout_ms: None,
		};
		let id = start_monitor(session_id.clone(), spec, Duration::from_millis(25))
			.await
			.unwrap();

		let message = tokio::time::timeout(Duration::from_secs(2), async {
			loop {
				if let Some(message) = crate::session::inbox::try_pop_inbox_message() {
					break message;
				}
				tokio::time::sleep(Duration::from_millis(5)).await;
			}
		})
		.await
		.expect("monitor did not inject output");
		assert!(message.content.contains("event one\nevent two"));
		assert!(matches!(
			message.source,
			crate::session::inbox::InboxSource::Monitor { id: ref source_id, .. }
				if source_id == &id
		));
		assert!(has_running_monitors());

		clear_for_session(&session_id);
		let stopped = tokio::time::timeout(Duration::from_secs(2), async {
			while has_running_monitors() {
				tokio::time::sleep(Duration::from_millis(5)).await;
			}
		})
		.await;
		assert!(stopped.is_ok());
		crate::session::inbox::clear_inbox_for_session(&session_id);
	})
	.await;
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_stops_descendants_on_cancel_and_deadline() {
	const WAIT_LIMIT: Duration = Duration::from_secs(5);
	const POLL_INTERVAL: Duration = Duration::from_millis(25);

	let dir = tempfile::tempdir().unwrap();
	for timeout_ms in [None, Some(MIN_TIMEOUT_MS as u64)] {
		let session_id = format!("monitor-child-test-{}", Uuid::new_v4());
		crate::session::context::with_session_id(session_id.clone(), async {
			crate::session::inbox::init_inbox_for_session();
			init_for_session();
			let flush_interval = match timeout_ms {
				Some(_) => Duration::from_secs(DEFAULT_FLUSH_INTERVAL_SECS),
				None => POLL_INTERVAL,
			};
			let id = start_monitor(
				session_id.clone(),
				MonitorSpec {
					description: "watcher with a child".to_string(),
					command: "sleep 30 & printf 'child pid %s\\n' \"$!\"; wait".to_string(),
					workdir: dir.path().to_path_buf(),
					flush_interval_secs: DEFAULT_FLUSH_INTERVAL_SECS,
					max_batch_bytes: DEFAULT_MAX_BATCH_BYTES,
					timeout_ms,
				},
				flush_interval,
			)
			.await
			.unwrap();

			let message = tokio::time::timeout(WAIT_LIMIT, async {
				loop {
					if let Some(message) = crate::session::inbox::try_pop_inbox_message() {
						break message;
					}
					tokio::time::sleep(POLL_INTERVAL).await;
				}
			})
			.await
			.expect("monitor did not report its child");
			let pid = message
				.content
				.lines()
				.find_map(|line| line.strip_prefix("child pid "))
				.expect("watcher printed its child PID")
				.parse::<libc::pid_t>()
				.expect("valid child PID");
			if timeout_ms.is_some() {
				assert!(message.content.contains("monitor reached timeout"));
			} else {
				assert!(cancel_monitor(&session_id, &id));
			}

			tokio::time::timeout(WAIT_LIMIT, async {
				while has_running_monitors() {
					tokio::time::sleep(POLL_INTERVAL).await;
				}
				loop {
					let output = Command::new("ps")
						.args(["-o", "stat=", "-p", &pid.to_string()])
						.output()
						.await
						.expect("query child process state");
					let state = String::from_utf8(output.stdout).expect("process state is UTF-8");
					// An orphan can remain a zombie until its new parent reaps it.
					if (output.status.code() == Some(1) && output.stderr.is_empty() && state.trim().is_empty())
						|| (output.status.success() && state.trim_start().starts_with('Z'))
					{
						break;
					}
					assert!(
						output.status.success() && !state.trim().is_empty(),
						"{state:?}, {:?}",
						output.stderr
					);
					tokio::time::sleep(POLL_INTERVAL).await;
				}
			})
			.await
			.unwrap_or_else(|_| panic!("monitor {id} left child {pid} running (timeout_ms={timeout_ms:?})"));
			clear_for_session(&session_id);
			crate::session::inbox::clear_inbox_for_session(&session_id);
		})
		.await;
	}
}

#[cfg(unix)]
#[tokio::test]
async fn failed_command_injects_one_terminal_diagnostic() {
	let dir = tempfile::tempdir().unwrap();

	let session_id = format!("monitor-failure-test-{}", Uuid::new_v4());
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::init_inbox_for_session();
		init_for_session();
		start_monitor(
			session_id.clone(),
			MonitorSpec {
				description: "failing watcher".to_string(),
				command: "echo 'watch failed' >&2; exit 7".to_string(),
				workdir: dir.path().to_path_buf(),
				flush_interval_secs: 30,
				max_batch_bytes: 4096,
				timeout_ms: Some(30_000),
			},
			Duration::from_secs(30),
		)
		.await
		.unwrap();

		let message = tokio::time::timeout(Duration::from_secs(2), async {
			loop {
				if let Some(message) = crate::session::inbox::try_pop_inbox_message() {
					break message;
				}
				tokio::time::sleep(Duration::from_millis(5)).await;
			}
		})
		.await
		.expect("monitor failure was not injected");
		assert!(message.content.contains("unsuccessfully (7)"));
		assert!(message.content.contains("watch failed"));
		assert!(!has_running_monitors());
		assert!(crate::session::inbox::try_pop_inbox_message().is_none());
		clear_for_session(&session_id);
		crate::session::inbox::clear_inbox_for_session(&session_id);
	})
	.await;
}
