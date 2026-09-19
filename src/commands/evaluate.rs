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

//! `octomind evaluate` — one-shot call to the evaluation model. Reads a JSON
//! request (`state` plus typed `questions`) from a file or stdin and prints
//! the calibrated answers as JSON to stdout; model and usage go to stderr.

use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;
use octolib::evaluation::{EvaluationRequest, Question};
use octomind::config::Config;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

#[derive(Args, Debug)]
pub struct EvaluateArgs {
	/// Evaluation model as `provider:model` (e.g. `typesafe:jev-latest`).
	/// Defaults to `[supervisor.evaluate].model` from config.
	#[arg(long, short = 'm', value_name = "MODEL")]
	pub model: Option<String>,

	/// JSON request file: `{"state": ..., "questions": {"id": {"type": "noul", "instructions": "..."}}}`.
	/// If omitted, reads from stdin.
	#[arg(value_name = "FILE")]
	pub file: Option<PathBuf>,
}

/// Wire request minus the model, which comes from `-m` or config.
#[derive(Debug, Deserialize)]
struct Input {
	state: serde_json::Value,
	questions: BTreeMap<String, Question>,
}

pub async fn execute(args: &EvaluateArgs, config: &Config) -> Result<()> {
	let input = parse_input(&read_input(args.file.as_deref())?)?;
	let model = args
		.model
		.as_deref()
		.unwrap_or(&config.supervisor.evaluate.model);

	let mut request = EvaluationRequest::new(input.state);
	request.questions = input.questions;
	let started = std::time::Instant::now();
	let response = octolib::evaluation::evaluate(model, request).await?;

	// Pretty on a terminal, one compact record when piped so `jq` and
	// line-oriented tools get a single line.
	let answers = if io::stdout().is_terminal() {
		serde_json::to_string_pretty(&response.answers)?
	} else {
		serde_json::to_string(&response.answers)?
	};
	println!("{answers}");
	eprintln!(
		"{} {} {} {} in / {} out {} ${:.4} {} {}ms",
		"evaluate".bright_black(),
		model.bright_cyan(),
		"·".bright_black(),
		response.usage.input_tokens,
		response.usage.output_tokens,
		"·".bright_black(),
		response.usage.cost.unwrap_or(0.0),
		"·".bright_black(),
		started.elapsed().as_millis(),
	);
	Ok(())
}

fn parse_input(raw: &str) -> Result<Input> {
	let input: Input = serde_json::from_str(raw).context("invalid request JSON")?;
	if input.questions.is_empty() {
		bail!("request has no questions");
	}
	Ok(input)
}

fn read_input(file: Option<&Path>) -> Result<String> {
	match file {
		Some(path) => std::fs::read_to_string(path)
			.with_context(|| format!("failed to read {}", path.display())),
		None => {
			if io::stdin().is_terminal() {
				bail!("request must be passed as a file or piped via stdin");
			}
			let mut buf = String::new();
			io::stdin()
				.read_to_string(&mut buf)
				.context("failed to read stdin")?;
			Ok(buf)
		}
	}
}

#[cfg(test)]
#[path = "evaluate_tests.rs"]
mod tests;
