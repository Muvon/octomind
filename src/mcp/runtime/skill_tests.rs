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
	use crate::mcp::runtime::skill::{
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
	// rules parsing
	// ---------------------------------------------------------------------------

	#[test]
	fn test_parse_rules_file() {
		let content = "---\nname: s\ndescription: d\nrules:\n  - file(Cargo.toml)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.rules.len(), 1);
		assert_eq!(meta.rules[0].len(), 1);
		assert!(
			matches!(&meta.rules[0][0], crate::mcp::runtime::skill::ActivateCheck::File(p) if p == "Cargo.toml")
		);
	}

	#[test]
	fn test_parse_rules_multiple_groups() {
		let content = "---\nname: s\ndescription: d\nrules:\n  - file(Cargo.toml)\n  - content(rust)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.rules.len(), 2);
		assert_eq!(meta.rules[0].len(), 1);
		assert_eq!(meta.rules[1].len(), 1);
		assert!(
			matches!(&meta.rules[0][0], crate::mcp::runtime::skill::ActivateCheck::File(p) if p == "Cargo.toml")
		);
		assert!(
			matches!(&meta.rules[1][0], crate::mcp::runtime::skill::ActivateCheck::Content(p) if p == "rust")
		);
	}

	#[test]
	fn test_parse_rules_multiple_checks_in_group() {
		let content =
			"---\nname: s\ndescription: d\nrules:\n  - content(rust) content(cargo)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.rules.len(), 1);
		assert_eq!(meta.rules[0].len(), 2);
		assert!(
			matches!(&meta.rules[0][0], crate::mcp::runtime::skill::ActivateCheck::Content(p) if p == "rust")
		);
		assert!(
			matches!(&meta.rules[0][1], crate::mcp::runtime::skill::ActivateCheck::Content(p) if p == "cargo")
		);
	}

	#[test]
	fn test_parse_rules_grep_with_path() {
		let content = "---\nname: s\ndescription: d\nrules:\n  - grep(fn main, *.rs)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.rules.len(), 1);
		assert_eq!(meta.rules[0].len(), 1);
		assert!(
			matches!(&meta.rules[0][0], crate::mcp::runtime::skill::ActivateCheck::Grep { pattern, path } if pattern == "fn main" && path.as_deref() == Some("*.rs"))
		);
	}

	#[test]
	fn test_parse_rules_env_and_match() {
		let content = "---\nname: s\ndescription: d\nrules:\n  - env(CI=true) match(\\bdeploy\\b)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.rules.len(), 1);
		assert_eq!(meta.rules[0].len(), 2);
		assert!(
			matches!(&meta.rules[0][0], crate::mcp::runtime::skill::ActivateCheck::Env { var, value } if var == "CI" && value.as_deref() == Some("true"))
		);
		assert!(
			matches!(&meta.rules[0][1], crate::mcp::runtime::skill::ActivateCheck::Match(p) if p == r"\bdeploy\b")
		);
	}

	#[test]
	fn test_parse_no_rules() {
		let content = "---\nname: s\ndescription: d\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert!(meta.rules.is_empty());
	}

	#[test]
	fn test_parse_rules_with_other_fields() {
		let content = "---\nname: programming-rust\ndescription: Rust dev\ncapabilities: programming-rust\ndomains: developer\nrules:\n  - file(Cargo.toml)\n  - content(rust)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.name, "programming-rust");
		assert_eq!(meta.capabilities, vec!["programming-rust"]);
		assert_eq!(meta.domains, vec!["developer"]);
		assert_eq!(meta.rules.len(), 2);
	}

	#[test]
	fn test_parse_rules_bin() {
		let content = "---\nname: s\ndescription: d\nrules:\n  - bin(cargo)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.rules.len(), 1);
		assert!(
			matches!(&meta.rules[0][0], crate::mcp::runtime::skill::ActivateCheck::Bin(p) if p == "cargo")
		);
	}

	#[test]
	fn test_parse_rules_session() {
		let content = "---\nname: s\ndescription: d\nrules:\n  - session(developer)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.rules.len(), 1);
		assert!(
			matches!(&meta.rules[0][0], crate::mcp::runtime::skill::ActivateCheck::Session(p) if p == "developer")
		);
	}

	#[test]
	fn test_parse_rules_workdir() {
		let content = "---\nname: s\ndescription: d\nrules:\n  - workdir(rust)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.rules.len(), 1);
		assert!(
			matches!(&meta.rules[0][0], crate::mcp::runtime::skill::ActivateCheck::Workdir(p) if p == "rust")
		);
	}

	#[test]
	fn test_parse_rules_combined_new_checks() {
		let content = "---\nname: s\ndescription: d\nrules:\n  - bin(cargo) file(Cargo.toml)\n  - session(dev) workdir(rust)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.rules.len(), 2);
		assert_eq!(meta.rules[0].len(), 2);
		assert_eq!(meta.rules[1].len(), 2);
		assert!(
			matches!(&meta.rules[0][0], crate::mcp::runtime::skill::ActivateCheck::Bin(p) if p == "cargo")
		);
		assert!(
			matches!(&meta.rules[0][1], crate::mcp::runtime::skill::ActivateCheck::File(p) if p == "Cargo.toml")
		);
		assert!(
			matches!(&meta.rules[1][0], crate::mcp::runtime::skill::ActivateCheck::Session(p) if p == "dev")
		);
		assert!(
			matches!(&meta.rules[1][1], crate::mcp::runtime::skill::ActivateCheck::Workdir(p) if p == "rust")
		);
	}

	// ---------------------------------------------------------------------------
	// activate rule evaluation
	// ---------------------------------------------------------------------------

	#[test]
	fn test_activate_check_file_exists() {
		let dir = tempfile::tempdir().unwrap();
		fs::write(dir.path().join("Cargo.toml"), "").unwrap();
		let check = crate::mcp::runtime::skill::ActivateCheck::File("Cargo.toml".to_string());
		assert!(check.matches("", dir.path(), "", None));
		assert!(
			!crate::mcp::runtime::skill::ActivateCheck::File("go.mod".to_string()).matches(
				"",
				dir.path(),
				"",
				None
			)
		);
	}

	#[test]
	fn test_activate_check_content_match() {
		let check = crate::mcp::runtime::skill::ActivateCheck::Content("rust".to_string());
		assert!(check.matches("lets code in rust", std::path::Path::new("."), "", None));
		assert!(check.matches("RUST is great", std::path::Path::new("."), "", None));
		assert!(!check.matches("lets code in python", std::path::Path::new("."), "", None));
	}

	#[test]
	fn test_activate_check_content_word_boundary() {
		let check = crate::mcp::runtime::skill::ActivateCheck::Content("rust".to_string());
		assert!(check.matches("lets code in rust", std::path::Path::new("."), "", None));
		assert!(check.matches("RUST is great", std::path::Path::new("."), "", None));
		assert!(!check.matches("lets code in python", std::path::Path::new("."), "", None));
		// Word boundary: "rust" should not match "thrust"
		assert!(!check.matches("thrust is powerful", std::path::Path::new("."), "", None));
	}

	#[test]
	fn test_activate_check_bin_found() {
		// "ls" exists on all platforms
		let check = crate::mcp::runtime::skill::ActivateCheck::Bin("ls".to_string());
		assert!(check.matches("", std::path::Path::new("."), "", None));
	}

	#[test]
	fn test_activate_check_bin_not_found() {
		let check = crate::mcp::runtime::skill::ActivateCheck::Bin(
			"nonexistent_binary_xyz_12345".to_string(),
		);
		assert!(!check.matches("", std::path::Path::new("."), "", None));
	}

	#[test]
	fn test_activate_check_session_match() {
		let check = crate::mcp::runtime::skill::ActivateCheck::Session("octomind".to_string());
		assert!(check.matches(
			"",
			std::path::Path::new("."),
			"260421-141708-octomind-a1b2c3",
			None
		));
		// Case-insensitive
		assert!(check.matches("", std::path::Path::new("."), "Octomind-Session", None));
	}

	#[test]
	fn test_activate_check_session_no_match() {
		let check = crate::mcp::runtime::skill::ActivateCheck::Session("python".to_string());
		assert!(!check.matches(
			"",
			std::path::Path::new("."),
			"260421-141708-octomind-a1b2c3",
			None
		));
	}

	#[test]
	fn test_activate_check_workdir_match() {
		let check = crate::mcp::runtime::skill::ActivateCheck::Workdir("octomind".to_string());
		assert!(check.matches("", std::path::Path::new("/Users/dev/octomind"), "", None));
		// Case-insensitive
		assert!(check.matches("", std::path::Path::new("/Users/dev/Octomind"), "", None));
	}

	#[test]
	fn test_activate_check_workdir_no_match() {
		let check = crate::mcp::runtime::skill::ActivateCheck::Workdir("python".to_string());
		assert!(!check.matches("", std::path::Path::new("/Users/dev/octomind"), "", None));
	}

	// ---------------------------------------------------------------------------
	// activate check: semantic(...) — parse, render, match
	// ---------------------------------------------------------------------------

	#[test]
	fn test_activate_check_semantic_parse_default_threshold() {
		// `semantic(phrase)` parses with the global default threshold.
		let check = crate::mcp::runtime::skill::ActivateCheck::parse("semantic(deploying to prod)")
			.expect("parse semantic");
		match check {
			crate::mcp::runtime::skill::ActivateCheck::Semantic { phrase, threshold } => {
				assert_eq!(phrase, "deploying to prod");
				assert!(
					(threshold - crate::mcp::runtime::skill::SEMANTIC_DEFAULT_THRESHOLD).abs()
						< 1e-6
				);
			}
			other => panic!("expected Semantic variant, got {other:?}"),
		}
	}

	#[test]
	fn test_activate_check_semantic_parse_explicit_threshold() {
		// `semantic(phrase, 0.55)` parses the trailing float as threshold.
		let check = crate::mcp::runtime::skill::ActivateCheck::parse("semantic(deploy, 0.55)")
			.expect("parse with threshold");
		match check {
			crate::mcp::runtime::skill::ActivateCheck::Semantic { phrase, threshold } => {
				assert_eq!(phrase, "deploy");
				assert!((threshold - 0.55).abs() < 1e-6);
			}
			other => panic!("expected Semantic variant, got {other:?}"),
		}
	}

	#[test]
	fn test_activate_check_semantic_parse_phrase_with_comma() {
		// `semantic(deploy, ship, release)` — last piece doesn't parse as
		// f32, so the whole arg is the phrase (commas preserved).
		let check =
			crate::mcp::runtime::skill::ActivateCheck::parse("semantic(deploy, ship, release)")
				.expect("parse phrase with commas");
		match check {
			crate::mcp::runtime::skill::ActivateCheck::Semantic { phrase, threshold } => {
				assert_eq!(phrase, "deploy, ship, release");
				assert!(
					(threshold - crate::mcp::runtime::skill::SEMANTIC_DEFAULT_THRESHOLD).abs()
						< 1e-6
				);
			}
			other => panic!("expected Semantic variant, got {other:?}"),
		}
	}

	#[test]
	fn test_activate_check_semantic_parse_empty_rejects() {
		// Empty phrase is invalid; parser returns None.
		assert!(crate::mcp::runtime::skill::ActivateCheck::parse("semantic()").is_none());
		assert!(crate::mcp::runtime::skill::ActivateCheck::parse("semantic(   )").is_none());
		assert!(crate::mcp::runtime::skill::ActivateCheck::parse("semantic(, 0.5)").is_none());
	}

	#[test]
	fn test_activate_check_semantic_display_round_trip() {
		// Default-threshold renders as `semantic(phrase)`.
		let default_t = crate::mcp::runtime::skill::SEMANTIC_DEFAULT_THRESHOLD;
		let check = crate::mcp::runtime::skill::ActivateCheck::Semantic {
			phrase: "deploy".into(),
			threshold: default_t,
		};
		assert_eq!(check.to_string(), "semantic(deploy)");

		// Explicit threshold renders as `semantic(phrase, X)`.
		let check = crate::mcp::runtime::skill::ActivateCheck::Semantic {
			phrase: "deploy".into(),
			threshold: 0.6,
		};
		assert_eq!(check.to_string(), "semantic(deploy, 0.6)");
	}

	#[test]
	fn test_activate_check_semantic_matches_via_precomputed_scores() {
		use std::collections::HashMap;
		let check = crate::mcp::runtime::skill::ActivateCheck::Semantic {
			phrase: "deploying to production".into(),
			threshold: 0.45,
		};
		// Precomputed cosine above threshold → match.
		let mut scores = HashMap::new();
		scores.insert("deploying to production".to_string(), 0.6_f32);
		assert!(check.matches("any", std::path::Path::new("."), "", Some(&scores)));

		// Below threshold → no match.
		scores.insert("deploying to production".to_string(), 0.3_f32);
		assert!(!check.matches("any", std::path::Path::new("."), "", Some(&scores)));
	}

	#[test]
	fn test_activate_check_semantic_silent_false_without_context() {
		// When precomputed scores are unavailable (model not ready, etc.),
		// the semantic check evaluates to false rather than panicking — so
		// other checks in the same DNF group can still fire.
		let check = crate::mcp::runtime::skill::ActivateCheck::Semantic {
			phrase: "deploying to production".into(),
			threshold: 0.45,
		};
		assert!(!check.matches("any", std::path::Path::new("."), "", None));
	}

	#[test]
	fn test_activate_check_semantic_missing_phrase_in_scores_fails() {
		use std::collections::HashMap;
		// Score map exists but doesn't contain the phrase — fall through to false.
		let check = crate::mcp::runtime::skill::ActivateCheck::Semantic {
			phrase: "deploying to production".into(),
			threshold: 0.45,
		};
		let mut scores = HashMap::new();
		scores.insert("something else".to_string(), 0.99_f32);
		assert!(!check.matches("any", std::path::Path::new("."), "", Some(&scores)));
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

	// ---------------------------------------------------------------------------
	// ActivateCheck::matches — filesystem, env, and regex checks
	// ---------------------------------------------------------------------------

	struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

	impl EnvGuard {
		fn new(keys: &[&'static str]) -> Self {
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

	use crate::mcp::runtime::skill::{universal_skill_dirs, ActivateCheck};

	#[test]
	fn test_activate_check_grep_matches() {
		let dir = tempfile::tempdir().unwrap();
		fs::write(dir.path().join("main.rs"), "fn main() { println_body() }").unwrap();
		fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

		let check = ActivateCheck::Grep {
			pattern: "fn main".to_string(),
			path: None,
		};
		assert!(check.matches("", dir.path(), "", None));

		// Path filter narrows the search to matching file names.
		let check = ActivateCheck::Grep {
			pattern: "fn main".to_string(),
			path: Some("*.rs".to_string()),
		};
		assert!(check.matches("", dir.path(), "", None));
		let check = ActivateCheck::Grep {
			pattern: "fn main".to_string(),
			path: Some("*.toml".to_string()),
		};
		assert!(!check.matches("", dir.path(), "", None));

		// No hit anywhere.
		let check = ActivateCheck::Grep {
			pattern: "no_such_symbol_xyz".to_string(),
			path: None,
		};
		assert!(!check.matches("", dir.path(), "", None));

		// Invalid regex falls back to literal substring search (no panic). The
		// literal must also be absent from the fixture files, otherwise the
		// fallback match finds it.
		let check = ActivateCheck::Grep {
			pattern: "no_such_literal_(".to_string(),
			path: None,
		};
		assert!(!check.matches("", dir.path(), "", None));
	}

	#[test]
	#[serial_test::serial]
	fn test_activate_check_env_matches() {
		let _env = EnvGuard::new(&["SKILLTEST_ENV_VAR"]);

		let check = ActivateCheck::Env {
			var: "SKILLTEST_ENV_VAR".to_string(),
			value: None,
		};
		std::env::remove_var("SKILLTEST_ENV_VAR");
		assert!(!check.matches("", std::path::Path::new("."), "", None));

		std::env::set_var("SKILLTEST_ENV_VAR", "");
		assert!(
			!check.matches("", std::path::Path::new("."), "", None),
			"empty value must not match"
		);

		std::env::set_var("SKILLTEST_ENV_VAR", "hello");
		assert!(check.matches("", std::path::Path::new("."), "", None));

		// Value-pinned variant: equality, not presence.
		let check = ActivateCheck::Env {
			var: "SKILLTEST_ENV_VAR".to_string(),
			value: Some("hello".to_string()),
		};
		assert!(check.matches("", std::path::Path::new("."), "", None));
		let check = ActivateCheck::Env {
			var: "SKILLTEST_ENV_VAR".to_string(),
			value: Some("other".to_string()),
		};
		assert!(!check.matches("", std::path::Path::new("."), "", None));
	}

	#[test]
	fn test_activate_check_match_regex() {
		let check = ActivateCheck::Match(r"\bdeploy\b".to_string());
		assert!(check.matches("please deploy now", std::path::Path::new("."), "", None));
		assert!(!check.matches("deployment soon", std::path::Path::new("."), "", None));

		// Invalid regex evaluates to false, never panics.
		let check = ActivateCheck::Match("[invalid".to_string());
		assert!(!check.matches("anything", std::path::Path::new("."), "", None));
	}

	#[test]
	fn test_activate_check_file_glob() {
		let dir = tempfile::tempdir().unwrap();
		fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

		let check = ActivateCheck::File("*.toml".to_string());
		assert!(check.matches("", dir.path(), "", None));
		let check = ActivateCheck::File("*.md".to_string());
		assert!(!check.matches("", dir.path(), "", None));
	}

	#[test]
	fn test_universal_skill_dirs_project_only() {
		let with_project = tempfile::tempdir().unwrap();
		fs::create_dir_all(with_project.path().join(".agents/skills")).unwrap();
		let dirs = universal_skill_dirs(with_project.path());
		assert!(dirs.contains(&with_project.path().join(".agents/skills")));

		let without_project = tempfile::tempdir().unwrap();
		let dirs = universal_skill_dirs(without_project.path());
		// The global dir may or may not exist on this machine; when it doesn't,
		// a project without .agents/skills yields no dirs at all.
		let global = dirs::home_dir()
			.map(|h| h.join(".config").join("agents").join("skills"))
			.unwrap_or_else(|| std::path::PathBuf::from("/dev/null"));
		if !global.is_dir() {
			assert!(dirs.is_empty());
		}
	}
	#[test]
	#[serial_test::serial]
	fn test_find_skill_by_name_tap_source_and_name_mismatch() {
		let _env = EnvGuard::new(&["OCTOMIND_DATA_DIR"]);
		let dir = tempfile::tempdir().unwrap();
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());

		// The default tap needs no taps.toml — creating the dir is enough.
		let tap = dir.path().join("taps").join("muvon").join("octomind-tap");
		fs::create_dir_all(tap.join("skills").join("skilltest-tapfind")).unwrap();
		fs::write(
			tap.join("skills")
				.join("skilltest-tapfind")
				.join("SKILL.md"),
			"---\nname: skilltest-tapfind\ndescription: Tap fixture\n---\n\nTAPFIND-BODY\n",
		)
		.unwrap();
		// Directory name and frontmatter name disagree: must NOT resolve.
		fs::create_dir_all(tap.join("skills").join("skilltest-mismatch")).unwrap();
		fs::write(
			tap.join("skills")
				.join("skilltest-mismatch")
				.join("SKILL.md"),
			"---\nname: skilltest-other-name\ndescription: Mismatched fixture\n---\n\nBody\n",
		)
		.unwrap();

		let (_, _, content) =
			crate::mcp::runtime::skill::find_skill_by_name_pub("skilltest-tapfind")
				.expect("tap skill resolves by name");
		assert!(content.contains("TAPFIND-BODY"));
		assert!(
			crate::mcp::runtime::skill::find_skill_by_name_pub("skilltest-mismatch").is_none(),
			"dir/name mismatch must not resolve"
		);
	}

	#[test]
	fn test_parse_check_rejects_malformed_parens() {
		use crate::mcp::runtime::skill::ActivateCheck;
		// Close paren before the open paren: not a check.
		assert!(ActivateCheck::parse(")file(").is_none());
		// No parens at all.
		assert!(ActivateCheck::parse("file").is_none());
	}

	#[test]
	fn test_parse_grep_without_path() {
		use crate::mcp::runtime::skill::ActivateCheck;
		assert!(matches!(
			ActivateCheck::parse("grep(fn main)"),
			Some(ActivateCheck::Grep {
				pattern,
				path: None,
			}) if pattern == "fn main"
		));
	}

	#[test]
	fn test_display_renders_all_check_variants() {
		use crate::mcp::runtime::skill::ActivateCheck;
		assert_eq!(
			ActivateCheck::Grep {
				pattern: "fn main".to_string(),
				path: Some("*.rs".to_string()),
			}
			.to_string(),
			"grep(fn main, *.rs)"
		);
		assert_eq!(
			ActivateCheck::Grep {
				pattern: "fn main".to_string(),
				path: None,
			}
			.to_string(),
			"grep(fn main)"
		);
		assert_eq!(
			ActivateCheck::Env {
				var: "KEY".to_string(),
				value: Some("val".to_string()),
			}
			.to_string(),
			"env(KEY=val)"
		);
		assert_eq!(
			ActivateCheck::Env {
				var: "KEY".to_string(),
				value: None,
			}
			.to_string(),
			"env(KEY)"
		);
		assert_eq!(
			ActivateCheck::Match(r"\bdeploy\b".to_string()).to_string(),
			r"match(\bdeploy\b)"
		);
		assert_eq!(
			ActivateCheck::Bin("cargo".to_string()).to_string(),
			"bin(cargo)"
		);
		assert_eq!(
			ActivateCheck::Session("octomind".to_string()).to_string(),
			"session(octomind)"
		);
		assert_eq!(
			ActivateCheck::Workdir("rust".to_string()).to_string(),
			"workdir(rust)"
		);
	}

	#[test]
	fn test_grep_workdir_path_filter() {
		use crate::mcp::runtime::skill::ActivateCheck;
		let dir = tempfile::tempdir().unwrap();
		fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
		fs::write(dir.path().join("notes.md"), "fn main mentioned in notes").unwrap();

		// grep_workdir is exercised through the Grep check's matcher: the
		// path filter must restrict the search and a miss must fall through
		// to the final `false`.
		let rs_only = ActivateCheck::Grep {
			pattern: "fn main".to_string(),
			path: Some("*.rs".to_string()),
		};
		let toml_only = ActivateCheck::Grep {
			pattern: "fn main".to_string(),
			path: Some("*.toml".to_string()),
		};
		let everywhere = ActivateCheck::Grep {
			pattern: "fn main".to_string(),
			path: None,
		};
		let nowhere = ActivateCheck::Grep {
			pattern: "no_such_symbol_anywhere".to_string(),
			path: None,
		};
		assert!(rs_only.matches("", dir.path(), "", None));
		assert!(!toml_only.matches("", dir.path(), "", None));
		assert!(everywhere.matches("", dir.path(), "", None));
		assert!(!nowhere.matches("", dir.path(), "", None));
	}

	#[test]
	fn test_parse_skill_meta_rules_skips_non_check_lines() {
		let content = "---\nname: s\ndescription: d\nunknown-key: ignored\nrules:\n  # a comment line, not a check\n  - content(rust)\n---\nbody\n";
		let meta = parse_skill_meta(content).expect("should parse");
		assert_eq!(meta.rules.len(), 1, "{:?}", meta.rules);
		assert_eq!(
			crate::mcp::runtime::skill::ActivateCheck::Content("rust".to_string()).to_string(),
			meta.rules[0][0].to_string()
		);
	}

	#[test]
	#[serial_test::serial]
	fn test_universal_skill_dirs_includes_global_home_dir() {
		let _env = EnvGuard::new(&["HOME"]);
		let home = tempfile::tempdir().unwrap();
		let global = home.path().join(".config").join("agents").join("skills");
		fs::create_dir_all(&global).unwrap();
		std::env::set_var("HOME", home.path());

		let workdir = tempfile::tempdir().unwrap();
		let dirs = universal_skill_dirs(workdir.path());
		assert!(
			dirs.contains(&global),
			"global skills dir must be listed: {dirs:?}"
		);
	}

	#[tokio::test]
	#[serial_test::serial]
	async fn test_find_all_skills_dedupes_tap_over_project() {
		let _env = EnvGuard::new(&["OCTOMIND_DATA_DIR"]);
		let data = tempfile::tempdir().unwrap();
		std::env::set_var("OCTOMIND_DATA_DIR", data.path());

		let tap = data.path().join("taps").join("muvon").join("octomind-tap");
		fs::create_dir_all(tap.join("skills").join("skilltest-dup")).unwrap();
		fs::write(
			tap.join("skills").join("skilltest-dup").join("SKILL.md"),
			"---\nname: skilltest-dup\ndescription: Tap copy\n---\n\nTAP-COPY\n",
		)
		.unwrap();

		let sid = "__skilltests_dedup".to_string();
		let project = tempfile::tempdir().unwrap();
		fs::create_dir_all(project.path().join(".agents/skills/skilltest-dup")).unwrap();
		fs::write(
			project.path().join(".agents/skills/skilltest-dup/SKILL.md"),
			"---\nname: skilltest-dup\ndescription: Project copy\n---\n\nPROJECT-COPY\n",
		)
		.unwrap();

		crate::session::context::with_session_id(sid.clone(), async {
			crate::session::context::set_session_workdir(&sid, project.path().to_path_buf());

			let found = crate::mcp::runtime::skill::find_all_skills_with_details();
			let hits: Vec<&std::path::PathBuf> = found
				.iter()
				.filter(|(meta, _)| meta.name == "skilltest-dup")
				.map(|(_, path)| path)
				.collect();
			assert_eq!(hits.len(), 1, "tap copy must win over the project copy");
			assert!(
				hits[0].starts_with(&tap),
				"expected the tap copy, got {:?}",
				hits[0]
			);
		})
		.await;
		crate::session::context::cleanup_session(&sid);
	}
}
