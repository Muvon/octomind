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
fn test_read_text_files() {
	let tmp = tempfile::tempdir().expect("tempdir");
	std::fs::write(tmp.path().join("a.txt"), "aaaaa").expect("write a");
	std::fs::write(tmp.path().join("b.txt"), "bbbbb").expect("write b");

	// Generous cap → both files whole (directory order is unspecified)
	let all = read_text_files(tmp.path(), 100);
	assert_eq!(all.len(), 2);
	assert_eq!(all.iter().map(String::len).sum::<usize>(), 10);

	// Cap mid-second-file → last file truncated to fit exactly
	let capped = read_text_files(tmp.path(), 7);
	assert_eq!(capped.len(), 2);
	assert_eq!(capped.iter().map(String::len).sum::<usize>(), 7);

	// Cap consumed by the first file → second never read
	let one = read_text_files(tmp.path(), 5);
	assert_eq!(one.len(), 1);
	assert_eq!(one[0].len(), 5);

	// Missing directory → empty
	assert!(read_text_files(&tmp.path().join("nope"), 100).is_empty());
}

#[test]
fn test_read_text_files_utf8_boundary() {
	let tmp = tempfile::tempdir().expect("tempdir");
	// 5 × 'é' = 10 bytes; a 3-byte cap must floor to the 2-byte boundary
	std::fs::write(tmp.path().join("u.txt"), "ééééé").expect("write utf8");
	let out = read_text_files(tmp.path(), 3);
	assert_eq!(out.len(), 1);
	assert_eq!(out[0], "é");
}
