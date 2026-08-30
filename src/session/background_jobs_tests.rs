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
fn test_acquire_and_release() {
	let mgr = BackgroundJobManager::new(2);
	assert!(mgr.try_acquire().is_ok());
	assert!(mgr.try_acquire().is_ok());
	assert!(mgr.try_acquire().is_err());
	// release decrements the counter; inbox push is a no-op (no inbox registered)
	mgr.release(CompletedJob {
		agent_name: "a".into(),
		output: "done".into(),
	});
	assert!(mgr.try_acquire().is_ok());
}

#[test]
fn test_active_count() {
	let mgr = BackgroundJobManager::new(10);
	assert_eq!(mgr.active_count(), 0);
	mgr.try_acquire().unwrap();
	mgr.try_acquire().unwrap();
	assert_eq!(mgr.active_count(), 2);
	mgr.release(CompletedJob {
		agent_name: "a".into(),
		output: "x".into(),
	});
	assert_eq!(mgr.active_count(), 1);
}
