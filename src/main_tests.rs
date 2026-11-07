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
fn configure_first_run_requires_missing_config_and_both_ttys() {
	let run = Commands::Run(commands::RunArgs::default());
	for missing in [false, true] {
		for stdin_tty in [false, true] {
			for stdout_tty in [false, true] {
				assert_eq!(
					should_configure_first_run(&run, missing, stdin_tty, stdout_tty),
					missing && stdin_tty && stdout_tty
				);
			}
		}
	}
}

#[test]
fn configure_first_run_excludes_noninteractive_modes_and_other_commands() {
	for argv in [
		vec!["octomind", "run", "--format", "plain"],
		vec!["octomind", "run", "--format", "jsonl"],
		vec!["octomind", "run", "--daemon"],
		vec!["octomind", "acp"],
		vec!["octomind", "server"],
		vec!["octomind", "workflow"],
		vec!["octomind", "evaluate"],
		vec!["octomind", "send", "--name", "session", "hello"],
		vec!["octomind", "config"],
		vec!["octomind", "configure"],
		vec!["octomind", "login"],
		vec!["octomind", "tap"],
		vec!["octomind", "untap", "user/repo"],
		vec!["octomind", "vars"],
		vec!["octomind", "completion", "bash"],
		vec!["octomind", "complete", "run"],
		vec![
			"octomind",
			"distill",
			"--messages",
			"messages.json",
			"--role",
			"assistant",
		],
		vec![
			"octomind",
			"compact",
			"--fold",
			"fold.json",
			"--session-file",
			"session.json",
		],
		vec!["octomind", "hindsight-worker", "--model", "local:test"],
	] {
		let parsed = CliArgs::try_parse_from(&argv).expect("valid command");
		assert!(
			!should_configure_first_run(&parsed.command.unwrap(), true, true, true),
			"{argv:?}"
		);
	}
	for argv in [vec!["octomind"], vec!["octomind", "run"]] {
		let parsed = CliArgs::try_parse_from(argv).expect("interactive command");
		let command = parsed
			.command
			.unwrap_or_else(|| Commands::Run(commands::RunArgs::default()));
		assert!(should_configure_first_run(&command, true, true, true));
	}
}

#[test]
fn configure_is_a_visible_subcommand() {
	let app = CliArgs::command();
	let sub = app.find_subcommand("configure").expect("configure registered");
	assert!(!sub.is_hide_set());
	assert_eq!(
		sub.get_about().unwrap().to_string(),
		"Choose your model provider, key and models — the Octomind hub or your own keys"
	);
	let args = CliArgs::try_parse_from(["octomind", "configure"]).expect("configure parses");
	assert_eq!(command_name(&args.command.unwrap()), "configure");
}

#[test]
fn compact_is_hidden_and_requires_fold_and_session_file() {
	let app = CliArgs::command();
	assert!(app.find_subcommand("compact").unwrap().is_hide_set());
	assert!(CliArgs::try_parse_from(["octomind", "compact", "--fold", "request.fold.json"]).is_err());
	assert!(CliArgs::try_parse_from(["octomind", "compact", "--session-file", "session.jsonl.zst"]).is_err());
	let parsed = CliArgs::try_parse_from([
		"octomind",
		"compact",
		"--fold",
		"request.fold.json",
		"--session-file",
		"session.jsonl.zst",
	])
	.unwrap();
	let command = parsed.command.unwrap();
	assert_eq!(command_name(&command), "compact");
	assert!(matches!(command, Commands::Compact(_)));
}

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
