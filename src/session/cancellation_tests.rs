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
fn test_is_cancelled_detection() {
	assert!(is_cancelled(&anyhow::Error::new(Cancelled)));
	// Wrapped in context further up the stack still matches
	let wrapped = anyhow::Error::new(Cancelled).context("while calling provider");
	assert!(is_cancelled(&wrapped));
	assert!(!is_cancelled(&anyhow::anyhow!("some other failure")));
}

#[test]
fn test_session_cancellation_lifecycle() {
	let mut cancellation = SessionCancellation::new();
	assert!(!cancellation.is_cancelled());

	let old_rx = cancellation.new_operation();
	assert!(!*old_rx.borrow());

	cancellation.shutdown();
	assert!(cancellation.is_cancelled());
	assert!(*old_rx.borrow());

	// Reset starts a fresh operation; receivers of the old operation keep
	// their `true` so orphaned tasks always see the cancellation.
	cancellation.reset();
	assert!(!cancellation.is_cancelled());
	assert!(*old_rx.borrow());
}
