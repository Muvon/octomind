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

// Shared time utilities

use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the current Unix timestamp in seconds.
pub fn now_secs() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs()
}

/// Compact elapsed-duration label: `"0m48s"`, `"3m12s"`, `"1h05m"`, `"2d03h"`.
/// Used by the `/agents` panel to show how long a run has been going.
pub fn format_duration_short(secs: u64) -> String {
	let days = secs / 86_400;
	let hours = (secs % 86_400) / 3_600;
	let mins = (secs % 3_600) / 60;
	let s = secs % 60;
	if days > 0 {
		format!("{days}d{hours:02}h")
	} else if hours > 0 {
		format!("{hours}h{mins:02}m")
	} else {
		format!("{mins}m{s:02}s")
	}
}

/// Relative "time since" label: `"just now"`, `"45s ago"`, `"2m ago"`, `"3h ago"`, `"5d ago"`.
/// Used by the `/agents` panel for finished runs.
pub fn format_ago(secs_ago: u64) -> String {
	if secs_ago < 5 {
		"just now".to_string()
	} else if secs_ago < 60 {
		format!("{secs_ago}s ago")
	} else if secs_ago < 3_600 {
		format!("{}m ago", secs_ago / 60)
	} else if secs_ago < 86_400 {
		format!("{}h ago", secs_ago / 3_600)
	} else {
		format!("{}d ago", secs_ago / 86_400)
	}
}
