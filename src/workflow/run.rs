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

//! Workflow executor: drives the step graph, manages session IDs,
//! aggregates stats, prints progress to stderr.

use anyhow::{bail, Result};
use colored::Colorize;
use regex::Regex;
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

use super::proc::{run_step, send_done, RunOutcome, StepStats};
use super::schema::{
	Condition, ConditionalStep, LoopStep, ParallelStep, Sequential, SessionMode, Step, WorkflowDef,
};
use super::validate;

/// Final summed totals printed once at the end.
#[derive(Debug, Default, Clone, Copy)]
struct Totals {
	duration: Duration,
	cost: f64,
	tokens: u64,
}

impl Totals {
	fn add(&mut self, s: &StepStats) {
		self.duration += s.duration;
		self.cost += s.cost;
		self.tokens += s.total_tokens;
	}
}

/// Per-workflow execution state.
struct Executor {
	outputs: HashMap<String, String>,
	/// step_name → persistent octomind session name (for `session = "continue"`).
	session_ids: HashMap<String, String>,
	/// Tracks whether a given continue-session has been used at least once,
	/// so we know when to send `/done` before resuming.
	used_continue: HashMap<String, bool>,
	totals: Totals,
	/// Last sequentially-completed step name (for unnamed condition output).
	last_step: Option<String>,
	wf_name: String,
}

impl Executor {
	fn new(wf_name: String) -> Self {
		Self {
			outputs: HashMap::new(),
			session_ids: HashMap::new(),
			used_continue: HashMap::new(),
			totals: Totals::default(),
			last_step: None,
			wf_name,
		}
	}

	/// Resolve `{{var}}` against current outputs (and `{{input}}`).
	fn substitute(&self, prompt: &str, input: &str) -> String {
		let re = validate::var_regex();
		re.replace_all(prompt, |caps: &regex::Captures| {
			let var = &caps[1];
			if var == "input" {
				input.to_string()
			} else {
				self.outputs.get(var).cloned().unwrap_or_default()
			}
		})
		.into_owned()
	}

	/// Drive one sequential step with retries / session handling.
	///
	/// `progress_prefix` is what gets stamped on each running/done line on
	/// stderr (e.g. `"  "` for top-level, `"  refine-loop [2/5] "` for loop).
	async fn exec_sequential(
		&mut self,
		s: &Sequential,
		input: &str,
		progress_prefix: &str,
	) -> Result<StepStats> {
		let templated_prompt = self.substitute(&s.prompt, input);
		let max_attempts = s.retries + 1;
		let mut last_err: Option<String> = None;

		for attempt in 1..=max_attempts {
			eprintln_progress(&format!(
				"{prefix}{arrow} {name:<14} running...",
				prefix = progress_prefix,
				arrow = "►".bright_blue(),
				name = s.name,
			));

			// Resolve session name policy.
			let session_name: Option<String> = match s.session {
				SessionMode::Fresh => None,
				SessionMode::Continue => {
					let id = self
						.session_ids
						.entry(s.name.clone())
						.or_insert_with(|| {
							format!("wf-{}-{}-{}", sanitize(&self.wf_name), s.name, short_uuid())
						})
						.clone();
					// If this session has been used before, compress it with /done first.
					if *self.used_continue.get(&s.name).unwrap_or(&false) {
						let _ = send_done(&id).await;
					}
					Some(id)
				}
			};

			// Prompt selection:
			//   - Fresh session OR first use of a Continue session → templated prompt.
			//   - Subsequent invocation of a Continue session (loop iter 2+ or retry)
			//     → the session already holds the full templated context; just feed it
			//     the most recent prior step's output as a nudge to drive the next
			//     turn. This matches the GAN-style refine pattern where the only
			//     thing that needs to change between rounds is the reviewer's verdict.
			let prompt_for_run = if s.session == SessionMode::Continue
				&& *self.used_continue.get(&s.name).unwrap_or(&false)
			{
				self.last_step
					.as_ref()
					.and_then(|n| self.outputs.get(n))
					.cloned()
					.unwrap_or_else(|| templated_prompt.clone())
			} else {
				templated_prompt.clone()
			};

			let outcome =
				run_step(&s.role, &prompt_for_run, session_name.as_deref(), s.timeout).await;

			match outcome {
				RunOutcome::Ok(stats) => {
					if s.session == SessionMode::Continue {
						self.used_continue.insert(s.name.clone(), true);
					}
					eprintln_progress(&format!(
						"{prefix}{tick} {name:<14} {dur:>5}  ${cost:.4}  {tok} tok",
						prefix = progress_prefix,
						tick = "✓".green(),
						name = s.name,
						dur = fmt_dur(stats.duration),
						cost = stats.cost,
						tok = stats.total_tokens,
					));
					self.totals.add(&stats);
					return Ok(stats);
				}
				RunOutcome::Empty(stats) => {
					self.totals.add(&stats);
					last_err = Some(format!(
						"produced no assistant output (attempt {attempt}/{max_attempts})"
					));
				}
				RunOutcome::NonZero { stats, code } => {
					self.totals.add(&stats);
					last_err = Some(format!(
						"failed exit code {code:?} (attempt {attempt}/{max_attempts})"
					));
				}
				RunOutcome::Timeout(elapsed) => {
					last_err = Some(format!(
						"timed out after {}s (attempt {attempt}/{max_attempts})",
						elapsed.as_secs()
					));
				}
				RunOutcome::SpawnError(e) => {
					last_err = Some(format!("spawn error: {e}"));
				}
			}

			eprintln_progress(&format!(
				"{prefix}{cross} {name:<14} {msg}",
				prefix = progress_prefix,
				cross = "✗".red(),
				name = s.name,
				msg = last_err.as_deref().unwrap_or("failed"),
			));
		}

		bail!(
			"step '{}' failed after {} attempts: {}",
			s.name,
			max_attempts,
			last_err.unwrap_or_else(|| "unknown".into())
		);
	}

	async fn exec_parallel(&mut self, p: &ParallelStep, input: &str) -> Result<()> {
		// Substitute every sub-step's prompt up-front against the SAME
		// outer scope — sub-steps cannot reference each other.
		let prepared: Vec<(Sequential, String)> = p
			.run
			.iter()
			.map(|s| (s.clone(), self.substitute(&s.prompt, input)))
			.collect();

		// We can't borrow &mut self across the join, so run each sub-step
		// in isolation here and collect into a tiny snapshot.
		// Implementation: launch all in parallel using join_all, but each
		// task owns its own Sequential copy and we DON'T touch self.
		let mut handles = Vec::new();
		for (s, prompt) in prepared {
			let sname = s.name.clone();
			let role = s.role.clone();
			let timeout = s.timeout;
			let retries = s.retries;
			// Parallel sub-steps cannot use `session = "continue"` semantics
			// across iterations of an outer loop because there is no outer
			// loop concept here — they get fresh sessions per call.
			handles.push(tokio::spawn(async move {
				let mut last_err: Option<String> = None;
				let max_attempts = retries + 1;
				for attempt in 1..=max_attempts {
					let outcome = run_step(&role, &prompt, None, timeout).await;
					match outcome {
						RunOutcome::Ok(stats) => return Ok::<_, String>((sname, stats)),
						RunOutcome::Empty(s) => {
							last_err =
								Some(format!("empty output (attempt {attempt}/{max_attempts})"));
							let _ = s;
						}
						RunOutcome::NonZero { code, .. } => {
							last_err = Some(format!(
								"non-zero exit {code:?} (attempt {attempt}/{max_attempts})"
							));
						}
						RunOutcome::Timeout(e) => {
							last_err = Some(format!(
								"timed out after {}s (attempt {attempt}/{max_attempts})",
								e.as_secs()
							));
						}
						RunOutcome::SpawnError(e) => {
							last_err = Some(format!("spawn error: {e}"));
						}
					}
				}
				Err(format!("'{sname}' {}", last_err.unwrap_or_default()))
			}));
		}

		eprintln_progress(&format!(
			"  {arrow} {name:<14} running {n} sub-steps in parallel...",
			arrow = "►".bright_blue(),
			name = p.name,
			n = p.run.len(),
		));

		let results = futures::future::join_all(handles).await;
		for r in results {
			match r {
				Ok(Ok((name, stats))) => {
					eprintln_progress(&format!(
						"  {tick} {name:<14} {dur:>5}  ${cost:.4}  {tok} tok",
						tick = "✓".green(),
						name = name,
						dur = fmt_dur(stats.duration),
						cost = stats.cost,
						tok = stats.total_tokens,
					));
					self.totals.add(&stats);
					self.outputs.insert(name.clone(), stats.output);
					self.last_step = Some(name);
				}
				Ok(Err(e)) => bail!("parallel step '{}' failed: {}", p.name, e),
				Err(e) => bail!("parallel step '{}' panicked: {}", p.name, e),
			}
		}
		Ok(())
	}

	async fn exec_loop(&mut self, l: &LoopStep, input: &str) -> Result<()> {
		let max = l.max_iterations;
		for i in 1..=max {
			for sub in &l.run {
				let prefix = format!("  {name} [{i}/{max}] ", name = l.name.bright_magenta());
				let stats = self.exec_sequential(sub, input, &prefix).await?;
				self.outputs.insert(sub.name.clone(), stats.output);
				self.last_step = Some(sub.name.clone());
			}

			// Check exit_when (validated to be Some during pre-flight).
			let exit_when = l
				.exit_when
				.as_ref()
				.expect("validate() guarantees exit_when is set for loop steps");
			let target = match &exit_when.output {
				Some(n) => n.clone(),
				None => self
					.last_step
					.clone()
					.unwrap_or_else(|| l.run.last().unwrap().name.clone()),
			};
			if let Some(value) = self.outputs.get(&target) {
				if condition_matches(exit_when, value) {
					eprintln_progress(&format!(
						"  {ok} exit condition matched at iteration {i}",
						ok = "✓".green()
					));
					return Ok(());
				}
			}
		}
		eprintln!(
			"  {warn} loop '{name}' reached max_iterations ({max}) without exit condition matching",
			warn = "⚠".yellow(),
			name = l.name,
			max = max,
		);
		Ok(())
	}

	async fn exec_conditional(&mut self, c: &ConditionalStep, input: &str) -> Result<()> {
		let target = match &c.condition.output {
			Some(n) => n.clone(),
			None => match &self.last_step {
				Some(n) => n.clone(),
				None => bail!(
					"conditional step '{}': no prior step output to test",
					c.name
				),
			},
		};
		let value = self.outputs.get(&target).cloned().unwrap_or_default();
		let matched = condition_matches(&c.condition, &value);

		let branch_names: &[String] = if matched { &c.on_match } else { &c.on_no_match };
		eprintln_progress(&format!(
			"  {arrow} {name:<14} condition {res} → running [{branch}]",
			arrow = "►".bright_blue(),
			name = c.name,
			res = if matched {
				"true".green()
			} else {
				"false".yellow()
			},
			branch = branch_names.join(", "),
		));

		let chosen: Vec<&Sequential> = c
			.run
			.iter()
			.filter(|s| branch_names.iter().any(|n| n == &s.name))
			.collect();
		let skipped: Vec<&Sequential> = c
			.run
			.iter()
			.filter(|s| !branch_names.iter().any(|n| n == &s.name))
			.collect();

		for s in chosen {
			let stats = self.exec_sequential(s, input, "    ").await?;
			self.outputs.insert(s.name.clone(), stats.output);
			self.last_step = Some(s.name.clone());
		}
		// Skipped branch outputs resolve to empty string.
		for s in skipped {
			self.outputs.entry(s.name.clone()).or_default();
		}
		Ok(())
	}
}

fn condition_matches(cond: &Condition, value: &str) -> bool {
	if let Some(needle) = &cond.contains {
		if value.contains(needle) {
			return true;
		}
	}
	if let Some(pat) = &cond.matches {
		if let Ok(re) = Regex::new(pat) {
			if re.is_match(value) {
				return true;
			}
		}
	}
	false
}

fn fmt_dur(d: Duration) -> String {
	let secs = d.as_secs_f64();
	if secs < 60.0 {
		format!("{secs:.1}s")
	} else {
		let m = (secs / 60.0) as u64;
		let s = secs - (m as f64 * 60.0);
		format!("{m}m{s:02.0}s")
	}
}

fn short_uuid() -> String {
	Uuid::new_v4()
		.to_string()
		.split('-')
		.next()
		.unwrap_or("0000")
		.to_string()
}

fn sanitize(s: &str) -> String {
	s.chars()
		.map(|c| {
			if c.is_ascii_alphanumeric() || c == '-' {
				c
			} else {
				'-'
			}
		})
		.collect()
}

/// `eprintln!` wrapper — workflow progress always goes to stderr so
/// the final stdout stream is clean for piping.
fn eprintln_progress(line: &str) {
	eprintln!("{line}");
}

/// Public entry — runs a fully-validated workflow.
///
/// Returns the text that should be written to stdout (the resolved
/// `result` step's output, or the last step if `result` is unset).
pub async fn execute(wf: &WorkflowDef, input: &str) -> Result<String> {
	let mut ex = Executor::new(wf.name.clone());

	eprintln!(
		"{open} workflow: {name}",
		open = "╭".bright_black(),
		name = wf.name.bright_cyan()
	);

	let mut last_top_level: Option<String> = None;

	for step in &wf.steps {
		match step {
			Step::Sequential(s) => {
				let stats = ex.exec_sequential(s, input, "  ").await?;
				ex.outputs.insert(s.name.clone(), stats.output);
				ex.last_step = Some(s.name.clone());
				last_top_level = Some(s.name.clone());
			}
			Step::Parallel(p) => {
				ex.exec_parallel(p, input).await?;
				last_top_level = Some(p.name.clone());
			}
			Step::Loop(l) => {
				ex.exec_loop(l, input).await?;
				last_top_level = Some(l.name.clone());
			}
			Step::Conditional(c) => {
				ex.exec_conditional(c, input).await?;
				last_top_level = Some(c.name.clone());
			}
		}
	}

	eprintln!(
		"{close} Total: {dur}  ${cost:.4}  {tok} tok",
		close = "╰".bright_black(),
		dur = fmt_dur(ex.totals.duration),
		cost = ex.totals.cost,
		tok = ex.totals.tokens,
	);

	// Resolve final output.
	let result_name = wf
		.result
		.clone()
		.or(last_top_level)
		.ok_or_else(|| anyhow::anyhow!("no steps produced output"))?;
	let out = ex
		.outputs
		.get(&result_name)
		.cloned()
		.ok_or_else(|| anyhow::anyhow!("result step '{}' produced no output", result_name))?;
	Ok(out)
}
