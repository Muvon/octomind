// Copyright 2025 Muvon Un Limited
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

// Utility functions for session display

// Utility function to format numbers in a human-readable format
pub fn format_number(number: u64) -> String {
	if number == 0 {
		return "0".to_string();
	}

	if number < 1_000 {
		number.to_string()
	} else if number < 10_000 {
		// For numbers 1K-9.99K, show one decimal place
		let k = number as f64 / 1_000.0;
		if k.fract() == 0.0 {
			format!("{}K", k as u64)
		} else {
			format!("{:.1}K", k)
		}
	} else if number < 1_000_000 {
		// For numbers 10K-999K, show whole K
		format!("{}K", number / 1_000)
	} else if number < 10_000_000 {
		// For numbers 1M-9.99M, show one decimal place
		let m = number as f64 / 1_000_000.0;
		if m.fract() == 0.0 {
			format!("{}M", m as u64)
		} else {
			format!("{:.1}M", m)
		}
	} else if number < 1_000_000_000 {
		// For numbers 10M-999M, show whole M
		format!("{}M", number / 1_000_000)
	} else {
		// For numbers 1B+, show one decimal place
		let b = number as f64 / 1_000_000_000.0;
		if b.fract() == 0.0 {
			format!("{}B", b as u64)
		} else {
			format!("{:.1}B", b)
		}
	}
}

// Utility function to format time in a human-readable format
pub fn format_duration(milliseconds: u64) -> String {
	if milliseconds == 0 {
		return "0ms".to_string();
	}

	let ms = milliseconds % 1000;
	let seconds = (milliseconds / 1000) % 60;
	let minutes = (milliseconds / 60000) % 60;
	let hours = milliseconds / 3600000;

	let mut parts = Vec::new();

	if hours > 0 {
		parts.push(format!("{}h", hours));
	}
	if minutes > 0 {
		parts.push(format!("{}m", minutes));
	}
	if seconds > 0 {
		parts.push(format!("{}s", seconds));
	}
	if ms > 0 || parts.is_empty() {
		if parts.is_empty() {
			parts.push(format!("{}ms", ms));
		} else if ms >= 100 {
			// Only show milliseconds if >= 100ms when other units are present
			parts.push(format!("{}ms", ms));
		}
	}

	parts.join(" ")
}