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

// Session output streams: numbered, buffered, and routed to the latest bind.

use super::protocol::{AckPayload, ServerMessage, StreamResume};
use super::server::WsSender;
use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio_tungstenite::tungstenite::Message;

/// Held frames per stream, whichever bound is hit first. Frames are whole
/// messages (never token deltas), so a turn rarely nears the count; the byte
/// bound is what large tool results run into.
const MAX_HELD_FRAMES: usize = 4096;
const MAX_HELD_BYTES: usize = 8 * 1024 * 1024;

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
	last_seq: u64,
	/// Frames of the turn running now and of the one before it, oldest first,
	/// exactly as they were sent.
	held: VecDeque<HeldFrame>,
	held_bytes: usize,
	/// First `stream_seq` of the latest turn; frames before the turn ahead of
	/// it are dropped when the next one begins.
	turn_start: u64,
	/// The connection that owns the output (see `attach` and `own`). None until
	/// one does, and again once it is gone: frames are then only held.
	sink: Option<WsSender>,
}

impl StreamState {
	fn hold(&mut self, seq: u64, text: String) {
		self.held_bytes += text.len();
		self.held.push_back(HeldFrame { seq, text });
		while self.held.len() > MAX_HELD_FRAMES || self.held_bytes > MAX_HELD_BYTES {
			self.drop_oldest();
		}
	}

	fn drop_oldest(&mut self) {
		if let Some(frame) = self.held.pop_front() {
			self.held_bytes -= frame.text.len();
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
				self.sink = None;
			}
		}
	}
}

/// The output of one session loaded in this process. Every frame the session
/// produces is numbered here (`stream_seq` 1, 2, 3 …), held for a resume, and
/// sent to the connection that owns the output — whichever connection or task
/// produced it.
pub(crate) struct SessionStream(Mutex<StreamState>);

impl SessionStream {
	fn new() -> Self {
		Self(Mutex::new(StreamState {
			id: uuid::Uuid::new_v4().simple().to_string(),
			last_seq: 0,
			held: VecDeque::new(),
			held_bytes: 0,
			turn_start: 1,
			sink: None,
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
		state.hold(seq, text);
		Ok(())
	}

	/// A new turn starts: what came before the previous turn is no longer
	/// resumable.
	pub(crate) fn begin_turn(&self) {
		let mut state = self.0.lock().unwrap();
		let previous_start = state.turn_start;
		while state.held.front().is_some_and(|frame| frame.seq < previous_start) {
			state.drop_oldest();
		}
		state.turn_start = state.last_seq + 1;
	}

	/// A client frame starts a turn: the connection that sent it reads that
	/// turn, as it does any later one until another connection takes over.
	pub(crate) fn own(&self, sink: &WsSender) {
		self.0.lock().unwrap().sink = Some(sink.clone());
	}

	/// Bind `sink` to the stream. Sends the bind's ack — completed with the
	/// stream position and, for a resume, its outcome — then every frame the
	/// resume asked for, byte-identical, then `greeting`, all before any live
	/// frame can reach `sink`.
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
	) -> Result<()> {
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
		if resume.is_some() || state.sink.is_none() {
			state.sink = Some(sink.clone());
		}
		Ok(())
	}

	fn detach(&self, sink: &WsSender) {
		let mut state = self.0.lock().unwrap();
		if state.sink.as_ref().is_some_and(|bound| bound.same_channel(sink)) {
			state.sink = None;
		}
	}
}

/// The streams of every session loaded in this process, by session id. A
/// stream is minted the first time its session is used and lives until the
/// session is closed (unloaded), so a re-load starts a new one.
#[derive(Clone, Default)]
pub(crate) struct Streams(Arc<Mutex<HashMap<String, Arc<SessionStream>>>>);

impl Streams {
	pub(crate) fn open(&self, session_id: &str) -> Arc<SessionStream> {
		let mut streams = self.0.lock().unwrap();
		Arc::clone(
			streams
				.entry(session_id.to_string())
				.or_insert_with(|| Arc::new(SessionStream::new())),
		)
	}

	pub(crate) fn close(&self, session_id: &str) {
		self.0.lock().unwrap().remove(session_id);
	}

	/// A connection ended: no stream sends to it any more, and none keeps its
	/// writer alive.
	pub(crate) fn detach(&self, sink: &WsSender) {
		for stream in self.0.lock().unwrap().values() {
			stream.detach(sink);
		}
	}
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
