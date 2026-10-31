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
use tokio::sync::mpsc::Receiver;

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

#[test]
fn a_saturated_connection_keeps_the_frame_for_reconnect() {
	let stream = SessionStream::new("s1");
	let (sink, _receiver) = WsSender::channel();
	stream.own(&sink);
	while sink.send(Message::text("queued")).is_ok() {}
	stream.emit(&assistant("held after overload")).unwrap();
	let frames = resume(&stream, at(&stream, 0));
	assert_eq!(frames[0]["resumed"], true);
	assert_eq!(frames[1]["content"], "held after overload");
	assert_eq!(frames[1]["stream_seq"], 1);
}

fn texts(rx: &mut Receiver<super::super::server::QueuedFrame>) -> Vec<String> {
	let mut texts = Vec::new();
	while let Ok(frame) = rx.try_recv() {
		texts.push(frame.frame.to_text().expect("text frame").to_string());
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
fn held_frames_are_bounded_by_bytes_oldest_first_and_the_newest_always_stays() {
	let stream = SessionStream::new("s1");
	let large = "x".repeat(HOLD_MAX_BYTES / 2);
	for _ in 0..3 {
		stream.emit(&assistant(&large)).unwrap();
	}
	assert_eq!(resume(&stream, at(&stream, 1))[0]["resumed"], false);
	let kept = resume(&stream, at(&stream, 2));
	assert_eq!(kept[0]["resumed"], true);
	assert_eq!(kept[1]["stream_seq"], 3);
	assert_eq!(kept.len(), 3, "ack, the one frame that fits, greeting");

	let oversized = SessionStream::new("s1");
	oversized.emit(&assistant(&"x".repeat(HOLD_MAX_BYTES * 2))).unwrap();
	let only = resume(&oversized, at(&oversized, 0));
	assert_eq!(
		only[0]["resumed"], true,
		"a frame over the cap stays while it is the newest"
	);
	assert_eq!(only[1]["stream_seq"], 1);
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

/// Registers an observer on `streams` and returns it with the receiving end of
/// its connection and what it received on registering.
fn observer(
	streams: &Streams,
	marks: &[ObserveMark],
) -> (
	WsSender,
	Receiver<super::super::server::QueuedFrame>,
	Vec<serde_json::Value>,
) {
	let (sink, mut rx) = WsSender::channel();
	streams.observe(&sink, marks).expect("observe");
	let opening = texts(&mut rx).iter().map(|text| json(text)).collect();
	(sink, rx, opening)
}

fn mark(session_id: &str, stream: String, after: u64) -> ObserveMark {
	ObserveMark {
		session_id: session_id.to_string(),
		resume: StreamResume { stream, after },
	}
}

fn observed_seqs(frames: &[serde_json::Value]) -> Vec<u64> {
	frames
		.iter()
		.filter(|frame| frame["type"] == "observed")
		.map(|frame| frame["frame"]["stream_seq"].as_u64().expect("numbered frame"))
		.collect()
}

#[test]
fn an_observer_hears_the_open_streams_then_every_frame_live_byte_identical() {
	let streams = Streams::default();
	let first = streams.open("s1");
	first.emit(&assistant("one")).unwrap();

	let (_sink, mut rx, opening) = observer(&streams, &[]);
	assert_eq!(opening[0]["type"], "observing");
	assert!(
		opening[0]["capabilities"]
			.as_array()
			.is_some_and(|caps| caps.iter().any(|cap| cap == "observe_v1")),
		"got: {}",
		opening[0]
	);
	assert_eq!(opening[1]["type"], "stream_open");
	assert_eq!(opening[1]["session_id"], "s1");
	assert_eq!(opening[1]["stream"], stream_id(&first).as_str());
	assert_eq!(opening[1]["last_seq"], 1);
	assert_eq!(opening[1]["gap"], false);
	assert_eq!(opening[2]["type"], "observed");
	assert_eq!(opening[2]["frame"]["content"], "one");
	assert_eq!(opening[2]["frame"]["stream_seq"], 1);
	assert_eq!(opening.len(), 3);

	// A driver reading the stream receives the very text the observer wraps.
	let (driver, mut driver_rx) = WsSender::channel();
	first.attach(&driver, bind_ack(), None, &greeting(), false).unwrap();
	texts(&mut driver_rx);
	first.emit(&assistant("two")).unwrap();
	let driven = texts(&mut driver_rx);
	let observed = texts(&mut rx);
	assert!(
		observed[0].contains(&driven[0]),
		"the frame travels byte-identical inside the envelope: {}",
		observed[0]
	);
	assert_eq!(json(&observed[0])["session_id"], "s1");
	assert_eq!(json(&observed[0])["stream"], stream_id(&first).as_str());

	// A session loaded later announces itself before its first frame.
	let second = streams.open("s2");
	second.emit(&assistant("other")).unwrap();
	let later: Vec<serde_json::Value> = texts(&mut rx).iter().map(|text| json(text)).collect();
	assert_eq!(later[0]["type"], "stream_open");
	assert_eq!(later[0]["session_id"], "s2");
	assert_eq!(later[0]["last_seq"], 0);
	assert_eq!(later[0]["gap"], false);
	assert_eq!(later[1]["type"], "observed");
	assert_eq!(later[1]["session_id"], "s2");
	assert_eq!(later.len(), 2);
}

#[test]
fn an_observer_mark_of_the_same_stream_replays_exactly_the_frames_after_it() {
	let streams = Streams::default();
	let stream = streams.open("s1");
	for content in ["one", "two", "three", "four"] {
		stream.emit(&assistant(content)).unwrap();
	}
	let id = stream_id(&stream);

	let (_, _, marked) = observer(&streams, &[mark("s1", id.clone(), 2)]);
	assert_eq!(marked[1]["last_seq"], 4);
	assert_eq!(marked[1]["gap"], false);
	assert_eq!(observed_seqs(&marked), vec![3, 4]);

	let (_, _, foreign) = observer(&streams, &[mark("s1", "0123456789abcdef".to_string(), 3)]);
	assert_eq!(foreign[1]["gap"], false);
	assert_eq!(observed_seqs(&foreign), vec![1, 2, 3, 4], "another stream: full replay");

	let (_, _, unmarked) = observer(&streams, &[]);
	assert_eq!(observed_seqs(&unmarked), vec![1, 2, 3, 4]);

	let (_, _, elsewhere) = observer(&streams, &[mark("other-session", id, 4)]);
	assert_eq!(
		observed_seqs(&elsewhere),
		vec![1, 2, 3, 4],
		"a mark names its own session"
	);
}

#[test]
fn a_gap_is_reported_when_the_cap_dropped_frames_past_the_mark() {
	let streams = Streams::default();
	let stream = streams.open("s1");
	let large = "x".repeat(HOLD_MAX_BYTES / 2);
	for _ in 0..3 {
		stream.emit(&assistant(&large)).unwrap();
	}
	let id = stream_id(&stream);
	// Only frame 3 is still held.

	let (_, _, behind) = observer(&streams, &[mark("s1", id.clone(), 1)]);
	assert_eq!(behind[1]["gap"], true, "frame 2 was lost: {}", behind[1]);
	assert_eq!(observed_seqs(&behind), vec![3]);

	let (_, _, caught_up) = observer(&streams, &[mark("s1", id, 2)]);
	assert_eq!(caught_up[1]["gap"], false);
	assert_eq!(observed_seqs(&caught_up), vec![3]);

	let (_, _, unmarked) = observer(&streams, &[]);
	assert_eq!(unmarked[1]["gap"], true, "frames 1 and 2 were lost");

	let (_, _, foreign) = observer(&streams, &[mark("s1", "0123456789abcdef".to_string(), 2)]);
	assert_eq!(
		foreign[1]["gap"], true,
		"another stream replays all that is held: {}",
		foreign[1]
	);
}

#[test]
fn stream_open_and_close_are_announced_to_observers_connected_before() {
	let streams = Streams::default();
	let (_sink, mut rx, opening) = observer(&streams, &[]);
	assert_eq!(opening.len(), 1, "nothing is loaded: only `observing`");

	let stream = streams.open("s1");
	let id = stream_id(&stream);
	streams.open("s1");
	streams.close("s1");
	streams.close("s1");
	let frames: Vec<serde_json::Value> = texts(&mut rx).iter().map(|text| json(text)).collect();
	assert_eq!(
		frames.len(),
		2,
		"one open and one close, however often they are asked: {frames:?}"
	);
	assert_eq!(frames[0]["type"], "stream_open");
	assert_eq!(frames[0]["session_id"], "s1");
	assert_eq!(frames[0]["stream"], id.as_str());
	assert_eq!(frames[0]["last_seq"], 0);
	assert_eq!(frames[0]["gap"], false);
	assert_eq!(frames[1]["type"], "stream_closed");
	assert_eq!(frames[1]["session_id"], "s1");
	assert_eq!(frames[1]["stream"], id.as_str());

	streams.open("s1");
	let reloaded = json(&texts(&mut rx)[0]);
	assert_eq!(reloaded["type"], "stream_open");
	assert_ne!(reloaded["stream"], id.as_str(), "a reload mints a new stream");
}

#[test]
fn an_observer_never_owns_the_output_and_a_gone_observer_is_dropped() {
	let streams = Streams::default();
	let stream = streams.open("s1");
	let (observer_sink, mut observer_rx, _) = observer(&streams, &[]);

	// Nobody owns the output, so a plain bind adopts it: the observer was never
	// counted as the owner.
	let (driver, mut driver_rx) = WsSender::channel();
	stream.attach(&driver, bind_ack(), None, &greeting(), false).unwrap();
	texts(&mut driver_rx);
	stream.emit(&assistant("one")).unwrap();
	assert_eq!(texts(&mut driver_rx).len(), 1, "the driver owns the output");
	assert_eq!(texts(&mut observer_rx).len(), 1, "and the observer still has its copy");

	streams.detach(&observer_sink);
	stream.emit(&assistant("two")).unwrap();
	assert!(texts(&mut observer_rx).is_empty(), "a detached observer gets nothing");
	assert_eq!(texts(&mut driver_rx).len(), 1);

	let (saturated, _rx, _) = observer(&streams, &[]);
	while saturated.send(Message::text("queued")).is_ok() {}
	stream.emit(&assistant("three")).unwrap();
	assert_eq!(
		texts(&mut driver_rx).len(),
		1,
		"an overflowing observer does not hold the driver back"
	);
	assert!(
		stream.0.lock().unwrap().observers.is_empty(),
		"an observer that refuses a frame is dropped"
	);
}

#[test]
fn an_observer_registering_mid_stream_sees_no_gap_and_no_duplicate() {
	let streams = Streams::default();
	let stream = streams.open("s1");
	let emitted = 2000;
	let emitter = {
		let stream = Arc::clone(&stream);
		std::thread::spawn(move || {
			for _ in 0..emitted {
				stream.emit(&assistant("x")).unwrap();
			}
		})
	};
	let (_sink, mut rx, opening) = observer(&streams, &[]);
	emitter.join().unwrap();

	let mut frames = opening;
	frames.extend(texts(&mut rx).iter().map(|text| json(text)));
	assert_eq!(observed_seqs(&frames), (1..=emitted as u64).collect::<Vec<_>>());
}

/// Twelve streams of about 1.8 MiB each are 22 MB of replay, more than a
/// connection may queue (16 MiB): one `observe` must stay inside its budget
/// instead, replay the newest frames of what does not fit, and say so.
#[test]
fn one_observe_replays_within_its_budget_and_flags_what_it_left_out() {
	let streams = Streams::default();
	let chunk = "x".repeat(HOLD_MAX_BYTES / 9);
	for index in 0..12 {
		let stream = streams.open(&format!("s{index}"));
		for _ in 0..8 {
			stream.emit(&assistant(&chunk)).unwrap();
		}
	}

	let (sink, mut rx) = WsSender::channel();
	streams.observe(&sink, &[]).expect("observe");
	let received = texts(&mut rx);
	let replayed_bytes: usize = received
		.iter()
		.filter(|text| text.starts_with(r#"{"type":"observed""#))
		.map(String::len)
		.sum();
	assert!(
		replayed_bytes <= OBSERVE_REPLAY_MAX_BYTES,
		"replayed {replayed_bytes} bytes"
	);
	assert!(
		sink.send(Message::text("live")).is_ok(),
		"the replay leaves the connection open and room for live frames"
	);

	let frames: Vec<serde_json::Value> = received.iter().map(|text| json(text)).collect();
	let opened: Vec<&serde_json::Value> = frames.iter().filter(|frame| frame["type"] == "stream_open").collect();
	assert_eq!(opened.len(), 12);
	let (mut whole, mut cut) = (0, 0);
	for open in opened {
		let session_id = open["session_id"].as_str().unwrap();
		let last = open["last_seq"].as_u64().unwrap();
		let seqs: Vec<u64> = frames
			.iter()
			.filter(|frame| frame["type"] == "observed" && frame["session_id"] == session_id)
			.map(|frame| frame["frame"]["stream_seq"].as_u64().unwrap())
			.collect();
		let first_replayed = seqs.first().copied().unwrap_or(last + 1);
		assert_eq!(
			seqs,
			(first_replayed..=last).collect::<Vec<_>>(),
			"{session_id}: the newest frames, in order, none skipped"
		);
		assert_eq!(open["gap"], first_replayed > 1, "{session_id}: {open}");
		if first_replayed > 1 {
			cut += 1;
		} else {
			whole += 1;
		}
	}
	assert!(whole > 0, "the budget replays some streams whole");
	assert!(cut > 0, "and flags the ones it could not");
}

/// Many small frames fill a connection's frame queue long before their bytes
/// do: the hold keeps at most `HOLD_MAX_FRAMES`, newest kept, so a resume
/// replay always fits the queue.
#[test]
fn held_frames_are_bounded_by_count_too() {
	let stream = SessionStream::new("s1");
	for _ in 0..HOLD_MAX_FRAMES + 10 {
		stream.emit(&assistant("x")).unwrap();
	}
	let state = stream.0.lock().unwrap();
	assert_eq!(state.held.len(), HOLD_MAX_FRAMES);
	assert_eq!(state.held.back().unwrap().seq, state.last_seq, "the newest frame stays");
	assert_eq!(state.held.front().unwrap().seq, 11, "the oldest ten went first");
}

/// Three streams of `HOLD_MAX_FRAMES` tiny frames are well under the byte
/// budget but over the frame budget: one `observe` replays at most
/// `OBSERVE_REPLAY_MAX_FRAMES` frames and flags what it left out.
#[test]
fn one_observe_replays_within_its_frame_budget() {
	let streams = Streams::default();
	for index in 0..3 {
		let stream = streams.open(&format!("s{index}"));
		for _ in 0..HOLD_MAX_FRAMES {
			stream.emit(&assistant("x")).unwrap();
		}
	}
	let (sink, mut rx) = WsSender::channel();
	streams.observe(&sink, &[]).expect("observe");
	let frames: Vec<serde_json::Value> = texts(&mut rx).iter().map(|text| json(text)).collect();
	let replayed = frames.iter().filter(|frame| frame["type"] == "observed").count();
	assert!(replayed <= OBSERVE_REPLAY_MAX_FRAMES, "replayed {replayed} frames");
	let gaps = frames
		.iter()
		.filter(|frame| frame["type"] == "stream_open" && frame["gap"] == true)
		.count();
	assert!(gaps > 0, "a stream the frame budget cut is flagged");
	assert!(sink.send(Message::text("live")).is_ok(), "the connection stays open");
}

/// A held frame too big for one replay would cut the resuming connection, and
/// the next reconnect would ask for it again: the resume is refused instead,
/// sending nothing held, so the client rebuilds from its own history.
#[test]
fn a_resume_that_cannot_fit_one_replay_is_refused_not_sent() {
	let stream = SessionStream::new("s1");
	stream.emit(&assistant("before")).unwrap();
	stream
		.emit(&assistant(&"x".repeat(OBSERVE_REPLAY_MAX_BYTES + 1)))
		.unwrap();
	let frames = resume(&stream, at(&stream, 0));
	assert_eq!(frames[0]["resumed"], false, "{}", frames[0]);
	assert_eq!(frames.len(), 2, "ack and greeting only");
}
