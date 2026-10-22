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
fn run_dir_is_writable_so_send_reaches_a_sandboxed_session() {
	let root = tempfile::tempdir().expect("temp root");
	let cwd = root.path().join("project");
	let run_dir = root.path().join("run");
	let xdg_data_home = root.path().join("share");
	let octolib_config = root.path().join("octolib");
	for path in [&cwd, &run_dir, &xdg_data_home, &octolib_config] {
		std::fs::create_dir_all(path).expect("create dir");
	}

	let paths = writable_paths(&cwd, &xdg_data_home, &octolib_config, &run_dir);

	assert_eq!(
		paths,
		vec![
			cwd.as_path(),
			run_dir.as_path(),
			xdg_data_home.as_path(),
			octolib_config.as_path()
		]
	);
}

#[test]
fn missing_optional_roots_are_skipped() {
	let root = tempfile::tempdir().expect("temp root");
	let cwd = root.path().join("project");
	let run_dir = root.path().join("run");
	let missing_share = root.path().join("no-share");
	let missing_octolib = root.path().join("no-octolib");
	std::fs::create_dir_all(&cwd).expect("create cwd");
	std::fs::create_dir_all(&run_dir).expect("create run dir");

	let paths = writable_paths(&cwd, &missing_share, &missing_octolib, &run_dir);

	assert_eq!(paths, vec![cwd.as_path(), run_dir.as_path()]);
}
