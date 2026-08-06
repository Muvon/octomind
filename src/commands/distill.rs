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

//! `octomind distill` — background lesson extraction for a session that just
//! exited. Spawned detached by the interactive exit path so the user's shell
//! returns immediately; not meant to be run by hand.

use anyhow::{Context, Result};
use clap::Args;
use octomind::config::Config;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct DistillArgs {
	/// JSON transcript snapshot written by the exiting session. Deleted after reading.
	#[arg(long, value_name = "PATH")]
	pub messages: PathBuf,

	/// Role the session ran under.
	#[arg(long, default_value = "")]
	pub role: String,

	/// Project scope the lessons are stored under.
	#[arg(long, default_value = "")]
	pub project: String,

	/// Session name recorded on each stored lesson.
	#[arg(long, default_value = "")]
	pub session: String,
}

pub async fn execute(args: &DistillArgs, config: &Config) -> Result<()> {
	let raw = std::fs::read(&args.messages).with_context(|| {
		format!(
			"failed to read transcript snapshot {}",
			args.messages.display()
		)
	})?;
	let _ = std::fs::remove_file(&args.messages);

	let messages: Vec<octomind::session::Message> =
		serde_json::from_slice(&raw).context("failed to parse transcript snapshot")?;

	let stored = octomind::supervisor::learning::extract::run_extraction(
		&messages,
		config,
		&args.role,
		&args.project,
		&args.session,
	)
	.await?;

	if stored > 0 {
		octomind::supervisor::notify(&format!("distilled {stored} lesson(s)"));
	}
	Ok(())
}
