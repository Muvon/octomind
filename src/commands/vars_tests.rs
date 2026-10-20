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

use super::{placeholder_description, VarsArgs};
use clap::Parser;

#[derive(Parser)]
struct Cli {
	#[command(flatten)]
	args: VarsArgs,
}

#[test]
fn vars_describes_home_and_session_role_placeholders() {
	assert_eq!(placeholder_description("{{HOME}}"), "User home directory");
	assert_eq!(
		placeholder_description("{{ROLE}}"),
		"Active session role, resolved separately for each session"
	);
}

#[test]
fn preview_and_expand_are_mutually_exclusive() {
	assert!(Cli::try_parse_from(["octomind", "--preview", "--expand"]).is_err());
	assert!(Cli::try_parse_from(["octomind", "--expand", "--preview"]).is_err());
	assert!(Cli::try_parse_from(["octomind", "--preview"]).is_ok());
	assert!(Cli::try_parse_from(["octomind", "--expand"]).is_ok());
}
