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

//! Handler-level tests for the `/report` session command error paths:
//! usage errors for bad periods or extra arguments, and the file-less
//! session explanation. Period aggregation over the real sessions dir is
//! intentionally not exercised here.

use super::*;

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

/// Unwrap a handled error output into its message and hint context.
fn error_output(result: CommandResult) -> (String, serde_json::Value) {
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	let CommandOutput::Error { error, context } = *output else {
		panic!("expected an error output");
	};
	(error, context.expect("hint context attached"))
}

#[test]
fn unknown_period_reports_usage() {
	let session = ChatSession::for_tests(Vec::new());
	let (error, context) = error_output(handle_report(&session, &template_config(), &["fortnight"]).unwrap());
	assert!(error.contains("fortnight"), "{error}");
	assert!(
		context["hint"].as_str().expect("hint").contains("Usage: /report"),
		"{context}"
	);
}

#[test]
fn extra_arguments_report_usage() {
	let session = ChatSession::for_tests(Vec::new());
	let (error, _) = error_output(handle_report(&session, &template_config(), &["day", "here", "extra"]).unwrap());
	assert!(error.contains("day here extra"), "{error}");
}

#[test]
fn session_report_without_a_session_file_explains_auto_save() {
	let session = ChatSession::for_tests(Vec::new());
	let (error, context) = error_output(handle_report(&session, &template_config(), &[]).unwrap());
	assert!(error.contains("No session file"), "{error}");
	assert!(
		context["hint"].as_str().expect("hint").contains("auto-saved"),
		"{context}"
	);
}
