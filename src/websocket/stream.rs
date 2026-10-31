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

// Session output streams: numbered, buffered, routed to the latest bind and
// copied to every observer.

use super::protocol::{
	observed_frame, server_capabilities, AckPayload, ObserveMark, ObserverMessage, ServerMessage, StreamResume,
};
use super::server::{WsSender, MAX_QUEUED_BYTES, MAX_QUEUED_FRAMES};
use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio_tungstenite::tungstenite::Message;

/// Held bytes per stream: the replay source for a resuming driver and for an
/// observer reconnecting with a mark. Frames are whole messages (never token
/// deltas). The newest frame is always kept, so one large tool result still
/// replays.
const HOLD_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Held frames per stream: a replay must also fit the connection's frame
/// queue, which many small frames fill long before their bytes add up.
const HOLD_MAX_FRAMES: usize = MAX_QUEUED_FRAMES / 2;

/// What one `observe` replays in total, across all streams: half a
/// connection's outbound limits, in bytes and in frames, so the replay always
/// fits and leaves room for the live frames behind it.
const OBSERVE_REPLAY_MAX_BYTES: usize = MAX_QUEUED_BYTES / 2;
const OBSERVE_REPLAY_MAX_FRAMES: usize = MAX_QUEUED_FRAMES / 2;

/// What is left of one `observe`'s replay allowance.
struct ReplayBudget {
	bytes: usize,
	frames: usize,
}

impl ReplayBudget {
	fn observe() -> Self {
		Self {
			bytes: OBSERVE_REPLAY_MAX_BYTES,
			frames: OBSERVE_REPLAY_MAX_FRAMES,
		}
	}

	/// Charge one replayed envelope; false (nothing charged) when it does not fit.
	fn take(&mut self, len: usize) -> bool {
		if len > self.bytes || self.frames == 0 {
			return false;
		}
		self.bytes -= len;
		self.frames -= 1;
		true
	}
}

/// A session frame as it goes on the wire: the payload plus its place in the
/// session's stream.
#[derive(Serialize)]
struct Sequenced<'a> {
	#[serde(flatten)]
	frame: &'a ServerMessage,
	stream_seq: u64,
}

struct HeldFrame {
	seq: u64,
	text: String,
}

struct StreamState {
	id: String,
	session_id: String,
	last_seq: u64,
	/// The newest frames, oldest first, exactly as they were sent, up to
	/// `HOLD_MAX_BYTES` and `HOLD_MAX_FRAMES`.
	held: VecDeque<HeldFrame>,
	held_bytes: usize,
	/// The connection that owns the output (see `attach` and `own`). None until
	/// one does, and again once it is gone: frames are then only held.
	sink: Option<WsSender>,
	/// Connections whose latest bind of this session listed `elicitation_v1`.
	answering: Vec<WsSender>,
	/// Observer connections: they get a copy of every frame and never own the
	/// output.
	observers: Vec<WsSender>,
}

impl StreamState {
	fn hold(&mut self, seq: u64, text: String) {
		self.held_bytes += text.len();
		self.held.push_back(HeldFrame { seq, text });
		while (self.held_bytes > HOLD_MAX_BYTES || self.held.len() > HOLD_MAX_FRAMES) && self.held.len() > 1 {
			let dropped = self.held.pop_front().expect("more than one frame is held");
			self.held_bytes -= dropped.text.len();
		}
	}

	/// Every frame numbered above `after` is still held (vacuously so when
	/// `after` is the last one).
	fn holds_after(&self, after: u64) -> bool {
		let first_held = self.held.front().map_or(self.last_seq + 1, |frame| frame.seq);
		after <= self.last_seq && after >= first_held - 1
	}

	/// A send the sink refuses means its connection is gone; the frame stays
	/// held for the next bind.
	fn deliver(&mut self, text: &str) {
		if let Some(sink) = &self.sink {
			if sink.send(Message::text(text.to_owned())).is_err() {
				crate::log_debug!("Session stream {}: bound connection is gone", self.id);
				self.set_owner(None);
			}
		}
	}

	/// Observers get every numbered frame, wrapped around the text already
	/// serialized. A send an observer refuses means its connection is gone.
	fn deliver_observed(&mut self, text: &str) {
		if self.observers.is_empty() {
			return;
		}
		let envelope = observed_frame(&self.session_id, &self.id, text);
		self.observers
			.retain(|observer| observer.send(Message::text(envelope.clone())).is_ok());
	}

	/// The session's MCP elicitations are answered by the connection that owns
	/// its output, when it bound with `elicitation_v1`. With no live owner, open
	/// questions end as cancelled instead of holding their tool calls. Returns
	/// whether the new owner answers, so the caller can re-send open questions.
	fn set_owner(&mut self, sink: Option<WsSender>) -> bool {
		let answers = sink
			.as_ref()
			.is_some_and(|sink| self.answering.iter().any(|answering| answering.same_channel(sink)));
		if sink.is_none() {
			crate::mcp::elicitation::clear_session(&self.session_id);
		} else {
			crate::mcp::elicitation::set_answerer(
				&self.session_id,
				answers.then_some(crate::mcp::elicitation::Answerer::Client),
			);
		}
		self.sink = sink;
		answers
	}
}

/// The output of one session loaded in this process. Every frame the session
/// produces is numbered here (`stream_seq` 1, 2, 3 …), held for a resume, sent
/// to the connection that owns the output — whichever connection or task
/// produced it — and copied to every observer.
pub(crate) struct SessionStream(Mutex<StreamState>);

/// Emits directly into the replay stream; producer output must not accumulate
/// in a second channel while a model call is running.
#[derive(Clone)]
pub struct StreamSink {
	pub(crate) stream: Arc<SessionStream>,
	pub(crate) pending_work: bool,
}

impl StreamSink {
	pub(crate) fn send(&self, mut frame: ServerMessage) -> Result<()> {
		if let ServerMessage::Cost(cost) = &mut frame {
			if !self.pending_work {
				return Ok(());
			}
			cost.pending_work = true;
		}
		self.stream.emit(&frame)
	}
}

impl crate::session::output::OutputSink for StreamSink {
	fn emit(&self, frame: ServerMessage) {
		if let Err(error) = self.send(frame) {
			crate::log_error!("Failed to emit WebSocket session output: {}", error);
		}
	}
}

impl SessionStream {
	fn new(session_id: &str) -> Self {
		Self(Mutex::new(StreamState {
			id: uuid::Uuid::new_v4().simple().to_string(),
			session_id: session_id.to_string(),
			last_seq: 0,
			held: VecDeque::new(),
			held_bytes: 0,
			sink: None,
			answering: Vec::new(),
			observers: Vec::new(),
		}))
	}

	/// Number, hold and send one session frame. Never fails for want of a
	/// connection: an unbound stream only holds it.
	pub(crate) fn emit(&self, frame: &ServerMessage) -> Result<()> {
		let mut state = self.0.lock().unwrap();
		let seq = state.last_seq + 1;
		let text = serde_json::to_string(&Sequenced { frame, stream_seq: seq })?;
		state.last_seq = seq;
		state.deliver(&text);
		state.deliver_observed(&text);
		state.hold(seq, text);
		Ok(())
	}

	/// A client frame starts a turn: the connection that sent it reads that
	/// turn, as it does any later one until another connection takes over.
	pub(crate) fn own(&self, sink: &WsSender) {
		self.0.lock().unwrap().set_owner(Some(sink.clone()));
	}

	/// Bind `sink` to the stream. Sends the bind's ack — completed with the
	/// stream position and, for a resume, its outcome — then every frame the
	/// resume asked for, byte-identical, then `greeting`, all before any live
	/// frame can reach `sink`. When the binding connection owns the output and
	/// `answers_elicitation`, open questions are re-sent after all of that.
	///
	/// A bind that resumes always takes the output over. One that does not takes
	/// it only when no live connection owns it, so a client re-binding ahead of
	/// its next input never pulls a running turn away from the connection
	/// reading it.
	pub(crate) fn attach(
		&self,
		sink: &WsSender,
		mut ack: AckPayload,
		resume: Option<&StreamResume>,
		greeting: &ServerMessage,
		answers_elicitation: bool,
	) -> Result<()> {
		let (resend_questions, session_id) = {
			let mut state = self.0.lock().unwrap();
			let replay_after = resume
				.filter(|resume| resume.stream == state.id && state.holds_after(resume.after))
				.map(|resume| resume.after);
			ack.stream = Some(state.id.clone());
			ack.stream_seq = Some(state.last_seq);
			ack.resumed = resume.map(|_| replay_after.is_some());
			sink.send(Message::text(serde_json::to_string(&ServerMessage::Ack(ack))?))?;
			if let Some(after) = replay_after {
				for frame in state.held.iter().filter(|frame| frame.seq > after) {
					sink.send(Message::text(frame.text.clone()))?;
				}
			}
			sink.send(Message::text(serde_json::to_string(greeting)?))?;
			state.answering.retain(|answering| !answering.same_channel(sink));
			if answers_elicitation {
				state.answering.push(sink.clone());
			}
			let owns = resume.is_some() || state.sink.as_ref().is_none_or(|owner| owner.same_channel(sink));
			(owns && state.set_owner(Some(sink.clone())), state.session_id.clone())
		};
		// After the lock: re-sent questions travel the session's notification
		// channel, which emits on this stream.
		if resend_questions {
			crate::mcp::elicitation::resend_pending(&session_id);
		}
		Ok(())
	}

	fn detach(&self, sink: &WsSender) {
		let mut state = self.0.lock().unwrap();
		state.answering.retain(|answering| !answering.same_channel(sink));
		state.observers.retain(|observer| !observer.same_channel(sink));
		if state.sink.as_ref().is_some_and(|bound| bound.same_channel(sink)) {
			state.set_owner(None);
		}
	}

	/// A freshly minted stream: the observers connected so far learn of it
	/// before its first frame, and receive that frame like any later one.
	fn announce_open(&self, observers: &[WsSender]) {
		let mut state = self.0.lock().unwrap();
		let opened = ObserverMessage::StreamOpen {
			session_id: state.session_id.clone(),
			stream: state.id.clone(),
			last_seq: 0,
			gap: false,
		}
		.text();
		state.observers = observers
			.iter()
			.filter(|observer| observer.send(Message::text(opened.clone())).is_ok())
			.cloned()
			.collect();
	}

	/// The session was unloaded: its observers hear the stream is over.
	fn announce_closed(&self) {
		let state = self.0.lock().unwrap();
		let closed = ObserverMessage::StreamClosed {
			session_id: state.session_id.clone(),
			stream: state.id.clone(),
		}
		.text();
		for observer in &state.observers {
			if let Err(error) = observer.send(Message::text(closed.clone())) {
				crate::log_debug!("Session stream {}: observer is gone: {}", state.id, error);
			}
		}
	}

	/// Tell a new observer where the stream stands, replay what it has not read
	/// and register it for the live frames, all under the stream lock so no
	/// frame falls between the replay and the live part. A mark of this very
	/// stream limits the replay to the frames after it; any other point gets
	/// every held frame.
	///
	/// The replay is charged to `budget`, shared by all streams of one `observe`.
	/// A stream that does not fit replays only its newest frames, and `gap` says
	/// the older ones were not delivered — the same as frames lost to the hold.
	fn observe(&self, sink: &WsSender, mark: Option<&StreamResume>, budget: &mut ReplayBudget) -> Result<()> {
		let mut state = self.0.lock().unwrap();
		let after = mark.filter(|mark| mark.stream == state.id).map_or(0, |mark| mark.after);
		// Newest first, so what is cut is always the oldest part of the replay.
		let mut replay = Vec::new();
		for frame in state.held.iter().rev().take_while(|frame| frame.seq > after) {
			let envelope = observed_frame(&state.session_id, &state.id, &frame.text);
			if !budget.take(envelope.len()) {
				break;
			}
			replay.push((frame.seq, envelope));
		}
		let first_replayed = replay.last().map_or(state.last_seq + 1, |(seq, _)| *seq);
		let opened = ObserverMessage::StreamOpen {
			session_id: state.session_id.clone(),
			stream: state.id.clone(),
			last_seq: state.last_seq,
			gap: first_replayed > after.saturating_add(1),
		};
		sink.send(Message::text(opened.text()))?;
		for (_, envelope) in replay.into_iter().rev() {
			sink.send(Message::text(envelope))?;
		}
		state.observers.push(sink.clone());
		Ok(())
	}
}

/// The streams of every session loaded in this process, by session id, and the
/// observer connections that see all of them. A stream is minted the first
/// time its session is used and lives until the session is closed (unloaded),
/// so a re-load starts a new one.
#[derive(Clone, Default)]
pub(crate) struct Streams(Arc<Mutex<Registry>>);

/// One lock covers both halves, so an observer registers against exactly the
/// streams that exist and never misses one opening or closing meanwhile.
#[derive(Default)]
struct Registry {
	streams: HashMap<String, Arc<SessionStream>>,
	observers: Vec<WsSender>,
}

impl Streams {
	pub(crate) fn open(&self, session_id: &str) -> Arc<SessionStream> {
		let mut registry = self.0.lock().unwrap();
		if let Some(stream) = registry.streams.get(session_id) {
			return Arc::clone(stream);
		}
		let stream = Arc::new(SessionStream::new(session_id));
		stream.announce_open(&registry.observers);
		registry.streams.insert(session_id.to_string(), Arc::clone(&stream));
		stream
	}

	pub(crate) fn close(&self, session_id: &str) {
		if let Some(stream) = self.0.lock().unwrap().streams.remove(session_id) {
			stream.announce_closed();
		}
	}

	/// `sink` becomes an observer: `observing`, then every open stream with the
	/// replay `marks` ask for, within one `ReplayBudget` in all. Streams
	/// opened later announce themselves to it.
	pub(crate) fn observe(&self, sink: &WsSender, marks: &[ObserveMark]) -> Result<()> {
		let mut registry = self.0.lock().unwrap();
		let observing = ObserverMessage::Observing {
			capabilities: server_capabilities(),
		};
		sink.send(Message::text(observing.text()))?;
		let mut budget = ReplayBudget::observe();
		for (session_id, stream) in &registry.streams {
			let mark = marks
				.iter()
				.find(|mark| mark.session_id == *session_id)
				.map(|mark| &mark.resume);
			stream.observe(sink, mark, &mut budget)?;
		}
		registry.observers.push(sink.clone());
		Ok(())
	}

	/// A connection ended: no stream sends to it any more, and none keeps its
	/// writer alive.
	pub(crate) fn detach(&self, sink: &WsSender) {
		let mut registry = self.0.lock().unwrap();
		registry.observers.retain(|observer| !observer.same_channel(sink));
		for stream in registry.streams.values() {
			stream.detach(sink);
		}
	}
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
