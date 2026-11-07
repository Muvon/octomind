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
use crate::supervisor::resolve::ResolvedTask;

fn job(resolution: tokio::sync::oneshot::Receiver<ResolvedTask>) -> ResolveJob {
	let (cancel, _) = tokio::sync::watch::channel(false);
	ResolveJob {
		handle: tokio::spawn(async move { resolution.await.expect("resolution sent") }),
		_cancel: cancel,
	}
}

#[tokio::test]
async fn a_running_resolution_waits_for_a_later_boundary_without_blocking() {
	let mut session = ChatSession::for_tests(Vec::new());
	let (send, receive) = tokio::sync::oneshot::channel();
	session.resolve_job = Some(job(receive));

	settle_resolution(&mut session, false).await;
	assert!(session.gate_task.is_none(), "an unfinished resolution is not applied");
	assert!(session.resolve_job.is_some(), "and stays pending for the next boundary");

	send.send(ResolvedTask::self_contained("fix it".to_string()))
		.expect("job alive");
	while !session.resolve_job.as_ref().expect("pending").handle.is_finished() {
		tokio::task::yield_now().await;
	}
	settle_resolution(&mut session, false).await;
	assert_eq!(
		session.gate_task.as_ref().map(|task| task.original_request.as_str()),
		Some("fix it")
	);
	assert!(session.resolve_job.is_none());
}

#[tokio::test]
async fn the_gate_boundary_waits_for_a_running_resolution() {
	let mut session = ChatSession::for_tests(Vec::new());
	let (send, receive) = tokio::sync::oneshot::channel();
	session.resolve_job = Some(job(receive));
	tokio::spawn(async move {
		tokio::task::yield_now().await;
		send.send(ResolvedTask::self_contained("ship it".to_string()))
			.expect("job alive");
	});

	settle_resolution(&mut session, true).await;
	assert_eq!(
		session.gate_task.as_ref().map(|task| task.original_request.as_str()),
		Some("ship it")
	);
	assert!(session.resolve_job.is_none());
}
