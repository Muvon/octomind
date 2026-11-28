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
fn split_tap_id_accepts_owner_name() {
	assert_eq!(split_tap_id("acme/team").unwrap(), ("acme", "team"));
}

#[test]
fn split_tap_id_rejects_bad_shapes() {
	for bad in ["acme", "acme/", "/team", "a/b/c", ""] {
		assert!(split_tap_id(bad).is_err(), "should reject {:?}", bad);
	}
}

#[test]
fn parse_agent_name_accepts_a_namespace_name() {
	assert_eq!(parse_agent_name("legal").unwrap(), "legal");
	assert_eq!(parse_agent_name("legal-v2").unwrap(), "legal-v2");
}

#[test]
fn parse_agent_name_rejects_a_colon_because_the_starter_is_a_namespace() {
	let err = parse_agent_name("legal:chief").unwrap_err().to_string();
	assert!(err.contains("lowercase letters, digits and hyphens"), "{err}");
}

#[test]
fn parse_agent_name_rejects_names_a_tag_cannot_have() {
	for bad in ["", "Legal", "my_agent", "a b", "agent.v2", "2fast", "-dash"] {
		assert!(parse_agent_name(bad).is_err(), "should reject {:?}", bad);
	}
}

#[test]
fn build_tokens_defaults_the_agent_to_the_repo_name() {
	let tokens = build_tokens("acme", "team", None).unwrap();
	assert_eq!(tokens["__TAP_ID__"], "acme/team");
	assert_eq!(tokens["__TAP_OWNER__"], "acme");
	assert_eq!(tokens["__TAP_NAME__"], "team");
	assert_eq!(tokens["__TAP_REPOSITORY__"], "octomind-team");
	assert_eq!(tokens["__AGENT_NAME__"], "team");
	assert!(!tokens.contains_key("__AGENT_SPEC__"));
	assert_eq!(tokens["__YEAR__"].len(), 4);
}

#[test]
fn build_tokens_agent_names_the_starter_namespace() {
	let tokens = build_tokens("acme", "team", Some("legal")).unwrap();
	assert_eq!(tokens["__AGENT_NAME__"], "legal");
	assert!(build_tokens("acme", "team", Some("legal:contracts")).is_err());
}

#[test]
fn render_replaces_tokens_and_preserves_runtime_placeholders() {
	let tokens = build_tokens("acme", "team", None).unwrap();
	let out = render("run __AGENT_NAME__ in {{CWD}}", &tokens);
	assert_eq!(out, "run team in {{CWD}}");
}

#[test]
fn ensure_empty_dest_refuses_non_empty_dir() {
	let dir = tempfile::tempdir().unwrap();
	std::fs::write(dir.path().join("existing.txt"), "x").unwrap();
	assert!(ensure_empty_dest(dir.path()).is_err());
}

#[test]
fn ensure_empty_dest_accepts_missing_and_empty() {
	let dir = tempfile::tempdir().unwrap();
	assert!(ensure_empty_dest(dir.path()).is_ok());
	let missing = dir.path().join("new");
	assert!(ensure_empty_dest(&missing).is_ok());
	assert!(missing.is_dir());
}

#[test]
fn the_token_list_names_the_agent_by_name() {
	let tokens = build_tokens("acme", "team", None).unwrap();
	let mut names: Vec<&str> = tokens.keys().map(String::as_str).collect();
	names.sort_unstable();
	assert!(names.contains(&"__AGENT_NAME__"), "{names:?}");
	assert!(!names.iter().any(|name| name.contains("DOMAIN")), "{names:?}");
}

fn leftover_regex() -> Regex {
	Regex::new(TOKEN_PATTERN).unwrap()
}

#[test]
fn render_tree_renders_paths_and_contents() {
	let src = tempfile::tempdir().unwrap();
	let dest = tempfile::tempdir().unwrap();
	let agent_dir = src.path().join("agents").join("__AGENT_NAME__");
	std::fs::create_dir_all(&agent_dir).unwrap();
	std::fs::write(
		agent_dir.join("default.toml"),
		"# __TAP_ID__\nwelcome = \"ready in {{CWD}}\"\n",
	)
	.unwrap();

	let tokens = build_tokens("acme", "team", None).unwrap();
	render_tree(src.path(), dest.path(), &tokens, &leftover_regex()).unwrap();

	let rendered = dest.path().join("agents/team/default.toml");
	let content = std::fs::read_to_string(&rendered).unwrap();
	assert_eq!(content, "# acme/team\nwelcome = \"ready in {{CWD}}\"\n");
}

#[test]
fn render_tree_renders_the_agent_directory_with_its_owned_skills() {
	let src = tempfile::tempdir().unwrap();
	let dest = tempfile::tempdir().unwrap();
	let skill_dir = src.path().join("agents/__AGENT_NAME__/skills/__AGENT_NAME__-notes");
	std::fs::create_dir_all(&skill_dir).unwrap();
	std::fs::write(skill_dir.join("SKILL.md"), "name: __AGENT_NAME__-notes\n").unwrap();
	std::fs::write(src.path().join("agents/__AGENT_NAME__/default.toml"), "# __TAP_ID__\n").unwrap();

	let tokens = build_tokens("acme", "team", Some("legal")).unwrap();
	render_tree(src.path(), dest.path(), &tokens, &leftover_regex()).unwrap();

	assert!(dest.path().join("agents/legal/default.toml").is_file());
	assert_eq!(
		std::fs::read_to_string(dest.path().join("agents/legal/skills/legal-notes/SKILL.md")).unwrap(),
		"name: legal-notes\n"
	);
	assert!(!dest.path().join("agents/__AGENT_NAME__").exists());
}

#[test]
fn render_tree_fails_on_unresolved_token_in_a_directory_name() {
	let src = tempfile::tempdir().unwrap();
	let dest = tempfile::tempdir().unwrap();
	std::fs::create_dir_all(src.path().join("agents/__UNKNOWN_TOKEN__")).unwrap();

	let tokens = build_tokens("acme", "team", None).unwrap();
	let err = render_tree(src.path(), dest.path(), &tokens, &leftover_regex()).unwrap_err();
	assert!(err.to_string().contains("__UNKNOWN_TOKEN__"), "{err}");
}

#[test]
fn render_tree_fails_on_unresolved_token() {
	let src = tempfile::tempdir().unwrap();
	let dest = tempfile::tempdir().unwrap();
	std::fs::write(src.path().join("file.md"), "leftover __UNKNOWN_TOKEN__").unwrap();

	let tokens = build_tokens("acme", "team", None).unwrap();
	let err = render_tree(src.path(), dest.path(), &tokens, &leftover_regex()).unwrap_err();
	assert!(err.to_string().contains("__UNKNOWN_TOKEN__"));
}

#[cfg(unix)]
#[test]
fn render_tree_preserves_executable_bit() {
	use std::os::unix::fs::PermissionsExt;
	let src = tempfile::tempdir().unwrap();
	let dest = tempfile::tempdir().unwrap();
	let script = src.path().join("check.sh");
	std::fs::write(&script, "#!/bin/sh\n").unwrap();
	std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

	let tokens = build_tokens("acme", "team", None).unwrap();
	render_tree(src.path(), dest.path(), &tokens, &leftover_regex()).unwrap();

	let mode = std::fs::metadata(dest.path().join("check.sh"))
		.unwrap()
		.permissions()
		.mode();
	assert_eq!(mode & 0o111, 0o111);
}
