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
use crate::websocket::protocol::{AssistantPayload, SessionMessage};
use tokio::sync::mpsc::UnboundedReceiver;

fn assistant(content: &str) -> ServerMessage {
	ServerMessage::Assistant(AssistantPayload {
		content: content.to_string(),
		session_id: "s1".to_string(),
		step: None,
	})
}

fn bind_ack() -> AckPayload {
	AckPayload::bind(&SessionMessage {
		request_id: Some("b1".to_string()),
		session_id: Some("s1".to_string()),
		cwd: None,
		role: None,
		resume: None,
		capabilities: Vec::new(),
	})
}

fn greeting() -> ServerMessage {
	ServerMessage::status("Session resumed: s1".to_string(), Some("s1".to_string()))
}

fn texts(rx: &mut UnboundedReceiver<Message>) -> Vec<String> {
	let mut texts = Vec::new();
	while let Ok(frame) = rx.try_recv() {
		texts.push(frame.to_text().expect("text frame").to_string());
	}
	texts
}

fn json(text: &str) -> serde_json::Value {
	serde_json::from_str(text).expect("frames are JSON")
}

fn stream_id(stream: &SessionStream) -> String {
	stream.0.lock().unwrap().id.clone()
}

/// Binds a fresh connection with `resume` and returns everything it received.
fn resume(stream: &SessionStream, resume: Option<StreamResume>) -> Vec<serde_json::Value> {
	let (sink, mut rx) = WsSender::channel();
	stream
		.attach(&sink, bind_ack(), resume.as_ref(), &greeting(), false)
		.expect("attach");
	texts(&mut rx).iter().map(|text| json(text)).collect()
}

fn at(stream: &SessionStream, after: u64) -> Option<StreamResume> {
	Some(StreamResume {
		stream: stream_id(stream),
		after,
	})
}

#[test]
fn frames_carry_a_gapless_stream_seq_beside_their_own_fields() {
	let stream = SessionStream::new("s1");
	let (sink, mut rx) = WsSender::channel();
	stream.attach(&sink, bind_ack(), None, &greeting(), false).unwrap();
	texts(&mut rx);

	for content in ["one", "two", "three"] {
		stream.emit(&assistant(content)).unwrap();
	}

	let frames = texts(&mut rx);
	assert_eq!(
		json(&frames[0]),
		serde_json::json!({"type": "assistant", "content": "one", "session_id": "s1", "stream_seq": 1})
	);
	let seqs: Vec<u64> = frames
		.iter()
		.map(|text| json(text)["stream_seq"].as_u64().unwrap())
		.collect();
	assert_eq!(seqs, vec![1, 2, 3]);
}

#[test]
fn the_bind_ack_reports_the_stream_and_carries_resumed_only_when_asked() {
	let stream = SessionStream::new("s1");
	let fresh = resume(&stream, None);
	assert_eq!(fresh[0]["type"], "ack");
	assert_eq!(fresh[0]["request_id"], "b1");
	assert_eq!(fresh[0]["stream_seq"], 0, "nothing assigned yet: {}", fresh[0]);
	let id = fresh[0]["stream"].as_str().expect("stream id");
	assert!(
		id.len() >= 16 && id.bytes().all(|b| b.is_ascii_hexdigit()),
		"at least 64 bits of hex: {id}"
	);
	assert!(fresh[0].get("resumed").is_none(), "no resume asked: {}", fresh[0]);
	assert!(
		fresh[1].get("stream_seq").is_none(),
		"the greeting answers the bind, it is not session output: {}",
		fresh[1]
	);
	assert_eq!(fresh.len(), 2);

	stream.emit(&assistant("one")).unwrap();
	let again = resume(&stream, None);
	assert_eq!(again[0]["stream"], id, "one stream while the session stays loaded");
	assert_eq!(again[0]["stream_seq"], 1);
	assert_ne!(
		SessionStream::new("s1").0.lock().unwrap().id,
		id,
		"every load mints its own stream"
	);
}

#[test]
fn frames_emitted_while_unbound_are_held_and_resumed() {
	let stream = SessionStream::new("s1");
	stream.emit(&assistant("one")).unwrap();
	stream.emit(&assistant("two")).unwrap();

	let frames = resume(&stream, at(&stream, 0));
	assert_eq!(frames[0]["resumed"], true, "got: {}", frames[0]);
	assert_eq!(frames[0]["stream_seq"], 2);
	assert_eq!(frames[1]["content"], "one");
	assert_eq!(frames[2]["content"], "two");
	assert_eq!(frames[3]["type"], "status", "the greeting follows the replay");
	assert_eq!(frames.len(), 4);
}

#[test]
fn a_resume_replays_the_missed_frames_byte_identical_and_takes_the_live_rest() {
	let stream = SessionStream::new("s1");
	let (first, mut first_rx) = WsSender::channel();
	stream.attach(&first, bind_ack(), None, &greeting(), false).unwrap();
	texts(&mut first_rx);
	for content in ["one", "two", "three", "four"] {
		stream.emit(&assistant(content)).unwrap();
	}
	let originals = texts(&mut first_rx);

	let (second, mut second_rx) = WsSender::channel();
	stream
		.attach(&second, bind_ack(), at(&stream, 2).as_ref(), &greeting(), false)
		.unwrap();
	stream.emit(&assistant("five")).unwrap();

	let received = texts(&mut second_rx);
	assert_eq!(json(&received[0])["resumed"], true);
	assert_eq!(json(&received[0])["stream_seq"], 4);
	assert_eq!(&received[1..3], &originals[2..4], "replayed exactly as first sent");
	assert_eq!(json(&received[3])["type"], "status");
	assert_eq!(json(&received[4])["stream_seq"], 5);
	assert_eq!(received.len(), 5);
	assert!(
		texts(&mut first_rx).is_empty(),
		"the latest bind owns the output: the earlier connection gets nothing more"
	);
}

#[test]
fn a_resume_up_to_the_last_frame_replays_nothing_but_is_honoured() {
	let stream = SessionStream::new("s1");
	stream.emit(&assistant("one")).unwrap();
	let frames = resume(&stream, at(&stream, 1));
	assert_eq!(frames[0]["resumed"], true);
	assert_eq!(frames[1]["type"], "status");
	assert_eq!(frames.len(), 2);
}

#[test]
fn a_resume_of_another_stream_or_an_unknown_point_is_refused_without_replay() {
	let stream = SessionStream::new("s1");
	stream.emit(&assistant("one")).unwrap();

	let other = resume(
		&stream,
		Some(StreamResume {
			stream: "0123456789abcdef".to_string(),
			after: 0,
		}),
	);
	assert_eq!(other[0]["resumed"], false, "got: {}", other[0]);
	assert_eq!(other[1]["type"], "status", "nothing replayed");
	assert_eq!(other.len(), 2);

	let ahead = resume(&stream, at(&stream, 5));
	assert_eq!(ahead[0]["resumed"], false, "a point the stream never reached");
	assert_eq!(ahead.len(), 2);
}

#[test]
fn a_new_turn_keeps_only_the_turn_before_it() {
	let stream = SessionStream::new("s1");
	stream.begin_turn();
	stream.emit(&assistant("turn one")).unwrap();
	stream.begin_turn();
	stream.emit(&assistant("turn two")).unwrap();
	stream.emit(&assistant("turn two, more")).unwrap();
	stream.begin_turn();
	stream.emit(&assistant("turn three")).unwrap();

	let evicted = resume(&stream, at(&stream, 0));
	assert_eq!(evicted[0]["resumed"], false, "turn one is gone");
	assert_eq!(evicted.len(), 2);

	let kept = resume(&stream, at(&stream, 1));
	assert_eq!(kept[0]["resumed"], true, "the last finished turn is held");
	let seqs: Vec<u64> = kept[1..4]
		.iter()
		.map(|frame| frame["stream_seq"].as_u64().unwrap())
		.collect();
	assert_eq!(seqs, vec![2, 3, 4]);
}

#[test]
fn held_frames_are_bounded_by_count_oldest_first() {
	let stream = SessionStream::new("s1");
	for _ in 0..=MAX_HELD_FRAMES {
		stream.emit(&assistant("x")).unwrap();
	}
	assert_eq!(resume(&stream, at(&stream, 0))[0]["resumed"], false);
	let kept = resume(&stream, at(&stream, 1));
	assert_eq!(kept[0]["resumed"], true);
	assert_eq!(kept.len(), MAX_HELD_FRAMES + 2, "ack, every held frame, greeting");
}

#[test]
fn held_frames_are_bounded_by_bytes_oldest_first() {
	let stream = SessionStream::new("s1");
	let large = "x".repeat(MAX_HELD_BYTES / 2);
	for _ in 0..3 {
		stream.emit(&assistant(&large)).unwrap();
	}
	assert_eq!(resume(&stream, at(&stream, 1))[0]["resumed"], false);
	let kept = resume(&stream, at(&stream, 2));
	assert_eq!(kept[0]["resumed"], true);
	assert_eq!(kept[1]["stream_seq"], 3);
}

#[test]
fn a_gone_connection_unbinds_and_its_frames_wait_for_the_next_bind() {
	let streams = Streams::default();
	let stream = streams.open("s1");
	let (sink, mut rx) = WsSender::channel();
	stream.attach(&sink, bind_ack(), None, &greeting(), false).unwrap();
	texts(&mut rx);

	streams.detach(&sink);
	stream.emit(&assistant("held")).unwrap();
	assert!(texts(&mut rx).is_empty(), "a detached connection gets nothing");

	let (other, mut other_rx) = WsSender::channel();
	stream.attach(&other, bind_ack(), None, &greeting(), false).unwrap();
	texts(&mut other_rx);
	stream.emit(&assistant("adopted")).unwrap();
	assert_eq!(
		json(&texts(&mut other_rx)[0])["content"],
		"adopted",
		"a plain bind takes output nobody owns"
	);

	let (late, mut late_rx) = WsSender::channel();
	stream.attach(&late, bind_ack(), None, &greeting(), false).unwrap();
	assert_eq!(texts(&mut late_rx).len(), 2, "ack and greeting only");
	stream.emit(&assistant("still the owner's")).unwrap();
	assert!(
		texts(&mut late_rx).is_empty(),
		"a plain bind never takes a live owner's output"
	);
	assert_eq!(texts(&mut other_rx).len(), 1);

	stream.own(&late);
	stream.emit(&assistant("the sender's turn")).unwrap();
	assert_eq!(
		texts(&mut late_rx).len(),
		1,
		"a client's own input moves its turn to it"
	);
	assert!(texts(&mut other_rx).is_empty());

	streams.close("s1");
	assert_ne!(
		stream_id(&streams.open("s1")),
		stream_id(&stream),
		"a closed session loads under a new stream"
	);
}

/// MCP elicitations are answered by the connection that owns the output, when
/// it bound with `elicitation_v1`; with no live owner, nobody answers.
#[test]
fn the_output_owner_answers_elicitations() {
	use crate::mcp::elicitation::{answerer, Answerer};
	let session = "stream-elicitation-owner";
	let stream = SessionStream::new(session);
	let (capable, mut capable_rx) = WsSender::channel();
	stream.attach(&capable, bind_ack(), None, &greeting(), true).unwrap();
	texts(&mut capable_rx);
	assert_eq!(answerer(session), Some(Answerer::Client));

	let (plain, _plain_rx) = WsSender::channel();
	stream.attach(&plain, bind_ack(), None, &greeting(), false).unwrap();
	assert_eq!(
		answerer(session),
		Some(Answerer::Client),
		"a plain bind leaves a live owner, and its answering, in place"
	);

	stream.own(&plain);
	assert_eq!(answerer(session), None, "output moved to a client that cannot answer");
	stream.own(&capable);
	assert_eq!(answerer(session), Some(Answerer::Client));

	stream.detach(&capable);
	assert_eq!(answerer(session), None, "nobody answers once the owner is gone");
}

/// A question waits for its live owner: losing it ends the question instead of
/// holding the tool call forever.
#[serial_test::serial]
#[tokio::test]
async fn losing_the_output_owner_cancels_open_questions() {
	let session = "stream-elicitation-owner-gone";
	let stream = SessionStream::new(session);
	let (capable, _capable_rx) = WsSender::channel();
	stream.attach(&capable, bind_ack(), None, &greeting(), true).unwrap();
	// The session's notification channel, where its questions go out.
	let (frames_tx, mut frames) = tokio::sync::mpsc::unbounded_channel();
	crate::session::context::set_notification_sender_for_session(&session.to_string(), frames_tx);
	let params = rmcp::model::ElicitRequestParams::FormElicitationParams {
		meta: None,
		message: "Still there?".to_string(),
		requested_schema: serde_json::from_value(serde_json::json!({
			"type": "object",
			"properties": { "ok": { "type": "boolean" } }
		}))
		.expect("valid schema fixture"),
	};
	let asker = tokio::spawn(async move { crate::mcp::elicitation::ask("srv", Some(session), None, params).await });
	let asked = tokio::time::timeout(std::time::Duration::from_secs(2), frames.recv()).await;
	assert!(
		matches!(asked, Ok(Some(ServerMessage::Elicitation(_)))),
		"the question must go out first"
	);

	stream.detach(&capable);
	let answer = tokio::time::timeout(std::time::Duration::from_secs(2), asker)
		.await
		.expect("the question must end with its owner")
		.expect("asker task must not panic")
		.expect("a cancelled question is not an error");
	assert_eq!(answer.action, rmcp::model::ElicitationAction::Cancel);
	crate::session::context::clear_notification_sender_for_session(&session.to_string());
}
