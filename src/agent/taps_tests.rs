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
fn test_github_url() {
	let tap = Tap {
		name: "muvon/tap".to_string(),
		local_path: None,
	};
	assert_eq!(tap.github_url(), "https://github.com/muvon/octomind-tap");

	// Names that are not user/repo fall back to being used verbatim
	let odd = Tap {
		name: "https://example.com/x.git".to_string(),
		local_path: None,
	};
	assert_eq!(odd.github_url(), "https://example.com/x.git");
}

#[test]
fn test_parse_tap_arg() {
	let tap = parse_tap_arg("user/repo").expect("valid GitHub tap");
	assert_eq!(tap.name, "user/repo");
	assert!(tap.local_path.is_none());

	let local = parse_tap_arg("user/repo /some/path").expect("valid local tap");
	assert_eq!(local.name, "user/repo");
	assert_eq!(local.local_path.as_deref(), Some("/some/path"));

	// Not user/repo format
	assert!(parse_tap_arg("plain").is_err());
	assert!(parse_tap_arg("a/b/c").is_err());
}

#[test]
fn test_expand_path() {
	let home = dirs::home_dir().expect("home dir in test env");
	assert_eq!(expand_path("~/x").expect("tilde"), home.join("x"));
	assert_eq!(
		expand_path("/abs/path").expect("absolute"),
		PathBuf::from("/abs/path")
	);
	let cwd = std::env::current_dir().expect("cwd");
	assert_eq!(expand_path("./rel").expect("relative"), cwd.join("rel"));
}
