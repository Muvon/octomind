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

#[test]
fn id_format_is_stable() {
	let id = generate_id("developer:general");
	assert!(id.starts_with("tap-developer-general-"));
	// 4 (tap-) + 17 (developer-general-) + 6 (hex) = 27
	assert_eq!(id.len(), "tap-developer-general-".len() + 6);
}

#[test]
fn status_strings_are_stable() {
	assert_eq!(TapJobStatus::Running.as_str(), "running");
	assert_eq!(TapJobStatus::Done.as_str(), "done");
	assert_eq!(TapJobStatus::Failed.as_str(), "failed");
	assert_eq!(TapJobStatus::Cancelled.as_str(), "cancelled");
}
