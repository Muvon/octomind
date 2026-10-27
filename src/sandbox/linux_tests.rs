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
fn run_dir_and_npm_cache_are_writable() {
	let root = tempfile::tempdir().expect("temp root");
	let cwd = root.path().join("project");
	let run_dir = root.path().join("run");
	let xdg_data_home = root.path().join("share");
	let octolib_config = root.path().join("octolib");
	let npm_cache = root.path().join(".npm");
	for path in [&cwd, &run_dir, &xdg_data_home, &octolib_config, &npm_cache] {
		std::fs::create_dir_all(path).expect("create dir");
	}

	let optional = [xdg_data_home.as_path(), octolib_config.as_path(), npm_cache.as_path()];
	let paths = writable_paths(&cwd, &run_dir, &optional);

	assert_eq!(
		paths,
		vec![
			cwd.as_path(),
			run_dir.as_path(),
			xdg_data_home.as_path(),
			octolib_config.as_path(),
			npm_cache.as_path()
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
	let missing_npm = root.path().join("no-npm");
	std::fs::create_dir_all(&cwd).expect("create cwd");
	std::fs::create_dir_all(&run_dir).expect("create run dir");

	let optional = [
		missing_share.as_path(),
		missing_octolib.as_path(),
		missing_npm.as_path(),
	];
	let paths = writable_paths(&cwd, &run_dir, &optional);

	assert_eq!(paths, vec![cwd.as_path(), run_dir.as_path()]);
}

/// Set in the re-executed test binary: `restrict_self()` cannot be undone, so
/// the sandbox is applied in a child process, never in the test runner itself.
const SANDBOX_CHILD_CWD: &str = "OCTOMIND_SANDBOX_TEST_CWD";

#[test]
fn a_sandboxed_shell_writes_tmp_and_dev_null_but_nothing_outside_its_roots() {
	if let Some(cwd) = std::env::var_os(SANDBOX_CHILD_CWD) {
		run_sandboxed_checks(Path::new(&cwd));
		return;
	}
	// Without the Landlock LSM (kernel < 5.13 or not enabled) nothing is
	// enforced, so the denial check below cannot hold.
	let landlock_enabled = std::fs::read_to_string("/sys/kernel/security/lsm")
		.is_ok_and(|lsms| lsms.trim().split(',').any(|lsm| lsm == "landlock"));
	if !landlock_enabled {
		eprintln!("skipped: Landlock is not enabled on this kernel");
		return;
	}

	let root = tempfile::tempdir().expect("temp root");
	let cwd = root.path().join("project");
	std::fs::create_dir_all(&cwd).expect("create cwd");
	let child = std::process::Command::new(std::env::current_exe().expect("test binary path"))
		.args([
			"a_sandboxed_shell_writes_tmp_and_dev_null_but_nothing_outside_its_roots",
			"--nocapture",
		])
		.env(SANDBOX_CHILD_CWD, &cwd)
		.env("OCTOMIND_DATA_DIR", root.path().join("data"))
		.output()
		.expect("run the sandboxed child");
	let stdout = String::from_utf8_lossy(&child.stdout);
	assert!(
		child.status.success(),
		"sandboxed child failed:\n{stdout}{}",
		String::from_utf8_lossy(&child.stderr)
	);
	// A filter that matched nothing would also exit 0.
	assert!(stdout.contains("1 passed"), "the child ran the checks:\n{stdout}");
}

fn run_sandboxed_checks(cwd: &Path) {
	apply(cwd).expect("apply the sandbox");

	let shell = std::process::Command::new("sh")
		.args(["-c", "echo x >/dev/null && f=$(mktemp) && rm \"$f\""])
		.current_dir(cwd)
		.output()
		.expect("run sh");
	assert!(
		shell.status.success(),
		"`2>/dev/null` and `mktemp` work: {}",
		String::from_utf8_lossy(&shell.stderr)
	);
	std::fs::write(cwd.join("inside"), "x").expect("cwd stays writable");

	// The crate root is outside every writable root (it is not under /tmp on any CI runner).
	let outside = Path::new(env!("CARGO_MANIFEST_DIR")).join("sandbox-write-probe");
	let written = std::fs::write(&outside, "x");
	if written.is_ok() {
		std::fs::remove_file(&outside).expect("remove the probe");
	}
	assert!(written.is_err(), "writes outside the roots are denied");
}
