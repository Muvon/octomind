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

//! Tests for `octomind run`: which session `resumed_session_name` resolves and
//! how `RunArgs` parses its flags.

use crate::commands::test_support::EnvGuard;
use super::*;
use clap::Parser;

const DATA_DIR_KEY: &str = "OCTOMIND_DATA_DIR";

#[test]
fn resume_name_wins_over_name() {
	let args = RunArgs {
		resume: Some("s1".into()),
		name: Some("n".into()),
		..Default::default()
	};
	assert_eq!(resumed_session_name(&args).as_deref(), Some("s1"));
}

#[test]
fn bare_resume_falls_back_to_name() {
	let args = RunArgs {
		resume: Some(String::new()),
		name: Some("n".into()),
		..Default::default()
	};
	assert_eq!(resumed_session_name(&args).as_deref(), Some("n"));
}

#[test]
fn bare_resume_without_name_resolves_to_none() {
	let args = RunArgs {
		resume: Some(String::new()),
		..Default::default()
	};
	assert_eq!(resumed_session_name(&args), None);
}

#[test]
fn resume_name_wins_over_resume_recent() {
	let args = RunArgs {
		resume: Some("x".into()),
		resume_recent: true,
		..Default::default()
	};
	assert_eq!(resumed_session_name(&args).as_deref(), Some("x"));
}

#[test]
fn nothing_set_resolves_to_none() {
	assert_eq!(resumed_session_name(&RunArgs::default()), None);
}

#[test]
#[serial_test::serial]
fn resume_recent_without_saved_sessions_is_none() {
	let _env = EnvGuard::new(&[DATA_DIR_KEY]);
	let data_dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var(DATA_DIR_KEY, data_dir.path());

	let args = RunArgs {
		name: Some("n".into()),
		resume_recent: true,
		..Default::default()
	};
	assert_eq!(resumed_session_name(&args), None);
}

#[derive(clap::Parser)]
struct Cli {
	#[command(flatten)]
	args: RunArgs,
}

#[test]
fn bare_resume_parses_as_empty_string() {
	let cli = Cli::try_parse_from(["octomind", "--resume"]).expect("bare --resume parses");
	assert_eq!(cli.args.resume.as_deref(), Some(""));
}

#[test]
fn resume_short_flag_takes_a_value() {
	let cli = Cli::try_parse_from(["octomind", "-r", "foo"]).expect("-r foo parses");
	assert_eq!(cli.args.resume.as_deref(), Some("foo"));
}

#[test]
fn resume_equals_syntax_parses() {
	for (flag, expected) in [("--resume=foo", "foo"), ("--resume=", "")] {
		let cli = Cli::try_parse_from(["octomind", flag]).expect("equals-sign --resume parses");
		assert_eq!(cli.args.resume.as_deref(), Some(expected));
	}
}

#[test]
fn invalid_arguments_return_parse_errors() {
	let unknown = Cli::try_parse_from(["octomind", "--unknown-flag"])
		.err()
		.expect("unknown flag returns a parse error");
	assert_eq!(unknown.kind(), clap::error::ErrorKind::UnknownArgument);

	let missing_value = Cli::try_parse_from(["octomind", "--model"])
		.err()
		.expect("--model requires a value");
	assert_eq!(missing_value.kind(), clap::error::ErrorKind::InvalidValue);
}

#[test]
fn hooks_collect_every_occurrence() {
	let cli = Cli::try_parse_from(["octomind", "--hook", "a", "--hook", "b"]).expect("hooks parse");
	assert_eq!(cli.args.hooks, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn name_and_model_short_flags_parse() {
	let cli =
		Cli::try_parse_from(["octomind", "-n", "sess", "-m", "openrouter:x"]).expect("-n/-m parse");
	assert_eq!(cli.args.name.as_deref(), Some("sess"));
	assert_eq!(cli.args.model.as_deref(), Some("openrouter:x"));
}
