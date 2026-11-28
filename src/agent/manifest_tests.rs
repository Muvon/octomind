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
fn private_roles_are_session_owned_and_promotion_does_not_change_the_manifest() {
	let a = "private-manifest-test-a";
	let b = "private-manifest-test-b";
	init(a);
	init(b);
	let tag = "dev:review";
	let grants = BTreeMap::from([("view".to_owned(), "files".to_owned())]);
	let m = Manifest::create(
		tag.into(),
		"Check API",
		"quotes: \" and newline\n",
		Some("my-model"),
		&grants,
	)
	.unwrap();
	let role: toml::Value = toml::from_str(&m.raw).unwrap();
	assert_eq!(role["roles"][0]["system"].as_str(), Some("quotes: \" and newline\n"));
	assert_eq!(role["roles"][0]["model"]["name"].as_str(), Some("my-model"));
	assert_eq!(role["roles"][0]["mcp"]["allowed_tools"][0].as_str(), Some("files:view"));
	register(a, m.clone()).unwrap();
	assert!(register(a, m.clone()).is_err());
	assert!(private_role(b, tag).is_err());
	assert_eq!(private_role(a, tag).unwrap().raw, m.raw);
	clear(a);
	clear(b);
	assert!(private_role(a, tag).is_err());
}

#[test]
fn publishing_a_role_never_overwrites_the_first_definition() {
	let dir = tempfile::tempdir().unwrap();
	let p = dir.path().join("agents/dev/review.toml");
	publish_new(&p, b"first").unwrap();
	assert!(publish_new(&p, b"second").is_err());
	assert_eq!(std::fs::read(&p).unwrap(), b"first");
	assert!(snapshot_path("tap-../../x").is_err());
}
