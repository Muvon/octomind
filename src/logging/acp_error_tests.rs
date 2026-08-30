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
use tempfile::tempdir;

#[test]
fn test_acp_error_sink_path() {
	let dir = tempdir().unwrap();
	let logs_dir = dir.path().join("logs");
	std::fs::create_dir_all(&logs_dir).unwrap();

	let path = logs_dir.join("acp-errors.jsonl");
	let sink = AcpErrorSink::new(path.clone()).unwrap();

	sink.log_error_simple("Test error").unwrap();

	let content = std::fs::read_to_string(&path).unwrap();
	assert!(content.contains("Test error"));
}
