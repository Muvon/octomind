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

//! Tests for `octomind evaluate`: request parsing and argument defaults.

use super::*;
use clap::Parser;

#[derive(clap::Parser)]
struct Cli {
	#[command(flatten)]
	args: EvaluateArgs,
}

#[test]
fn the_model_defaults_to_the_one_octomind_shipped_for_its_judgements() {
	let cli = Cli::try_parse_from(["octomind"]).expect("bare parse");
	assert_eq!(cli.args.model, DEFAULT_MODEL);
	assert_eq!(DEFAULT_MODEL, "cloudflare:typesafe/jev");
	assert!(cli.args.file.is_none());

	let cli = Cli::try_parse_from(["octomind", "-m", "typesafe:jev-latest", "req.json"]).expect("model + file parse");
	assert_eq!(cli.args.model, "typesafe:jev-latest");
	assert_eq!(cli.args.file.as_deref(), Some(Path::new("req.json")));
}

#[test]
fn the_help_text_names_the_default_model() {
	use clap::CommandFactory;
	let help = Cli::command().render_long_help().to_string();
	assert!(help.contains(DEFAULT_MODEL), "{help}");
}

#[test]
fn parse_input_accepts_typed_questions_and_rejects_empty() {
	let input =
		parse_input(r#"{"state":{"diff":"+1"},"questions":{"done":{"type":"noul","instructions":"Is it done?"}}}"#)
			.expect("noul request parses");
	assert_eq!(input.state["diff"], "+1");
	assert!(matches!(input.questions["done"], Question::Noul { .. }));

	let example = parse_input(include_str!("../../config-templates/evaluate.json")).expect("shipped example parses");
	assert!(matches!(example.questions["done"], Question::Noul { .. }));
	assert!(matches!(example.questions["next"], Question::Choice { .. }));
	assert!(matches!(example.questions["risk"], Question::Score { .. }));

	let err = parse_input(r#"{"state":"x","questions":{}}"#).unwrap_err();
	assert!(err.to_string().contains("no questions"));

	let err = parse_input("not json").unwrap_err();
	assert!(err.to_string().contains("invalid request JSON"));
}

#[tokio::test]
async fn an_empty_model_is_rejected() {
	let request = tempfile::NamedTempFile::new().unwrap();
	std::fs::write(request.path(), include_str!("../../config-templates/evaluate.json")).unwrap();
	let error = execute(&EvaluateArgs {
		model: String::new(),
		file: Some(request.path().to_path_buf()),
	})
	.await
	.unwrap_err();
	assert_eq!(
		error.to_string(),
		"model is empty; pass a `provider:model` name with --model"
	);
}
