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
fn remote_skill_verifies_every_frontmatter_field() {
	let content = "---\r\nname: release\r\ndescription: >-\r\n  Publish a release\r\nmetadata:\r\n  owner: team\r\n---\r\nSteps\r\n";
	let expected =
		SkillFrontmatter::new("release", "Publish a release").with_field("metadata", json!({"owner": "team"}));
	assert_eq!(verified_body(content, &expected).unwrap(), "Steps\r\n");
	assert!(verified_body(&content.replace("owner: team", "owner: other"), &expected).is_err());
	assert!(verified_body(&content.replace("---\r\nSteps", "--\r\nSteps"), &expected).is_err());
}

#[test]
fn remote_skill_reference_preserves_origin_and_does_not_shadow_local_names() {
	let uri = "skill://acme/release/SKILL.md";
	assert_eq!(reference(uri), Some((None, uri)));
	assert_eq!(
		reference("docs:skill://acme/release/SKILL.md"),
		Some((Some("docs"), uri))
	);
	assert_eq!(reference("release"), None);
}
