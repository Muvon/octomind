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

//! Session sharing — gzip the JSONL session log, POST it to octomind.run's
//! `/api/share` endpoint, get back a permanent `octomind.run/r/<id>` URL.
//!
//! Layout:
//!   - `upload` — gzip + HTTP POST to the share API, returns `{ id, url }`.
//!
//! Future siblings: `bridge` (localhost SSE for `/analyze`), `open` (cross-
//! platform URL launcher — currently we use the `open` crate directly from
//! the command handler).

pub mod bridge;
pub mod upload;

pub use bridge::{start_for_session as start_bridge, BridgeInfo};
pub use upload::{share_session, web_host, ShareResult};
