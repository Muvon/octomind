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
fn split_agent_tag_accepts_domain_spec() {
	assert_eq!(split_agent_tag("dev:rust").unwrap(), ("dev", "rust"));
	assert!(split_agent_tag("dev").is_err());
	assert!(split_agent_tag("dev:").is_err());
	assert!(split_agent_tag("a:b:c").is_err());
}

#[test]
fn build_tokens_defaults_domain_to_repo_name() {
	let tokens = build_tokens("acme", "team", None, Some("assistant")).unwrap();
	assert_eq!(tokens["__TAP_ID__"], "acme/team");
	assert_eq!(tokens["__TAP_OWNER__"], "acme");
	assert_eq!(tokens["__TAP_NAME__"], "team");
	assert_eq!(tokens["__TAP_REPOSITORY__"], "octomind-team");
	assert_eq!(tokens["__AGENT_DOMAIN__"], "team");
	assert_eq!(tokens["__AGENT_SPEC__"], "assistant");
	assert_eq!(tokens["__YEAR__"].len(), 4);
}

#[test]
fn build_tokens_honors_agent_override() {
	let tokens = build_tokens("acme", "team", Some("legal:contracts"), Some("assistant")).unwrap();
	assert_eq!(tokens["__AGENT_DOMAIN__"], "legal");
	assert_eq!(tokens["__AGENT_SPEC__"], "contracts");
}

#[test]
fn build_tokens_requires_spec_default_without_override() {
	assert!(build_tokens("acme", "team", None, None).is_err());
}

#[test]
fn render_replaces_tokens_and_preserves_runtime_placeholders() {
	let tokens = build_tokens("acme", "team", None, Some("assistant")).unwrap();
	let out = render("run __AGENT_DOMAIN__:__AGENT_SPEC__ in {{CWD}}", &tokens);
	assert_eq!(out, "run team:assistant in {{CWD}}");
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

fn leftover_regex() -> Regex {
	Regex::new(TOKEN_PATTERN).unwrap()
}

#[test]
fn render_tree_renders_paths_and_contents() {
	let src = tempfile::tempdir().unwrap();
	let dest = tempfile::tempdir().unwrap();
	let agent_dir = src.path().join("agents/__AGENT_DOMAIN__");
	std::fs::create_dir_all(&agent_dir).unwrap();
	std::fs::write(
		agent_dir.join("__AGENT_SPEC__.toml"),
		"# __TAP_ID__\nwelcome = \"ready in {{CWD}}\"\n",
	)
	.unwrap();

	let tokens = build_tokens("acme", "team", None, Some("assistant")).unwrap();
	render_tree(src.path(), dest.path(), &tokens, &leftover_regex()).unwrap();

	let rendered = dest.path().join("agents/team/assistant.toml");
	let content = std::fs::read_to_string(&rendered).unwrap();
	assert_eq!(content, "# acme/team\nwelcome = \"ready in {{CWD}}\"\n");
}

#[test]
fn render_tree_fails_on_unresolved_token() {
	let src = tempfile::tempdir().unwrap();
	let dest = tempfile::tempdir().unwrap();
	std::fs::write(src.path().join("file.md"), "leftover __UNKNOWN_TOKEN__").unwrap();

	let tokens = build_tokens("acme", "team", None, Some("assistant")).unwrap();
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

	let tokens = build_tokens("acme", "team", None, Some("assistant")).unwrap();
	render_tree(src.path(), dest.path(), &tokens, &leftover_regex()).unwrap();

	let mode = std::fs::metadata(dest.path().join("check.sh"))
		.unwrap()
		.permissions()
		.mode();
	assert_eq!(mode & 0o111, 0o111);
}
