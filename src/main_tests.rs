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
fn run_collects_repeated_config_overrides_in_order() {
	let args = CliArgs::try_parse_from(["octomind", "run", "-c", "a=b", "-c", "c=d"])
		.expect("config overrides parse");
	assert_eq!(args.config_overrides, ["a=b", "c=d"]);
	assert!(matches!(args.command, Some(Commands::Run(_))));
}

/// The dynamic positional completion is wired for every subcommand that takes a
/// name from the local registry — adding a new one must not be forgotten here.
#[test]
fn completion_script_patches_dynamic_positionals() {
	let mut app = CliArgs::command();
	let name = app.get_name().to_string();

	for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
		let mut buf = Vec::new();
		generate(shell, &mut app, &name, &mut buf);
		let script = String::from_utf8_lossy(&buf);
		let patched = patch_completion_script(&script, shell);

		for sub in ["run", "workflow", "untap"] {
			assert!(
				patched.contains(&format!("octomind complete {sub}")),
				"{shell:?} script must complete `{sub}` dynamically"
			);
		}
	}
}

/// `untap` takes a required positional, so the zsh descriptor clap emits is
/// `'::name -- …:_default'`. The patcher rewrites that completer to the dynamic
/// one; if the arg id or the descriptor shape ever drifts, this fails loudly
/// instead of silently falling back to file completion.
#[test]
fn untap_zsh_positional_is_rebound_to_the_dynamic_completer() {
	let mut app = CliArgs::command();
	let name = app.get_name().to_string();
	let mut buf = Vec::new();
	generate(Shell::Zsh, &mut app, &name, &mut buf);
	let script = String::from_utf8_lossy(&buf);

	let patched = patch_completion_script(&script, Shell::Zsh);
	assert!(
		patched.contains(":_octomind_complete_untap'"),
		"the `untap` positional must point at the dynamic completer"
	);
}

#[test]
fn format_accepts_only_plain_and_jsonl() {
	assert!(CliArgs::try_parse_from(["octomind", "run", "--format", "json"]).is_err());
	assert!(CliArgs::try_parse_from(["octomind", "workflow", "x", "--format", "json"]).is_err());
	let args =
		CliArgs::try_parse_from(["octomind", "run", "--format", "jsonl"]).expect("jsonl parses");
	assert!(matches!(args.command, Some(Commands::Run(_))));
	let args = CliArgs::try_parse_from(["octomind", "workflow", "x", "--format", "jsonl"])
		.expect("jsonl parses");
	assert!(matches!(args.command, Some(Commands::Workflow(_))));
}
