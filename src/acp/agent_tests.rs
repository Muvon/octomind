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
use crate::websocket::McpNotificationPayload;
use futures::AsyncReadExt;

fn progress(tool_id: Option<&str>) -> ServerMessage {
	ServerMessage::McpNotification(McpNotificationPayload {
		server: "octofs".to_string(),
		method: "notifications/progress".to_string(),
		params: serde_json::json!({
			"progressToken": 1,
			"progress": 3.0,
			"message": "command still running"
		}),
		tool_id: tool_id.map(str::to_string),
	})
}

#[test]
fn progress_patches_the_tool_call_it_belongs_to() {
	let update = translate_server_message_to_acp(progress(Some("call-1")))
		.expect("progress with a tool id is forwarded");
	match update {
		SessionUpdate::ToolCallUpdate(upd) => {
			assert_eq!(&*upd.tool_call_id.0, "call-1");
			assert_eq!(
				upd.fields.title.as_deref(),
				Some("[octofs] command still running")
			);
			// Liveness is not completion — status must stay untouched.
			assert!(upd.fields.status.is_none());
		}
		other => panic!("expected a tool call update, got {other:?}"),
	}
}

#[test]
fn progress_without_a_tool_call_is_dropped() {
	// ACP has no session-level progress surface, so an unattributable beat has
	// nowhere to go — better dropped than rendered as agent output.
	assert!(translate_server_message_to_acp(progress(None)).is_none());
}
/// The disconnect signal must fire exactly when the stream hits EOF —
/// not on ordinary reads. `serve` relies on it to shut the process down
/// once the client closes our stdin; if it stops firing, every ACP
/// subprocess outlives its parent again.
#[tokio::test]
async fn signal_on_eof_fires_exactly_at_eof() {
	let (tx, mut rx) = tokio::sync::oneshot::channel();
	let mut reader = SignalOnEof {
		inner: futures::io::Cursor::new(b"data".to_vec()),
		eof_tx: Some(tx),
	};

	let mut buf = [0u8; 4];
	let n = reader.read(&mut buf).await.unwrap();
	assert_eq!(n, 4);
	assert!(
		matches!(
			rx.try_recv(),
			Err(tokio::sync::oneshot::error::TryRecvError::Empty)
		),
		"signal must not fire before EOF"
	);

	let n = reader.read(&mut buf).await.unwrap();
	assert_eq!(n, 0);
	rx.await.expect("EOF must fire the disconnect signal");
}
