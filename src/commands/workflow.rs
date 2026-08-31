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

//! `octomind workflow <name|file.toml>` — external workflow orchestrator CLI.
//!
//! Resolution mirrors `octomind run`: a bare NAME (e.g. `my-workflow`) is
//! fetched from taps (`<tap>/workflows/<name>.toml`) and validated to use only
//! public tap roles; an existing path / `*.toml` is loaded as a local file with
//! no role restriction. With no argument, lists available tap workflows.

use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;
use std::collections::HashSet;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use octomind::agent::{registry, taps};
use octomind::config::Config;
use octomind::workflow::{
	execute_workflow,
	schema::{Step, WorkflowDef},
	validate,
};

#[derive(Args, Debug)]
pub struct WorkflowArgs {
	/// Workflow to run: a tap workflow NAME (e.g. `my-workflow`, fetched from
	/// taps) or a path to a local TOML file. Omit to list available tap workflows.
	#[arg(value_name = "NAME")]
	pub name: Option<String>,

	/// Validate and print the execution plan to stdout without running any steps.
	#[arg(long)]
	pub dry_run: bool,

	/// Machine-readable output on stdout. With `jsonl`, emit one structured
	/// `assistant` event per step as it completes (the last is the final result)
	/// plus a trailing aggregated `cost` event. Without it, stdout is empty
	/// (except the `--dry-run` plan). Per-step progress always goes to stderr.
	#[arg(long = "format")]
	pub format: Option<String>,
}

pub async fn execute(args: &WorkflowArgs, config: &Config) -> Result<()> {
	// No target → list available tap workflows (discovery).
	let Some(target) = args.name.as_deref() else {
		return list_workflows();
	};

	// Resolve source: an existing local file vs. a tap workflow name.
	let path = PathBuf::from(target);
	let (raw, from_tap) = if path.exists() {
		let raw = std::fs::read_to_string(&path)
			.with_context(|| format!("failed to read {}", path.display()))?;
		(raw, false)
	} else if looks_like_path(target) {
		bail!("workflow file not found: {}", path.display());
	} else {
		let (raw, source_tap) = taps::fetch_workflow(target)
			.with_context(|| format!("failed to fetch workflow '{target}' from taps"))?;
		eprintln!(
			"{} {} {} {}",
			"workflow".bright_black(),
			target.bright_cyan(),
			"·".bright_black(),
			format!("from {source_tap}").bright_black(),
		);
		(raw, true)
	};

	let wf: WorkflowDef =
		toml::from_str(&raw).with_context(|| format!("failed to parse workflow '{target}'"))?;

	validate::validate(&wf)?;

	// Public workflows (fetched from taps) may only reference public tap roles.
	if from_tap {
		let public_roles: HashSet<String> = taps::list_agent_tags()
			.context("failed to enumerate tap roles")?
			.into_iter()
			.collect();
		validate::validate_public_roles(&wf, &public_roles)?;
	}

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

	execute_workflow(&wf, &input, config, args.format.as_deref()).await?;
	Ok(())
}

/// True when the argument is clearly meant as a filesystem path rather than a
/// bare tap workflow name — contains a path separator or a `.toml` extension.
fn looks_like_path(arg: &str) -> bool {
	arg.contains('/') || arg.contains('\\') || arg.ends_with(".toml")
}

/// `octomind workflow` with no argument — list public workflows from taps.
fn list_workflows() -> Result<()> {
	let workflows =
		registry::list_all_tap_workflows().context("failed to enumerate tap workflows")?;
	if workflows.is_empty() {
		println!(
			"{}",
			"No tap workflows installed. Add a tap with `octomind tap user/repo`.".bright_black()
		);
		return Ok(());
	}
	println!("{}", "available workflows".bright_black());
	let name_width = workflows
		.iter()
		.map(|w| w.name.len())
		.max()
		.unwrap_or(0)
		.min(40);
	for w in &workflows {
		// Pad the plain name before coloring so ANSI codes don't break alignment.
		let padded = format!("{:<width$}", w.name, width = name_width);
		let desc = if w.description.is_empty() {
			String::new()
		} else {
			format!("  {}", w.description.bright_black())
		};
		println!(
			"  {name}{desc}  {src}",
			name = padded.bright_cyan(),
			desc = desc,
			src = format!("({})", w.source_tap).bright_black(),
		);
	}
	Ok(())
}

fn print_plan(wf: &WorkflowDef) {
	println!("{} {}", "workflow:".bright_black(), wf.name.bright_cyan());
	if let Some(desc) = &wf.description {
		println!("  {} {}", "description:".bright_black(), desc);
	}
	if let Some(cap) = wf.max_cost {
		println!("  {} ${cap:.4}", "max_cost:".bright_black());
	}
	if wf.is_graph() {
		println!("  {} graph", "mode:".bright_black());
		println!(
			"  {} {}",
			"entry:".bright_black(),
			wf.entry.as_deref().expect("validated graph entry")
		);
		println!(
			"  {} {}",
			"max_transitions:".bright_black(),
			wf.graph_max_transitions()
		);
	}
	println!();

	for (i, step) in wf.steps.iter().enumerate() {
		print_step(i + 1, step, 0);
	}
	if wf.is_graph() {
		println!();
		println!("{}", "routes:".bright_black());
		for edge in &wf.edges {
			let condition = match &edge.when {
				Some(c) => format!(
					"  when output={:?} contains={:?} matches={:?}",
					c.output, c.contains, c.matches
				),
				None => "  default".to_string(),
			};
			println!("  {} -> {}{}", edge.from, edge.to, condition.bright_black());
		}
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
			let mut meta = format!(
				"session: {:?}  timeout: {}s  retries: {}",
				s.session, s.timeout, s.retries
			);
			if let Some(m) = &s.model.model {
				meta = format!("model: {m}  {meta}");
			}
			if let Some(w) = &s.workdir {
				meta = format!("{meta}  workdir: {w}");
			}
			println!("{indent}   {meta}");
			println!("{indent}   prompt: {}", truncate(&s.prompt, 120));
		}
		Step::Parallel(p) => {
			let kind = if p.match_pattern.is_some() {
				"[parallel · dynamic]"
			} else {
				"[parallel]"
			};
			println!(
				"{indent}{idx}. {name}  {kind}",
				idx = idx,
				name = p.name.bright_white(),
				kind = kind.bright_magenta(),
			);
			let mut meta = if let Some(pat) = &p.match_pattern {
				let source = p.source.as_deref().expect("validated dynamic source");
				format!("source={source:?}  match={pat:?}  runs=per-match")
			} else {
				let total: u32 = p.run.iter().map(|s| s.replica_count()).sum();
				format!("sub-steps={}  total_runs={total}", p.run.len())
			};
			if let Some(m) = p.min_success {
				meta = format!("{meta}  min_success={m}");
			}
			if let Some(mp) = p.max_parallel {
				meta = format!("{meta}  max_parallel={mp}");
			}
			println!("{indent}   {meta}");
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
	let mut meta = format!(
		"role={role}  session={sess:?}  timeout={t}s  retries={r}",
		role = s.role,
		sess = s.session,
		t = s.timeout,
		r = s.retries,
	);
	if let Some(m) = &s.model.model {
		meta = format!("model={m}  {meta}");
	}
	if let Some(w) = &s.workdir {
		meta = format!("{meta}  workdir={w}");
	}
	if let Some(c) = s.count {
		meta = format!("{meta}  count={c}");
	}
	println!(
		"{indent}{idx}. {name}  {meta}",
		idx = idx,
		name = s.name.bright_white(),
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

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
