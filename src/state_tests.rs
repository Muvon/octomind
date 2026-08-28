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
fn index_state_default_is_zeroed_and_empty() {
	let state = IndexState::default();
	assert_eq!(state.current_directory, PathBuf::new());
	assert_eq!(state.indexed_files, 0);
	assert_eq!(state.embedding_calls, 0);
	assert!(!state.indexing_complete);
	assert!(state.status_message.is_empty());
	assert!(!state.force_reindex);
	assert!(!state.graphrag_enabled);
	assert_eq!(state.graphrag_blocks, 0);
}

#[test]
fn create_shared_state_is_usable_read_write() {
	let shared = create_shared_state();
	{
		let state = shared.read();
		assert_eq!(state.indexed_files, 0);
		assert!(!state.indexing_complete);
	}
	{
		let mut state = shared.write();
		state.indexed_files = 42;
		state.embedding_calls = 7;
		state.indexing_complete = true;
		state.status_message = "indexing complete".to_string();
	}
	let state = shared.read();
	assert_eq!(state.indexed_files, 42);
	assert_eq!(state.embedding_calls, 7);
	assert!(state.indexing_complete);
	assert_eq!(state.status_message, "indexing complete");
}
