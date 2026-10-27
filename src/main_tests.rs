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
	let args = CliArgs::try_parse_from(["octomind", "run", "-c", "a=b", "-c", "c=d"]).expect("config overrides parse");
	assert_eq!(args.config_overrides, ["a=b", "c=d"]);
	assert!(matches!(args.command, Some(Commands::Run(_))));
}

#[test]
fn format_accepts_only_plain_and_jsonl() {
	assert!(CliArgs::try_parse_from(["octomind", "run", "--format", "json"]).is_err());
	assert!(CliArgs::try_parse_from(["octomind", "workflow", "x", "--format", "json"]).is_err());
	let args = CliArgs::try_parse_from(["octomind", "run", "--format", "jsonl"]).expect("jsonl parses");
	assert!(matches!(args.command, Some(Commands::Run(_))));
	let args = CliArgs::try_parse_from(["octomind", "workflow", "x", "--format", "jsonl"]).expect("jsonl parses");
	assert!(matches!(args.command, Some(Commands::Workflow(_))));
}

fn completion_script(shell: Shell) -> String {
	let mut app = CliArgs::command();
	let mut buf = Vec::new();
	generate(shell, &mut app, "octomind", &mut buf);
	patch_completion_script(&String::from_utf8(buf).expect("completion script is UTF-8"), shell)
}

// The patches are string matches against clap_complete's output, so a format
// change upstream silently turns them into no-ops unless a test checks them.
#[test]
fn completion_scripts_complete_dynamic_positionals_in_every_shell() {
	let bash = completion_script(Shell::Bash);
	let zsh = completion_script(Shell::Zsh);
	let fish = completion_script(Shell::Fish);
	for sub in ["run", "workflow", "untap"] {
		assert!(
			bash.contains(&format!("compgen -W \"$(octomind complete {sub} 2>/dev/null)\"")),
			"bash completes `{sub}` dynamically"
		);
		assert!(
			zsh.contains(&format!(":_octomind_complete_{sub}' \\")),
			"zsh completes `{sub}` dynamically"
		);
		assert!(
			fish.contains(&format!("-a '(octomind complete {sub} 2>/dev/null)'")),
			"fish completes `{sub}` dynamically"
		);
	}
}
