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

//! Every `.rs` file under `src/` and `tests/` must start with the Apache 2.0 header from AGENTS.md.

use std::fs;
use std::path::{Path, PathBuf};

const HEADER: [&str; 13] = [
	"// Copyright 2026 Muvon Un Limited",
	"//",
	"// Licensed under the Apache License, Version 2.0 (the \"License\");",
	"// you may not use this file except in compliance with the License.",
	"// You may obtain a copy of the License at",
	"//",
	"//     http://www.apache.org/licenses/LICENSE-2.0",
	"//",
	"// Unless required by applicable law or agreed to in writing, software",
	"// distributed under the License is distributed on an \"AS IS\" BASIS,",
	"// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.",
	"// See the License for the specific language governing permissions and",
	"// limitations under the License.",
];

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
	let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {}", dir.display(), e));
	for entry in entries {
		let path = entry
			.unwrap_or_else(|e| panic!("read entry in {}: {}", dir.display(), e))
			.path();
		if path.is_dir() {
			collect_rs_files(&path, out);
		} else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
			out.push(path);
		}
	}
}

#[test]
fn every_rs_file_has_the_apache_header() {
	let root = Path::new(env!("CARGO_MANIFEST_DIR"));
	let mut files = Vec::new();
	for dir in ["src", "tests"] {
		collect_rs_files(&root.join(dir), &mut files);
	}
	assert!(!files.is_empty(), "no .rs files found under src/ or tests/");

	// Compare the whole header: a file can keep the copyright line and still lose the license text.
	let mut missing = Vec::new();
	for file in files {
		let text = fs::read_to_string(&file).unwrap_or_else(|e| panic!("read {}: {}", file.display(), e));
		if !text.lines().take(HEADER.len()).eq(HEADER) {
			missing.push(file.display().to_string());
		}
	}
	assert!(
		missing.is_empty(),
		"files missing the Apache header:\n{}",
		missing.join("\n")
	);
}
