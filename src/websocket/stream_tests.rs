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
		.attach(&sink, bind_ack(), resume.as_ref(), &greeting())
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
	let stream = SessionStream::new();
	let (sink, mut rx) = WsSender::channel();
	stream.attach(&sink, bind_ack(), None, &greeting()).unwrap();
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
	let stream = SessionStream::new();
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
		SessionStream::new().0.lock().unwrap().id,
		id,
		"every load mints its own stream"
	);
}

#[test]
fn frames_emitted_while_unbound_are_held_and_resumed() {
	let stream = SessionStream::new();
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
	let stream = SessionStream::new();
	let (first, mut first_rx) = WsSender::channel();
	stream.attach(&first, bind_ack(), None, &greeting()).unwrap();
	texts(&mut first_rx);
	for content in ["one", "two", "three", "four"] {
		stream.emit(&assistant(content)).unwrap();
	}
	let originals = texts(&mut first_rx);

	let (second, mut second_rx) = WsSender::channel();
	stream
		.attach(&second, bind_ack(), at(&stream, 2).as_ref(), &greeting())
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
	let stream = SessionStream::new();
	stream.emit(&assistant("one")).unwrap();
	let frames = resume(&stream, at(&stream, 1));
	assert_eq!(frames[0]["resumed"], true);
	assert_eq!(frames[1]["type"], "status");
	assert_eq!(frames.len(), 2);
}

#[test]
fn a_resume_of_another_stream_or_an_unknown_point_is_refused_without_replay() {
	let stream = SessionStream::new();
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
	let stream = SessionStream::new();
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
	let stream = SessionStream::new();
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
	let stream = SessionStream::new();
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
	stream.attach(&sink, bind_ack(), None, &greeting()).unwrap();
	texts(&mut rx);

	streams.detach(&sink);
	stream.emit(&assistant("held")).unwrap();
	assert!(texts(&mut rx).is_empty(), "a detached connection gets nothing");

	let (other, mut other_rx) = WsSender::channel();
	stream.attach(&other, bind_ack(), None, &greeting()).unwrap();
	texts(&mut other_rx);
	stream.emit(&assistant("adopted")).unwrap();
	assert_eq!(
		json(&texts(&mut other_rx)[0])["content"],
		"adopted",
		"a plain bind takes output nobody owns"
	);

	let (late, mut late_rx) = WsSender::channel();
	stream.attach(&late, bind_ack(), None, &greeting()).unwrap();
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
