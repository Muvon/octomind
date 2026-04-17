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

#[cfg(test)]
mod tests {
	use crate::mcp::core::skill::{
		build_resource_catalog, has_activate_script, has_validate_script, parse_skill_meta,
	};
	use std::fs;

	// ---------------------------------------------------------------------------
	// parse_skill_meta
	// ---------------------------------------------------------------------------

	#[test]
	fn test_parse_skill_meta_valid_minimal() {
		let content = "---\nname: my-skill\ndescription: Does something useful\n---\n\n# Body";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.name, "my-skill");
		assert_eq!(meta.description, "Does something useful");
		assert!(meta.compatibility.is_none());
		assert!(meta.license.is_none());
		assert!(meta.allowed_tools.is_empty());
		assert!(meta.capabilities.is_empty());
		assert!(meta.domains.is_empty());
	}

	#[test]
	fn test_parse_skill_meta_all_fields() {
		let content = "---\nname: full-skill\ndescription: A complete skill\ncompatibility: developer\nlicense: MIT\nallowed-tools: shell view text_editor\n---\n\n# Instructions\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.name, "full-skill");
		assert_eq!(meta.description, "A complete skill");
		assert_eq!(meta.compatibility.as_deref(), Some("developer"));
		assert_eq!(meta.license.as_deref(), Some("MIT"));
		assert_eq!(meta.allowed_tools, vec!["shell", "view", "text_editor"]);
	}

	#[test]
	fn test_parse_skill_meta_quoted_values() {
		let content = "---\nname: \"quoted-skill\"\ndescription: 'single quoted'\n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.name, "quoted-skill");
		assert_eq!(meta.description, "single quoted");
	}

	#[test]
	fn test_parse_skill_meta_no_frontmatter() {
		let content = "# Just a markdown file\n\nNo frontmatter here.";
		assert!(parse_skill_meta(content).is_none());
	}

	#[test]
	fn test_parse_skill_meta_missing_name() {
		let content = "---\ndescription: No name field\n---\n";
		assert!(parse_skill_meta(content).is_none());
	}

	#[test]
	fn test_parse_skill_meta_missing_description() {
		let content = "---\nname: no-desc\n---\n";
		assert!(parse_skill_meta(content).is_none());
	}

	#[test]
	fn test_parse_skill_meta_unclosed_frontmatter() {
		// No closing ---
		let content = "---\nname: broken\ndescription: no close\n";
		assert!(parse_skill_meta(content).is_none());
	}

	#[test]
	fn test_parse_skill_meta_allowed_tools_single() {
		let content = "---\nname: s\ndescription: d\nallowed-tools: shell\n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.allowed_tools, vec!["shell"]);
	}

	#[test]
	fn test_parse_skill_meta_allowed_tools_empty_value() {
		// allowed-tools present but empty — should produce empty vec
		let content = "---\nname: s\ndescription: d\nallowed-tools: \n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert!(meta.allowed_tools.is_empty());
	}

	#[test]
	fn test_parse_skill_meta_leading_whitespace() {
		// File may have leading whitespace/newlines before ---
		let content = "\n\n---\nname: ws-skill\ndescription: whitespace before\n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.name, "ws-skill");
	}

	#[test]
	fn test_parse_skill_meta_unknown_fields_ignored() {
		let content =
			"---\nname: s\ndescription: d\nunknown-field: ignored\nanother: also-ignored\n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.name, "s");
		assert_eq!(meta.description, "d");
	}

	// ---------------------------------------------------------------------------
	// capabilities and domains parsing
	// ---------------------------------------------------------------------------

	#[test]
	fn test_parse_skill_meta_capabilities_space_delimited() {
		let content = "---\nname: s\ndescription: d\ncapabilities: git memory codesearch\n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.capabilities, vec!["git", "memory", "codesearch"]);
	}

	#[test]
	fn test_parse_skill_meta_capabilities_array_syntax() {
		let content = "---\nname: s\ndescription: d\ncapabilities: [\"git\", \"memory\"]\n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.capabilities, vec!["git", "memory"]);
	}

	#[test]
	fn test_parse_skill_meta_capabilities_array_unquoted() {
		let content = "---\nname: s\ndescription: d\ncapabilities: [git, memory]\n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.capabilities, vec!["git", "memory"]);
	}

	#[test]
	fn test_parse_skill_meta_domains_space_delimited() {
		let content = "---\nname: s\ndescription: d\ndomains: developer devops\n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.domains, vec!["developer", "devops"]);
	}

	#[test]
	fn test_parse_skill_meta_domains_array_syntax() {
		let content = "---\nname: s\ndescription: d\ndomains: [\"developer\", \"devops\"]\n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.domains, vec!["developer", "devops"]);
	}

	#[test]
	fn test_parse_skill_meta_empty_capabilities() {
		let content = "---\nname: s\ndescription: d\ncapabilities: \n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert!(meta.capabilities.is_empty());
	}

	#[test]
	fn test_parse_skill_meta_all_new_fields() {
		let content = "---\nname: rust-dev\ndescription: Rust development\ncapabilities: git memory\ndomains: developer\nallowed-tools: shell text_editor\n---\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.name, "rust-dev");
		assert_eq!(meta.capabilities, vec!["git", "memory"]);
		assert_eq!(meta.domains, vec!["developer"]);
		assert_eq!(meta.allowed_tools, vec!["shell", "text_editor"]);
	}

	// ---------------------------------------------------------------------------
	// activate/validate script discovery
	// ---------------------------------------------------------------------------

	#[test]
	fn test_has_activate_script() {
		let dir = tempfile::tempdir().unwrap();
		assert!(!has_activate_script(dir.path()));
		fs::write(dir.path().join("activate"), "#!/bin/bash\nexit 0").unwrap();
		assert!(has_activate_script(dir.path()));
	}

	#[test]
	fn test_has_validate_script() {
		let dir = tempfile::tempdir().unwrap();
		assert!(!has_validate_script(dir.path()));
		fs::write(dir.path().join("validate"), "#!/bin/bash\nexit 0").unwrap();
		assert!(has_validate_script(dir.path()));
	}

	// ---------------------------------------------------------------------------
	// build_resource_catalog
	// ---------------------------------------------------------------------------

	#[test]
	fn test_build_resource_catalog_empty_dir() {
		let dir = tempfile::tempdir().unwrap();
		let result = build_resource_catalog(dir.path());
		assert!(result.is_empty(), "no subdirs → empty catalog");
	}

	#[test]
	fn test_build_resource_catalog_no_known_subdirs() {
		let dir = tempfile::tempdir().unwrap();
		fs::create_dir(dir.path().join("other")).unwrap();
		fs::write(dir.path().join("other/file.txt"), "content").unwrap();
		let result = build_resource_catalog(dir.path());
		assert!(result.is_empty(), "unknown subdir → not included");
	}

	#[test]
	fn test_build_resource_catalog_scripts_only() {
		let dir = tempfile::tempdir().unwrap();
		let scripts = dir.path().join("scripts");
		fs::create_dir(&scripts).unwrap();
		fs::write(scripts.join("deploy.sh"), "#!/bin/bash\necho hi").unwrap();

		let result = build_resource_catalog(dir.path());
		assert!(result.contains("**scripts/**"));
		assert!(result.contains("deploy.sh"));
		assert!(result.contains(&scripts.join("deploy.sh").display().to_string()));
		assert!(!result.contains("**references/**"));
		assert!(!result.contains("**assets/**"));
	}

	#[test]
	fn test_build_resource_catalog_all_subdirs() {
		let dir = tempfile::tempdir().unwrap();

		let scripts = dir.path().join("scripts");
		fs::create_dir(&scripts).unwrap();
		fs::write(scripts.join("run.sh"), "#!/bin/bash").unwrap();

		let refs = dir.path().join("references");
		fs::create_dir(&refs).unwrap();
		fs::write(refs.join("guide.md"), "# Guide").unwrap();

		let assets = dir.path().join("assets");
		fs::create_dir(&assets).unwrap();
		fs::write(assets.join("template.json"), "{}").unwrap();

		let result = build_resource_catalog(dir.path());
		assert!(result.contains("**scripts/**"));
		assert!(result.contains("run.sh"));
		assert!(result.contains("**references/**"));
		assert!(result.contains("guide.md"));
		assert!(result.contains("**assets/**"));
		assert!(result.contains("template.json"));
	}

	#[test]
	fn test_build_resource_catalog_empty_subdir_skipped() {
		let dir = tempfile::tempdir().unwrap();
		// scripts exists but is empty — should not appear in output
		fs::create_dir(dir.path().join("scripts")).unwrap();
		// references has a file
		let refs = dir.path().join("references");
		fs::create_dir(&refs).unwrap();
		fs::write(refs.join("note.md"), "note").unwrap();

		let result = build_resource_catalog(dir.path());
		assert!(
			!result.contains("**scripts/**"),
			"empty scripts/ should be skipped"
		);
		assert!(result.contains("**references/**"));
	}

	#[test]
	fn test_build_resource_catalog_sorted_entries() {
		let dir = tempfile::tempdir().unwrap();
		let scripts = dir.path().join("scripts");
		fs::create_dir(&scripts).unwrap();
		fs::write(scripts.join("z_last.sh"), "").unwrap();
		fs::write(scripts.join("a_first.sh"), "").unwrap();
		fs::write(scripts.join("m_middle.sh"), "").unwrap();

		let result = build_resource_catalog(dir.path());
		let pos_a = result.find("a_first.sh").unwrap();
		let pos_m = result.find("m_middle.sh").unwrap();
		let pos_z = result.find("z_last.sh").unwrap();
		assert!(
			pos_a < pos_m && pos_m < pos_z,
			"entries should be sorted alphabetically"
		);
	}

	#[test]
	fn test_build_resource_catalog_subdirs_not_listed_as_files() {
		let dir = tempfile::tempdir().unwrap();
		let scripts = dir.path().join("scripts");
		fs::create_dir(&scripts).unwrap();
		// A nested directory inside scripts — should be ignored (not a file)
		fs::create_dir(scripts.join("nested")).unwrap();
		fs::write(scripts.join("real.sh"), "").unwrap();

		let result = build_resource_catalog(dir.path());
		assert!(result.contains("real.sh"));
		assert!(
			!result.contains("nested"),
			"subdirectories should not appear as entries"
		);
	}

	#[test]
	fn test_build_resource_catalog_header_format() {
		let dir = tempfile::tempdir().unwrap();
		let refs = dir.path().join("references");
		fs::create_dir(&refs).unwrap();
		fs::write(refs.join("doc.md"), "content").unwrap();

		let result = build_resource_catalog(dir.path());
		assert!(result.starts_with("\n\n## Skill Resources\n\n"));
	}
}
