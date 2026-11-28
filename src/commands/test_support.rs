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

//! Test helpers shared by the `octomind` binary's command tests.
//!
//! The binary cannot see the library's `#[cfg(test)]` items, so this mirrors
//! the library's `session::chat::test_support` for the few helpers the command
//! tests need.

/// Tap commands normally run after config loading installs the registry.
pub(crate) fn tap_sandbox(tag: &str) -> std::path::PathBuf {
	let config: octomind::Config =
		toml::from_str(&crate::commands::template_with_assistant_role::template_with_assistant_role())
			.expect("parse default config template");
	octomind::agent::taps::set_built_in_tap(&config.registry);
	let dir = std::env::temp_dir().join(format!("octomind-{tag}-{}", std::process::id()));
	if dir.exists() {
		std::fs::remove_dir_all(&dir).expect("clear stale sandbox data dir");
	}
	std::fs::create_dir_all(&dir).expect("create sandbox data dir");
	dir
}

/// Snapshot environment variables and restore them on drop, so a failed assert
/// cannot leak a sandboxed data dir or API URL into the next test.
pub(crate) struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
	pub(crate) fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}

impl Drop for EnvGuard {
	fn drop(&mut self) {
		for (key, saved) in &self.0 {
			match saved {
				Some(v) => std::env::set_var(key, v),
				None => std::env::remove_var(key),
			}
		}
	}
}
