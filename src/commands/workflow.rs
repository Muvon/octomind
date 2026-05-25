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

//! `octomind workflow <file.toml>` — external workflow orchestrator CLI.

use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use octomind::workflow::{
	execute_workflow,
	schema::{Step, WorkflowDef},
	validate,
};

#[derive(Args, Debug)]
pub struct WorkflowArgs {
	/// Path to the workflow TOML file.
	#[arg(value_name = "FILE")]
	pub file: PathBuf,

	/// Validate and print the execution plan to stdout without running any steps.
	#[arg(long)]
	pub dry_run: bool,
}

pub async fn execute(args: &WorkflowArgs) -> Result<()> {
	if !args.file.exists() {
		bail!("workflow file not found: {}", args.file.display());
	}

	let raw = std::fs::read_to_string(&args.file)
		.with_context(|| format!("failed to read {}", args.file.display()))?;
	let wf: WorkflowDef =
		toml::from_str(&raw).with_context(|| format!("failed to parse {}", args.file.display()))?;

	validate::validate(&wf)?;

	if args.dry_run {
		print_plan(&wf);
		return Ok(());
	}

	// Read stdin (required when not a dry-run).
	if std::io::stdin().is_terminal() {
		bail!("workflow requires input via stdin");
	}
	let mut input = String::new();
	io::stdin()
		.read_to_string(&mut input)
		.context("failed to read stdin")?;
	let input = input.trim().to_string();
	if input.is_empty() {
		bail!("workflow requires input via stdin");
	}

	let result = execute_workflow(&wf, &input).await?;
	// Final output to stdout — clean for piping.
	println!("{result}");
	Ok(())
}

fn print_plan(wf: &WorkflowDef) {
	println!("{} {}", "workflow:".bright_black(), wf.name.bright_cyan());
	if let Some(desc) = &wf.description {
		println!("  {} {}", "description:".bright_black(), desc);
	}
	if let Some(r) = &wf.result {
		println!("  {} {}", "result:".bright_black(), r);
	} else {
		println!(
			"  {} {}",
			"result:".bright_black(),
			"(last step)".bright_black()
		);
	}
	println!();

	for (i, step) in wf.steps.iter().enumerate() {
		print_step(i + 1, step, 0);
	}
}

fn print_step(idx: usize, step: &Step, depth: usize) {
	let indent = "  ".repeat(depth + 1);
	match step {
		Step::Sequential(s) => {
			println!(
				"{indent}{idx}. {name}  {kind}",
				idx = idx,
				name = s.name.bright_white(),
				kind = "[sequential]".bright_black(),
			);
			println!("{indent}   role: {}", s.role);
			println!(
				"{indent}   session: {:?}  timeout: {}s  retries: {}",
				s.session, s.timeout, s.retries
			);
			println!("{indent}   prompt: {}", truncate(&s.prompt, 120));
		}
		Step::Parallel(p) => {
			println!(
				"{indent}{idx}. {name}  {kind}",
				idx = idx,
				name = p.name.bright_white(),
				kind = "[parallel]".bright_magenta(),
			);
			for (i, sub) in p.run.iter().enumerate() {
				print_sub(i + 1, sub, depth + 1);
			}
		}
		Step::Loop(l) => {
			println!(
				"{indent}{idx}. {name}  {kind}  max_iterations={mx}",
				idx = idx,
				name = l.name.bright_white(),
				kind = "[loop]".bright_yellow(),
				mx = l.max_iterations,
			);
			match &l.exit_when {
				Some(c) => println!(
					"{indent}   exit_when: output={:?} contains={:?} matches={:?}",
					c.output, c.contains, c.matches
				),
				None => println!("{indent}   exit_when: <missing>"),
			}
			for (i, sub) in l.run.iter().enumerate() {
				print_sub(i + 1, sub, depth + 1);
			}
		}
		Step::Conditional(c) => {
			println!(
				"{indent}{idx}. {name}  {kind}",
				idx = idx,
				name = c.name.bright_white(),
				kind = "[conditional]".bright_blue(),
			);
			println!(
				"{indent}   condition: output={:?} contains={:?} matches={:?}",
				c.condition.output, c.condition.contains, c.condition.matches
			);
			println!("{indent}   on_match:    {:?}", c.on_match);
			println!("{indent}   on_no_match: {:?}", c.on_no_match);
			for (i, sub) in c.run.iter().enumerate() {
				print_sub(i + 1, sub, depth + 1);
			}
		}
	}
}

fn print_sub(idx: usize, s: &octomind::workflow::schema::Sequential, depth: usize) {
	let indent = "  ".repeat(depth + 1);
	println!(
		"{indent}{idx}. {name}  role={role}  session={sess:?}  timeout={t}s  retries={r}",
		idx = idx,
		name = s.name.bright_white(),
		role = s.role,
		sess = s.session,
		t = s.timeout,
		r = s.retries,
	);
	println!("{indent}   prompt: {}", truncate(&s.prompt, 120));
}

fn truncate(s: &str, n: usize) -> String {
	let one_line = s.replace('\n', " ");
	if one_line.chars().count() <= n {
		one_line
	} else {
		let head: String = one_line.chars().take(n).collect();
		format!("{head}…")
	}
}
