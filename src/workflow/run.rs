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
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::time::{Duration, Instant};
use uuid::Uuid;

use super::proc::{run_step, send_done, RunOutcome, RunStepArgs, StepStats};
use super::schema::{
	Condition, ConditionalStep, LoopStep, ParallelStep, Sequential, SessionMode, Step, WorkflowDef,
};
use super::validate;
use crate::config::Config;
use crate::session::chat::markdown::{is_markdown_content, MarkdownRenderer};

/// Final summed totals printed once at the end.
#[derive(Debug, Default, Clone, Copy)]
struct Totals {
	duration: Duration,
	cost: f64,
	tokens: u64,
	tools: u64,
	tools_failed: u64,
}

impl Totals {
	fn add(&mut self, s: &StepStats) {
		self.duration += s.duration;
		self.cost += s.cost;
		self.tokens += s.total_tokens;
		self.tools += s.tool_count;
		self.tools_failed += s.tool_failed;
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
	/// True when stderr is a TTY — use animated spinner per step.
	/// False when piped/redirected — stream one event per line.
	interactive: bool,
	/// Workflow start instant — passed to `run_step` so the spinner can
	/// show total elapsed time across all completed + current steps.
	started: Instant,
	/// Honor `config.enable_markdown_rendering` when printing step responses.
	markdown_enabled: bool,
	/// Theme name from `config.markdown_theme` (parsed lazily).
	markdown_theme: String,
}

impl Executor {
	fn new(wf_name: String, config: &Config) -> Self {
		Self {
			outputs: HashMap::new(),
			session_ids: HashMap::new(),
			used_continue: HashMap::new(),
			totals: Totals::default(),
			last_step: None,
			wf_name,
			interactive: std::io::stderr().is_terminal(),
			started: Instant::now(),
			markdown_enabled: config.enable_markdown_rendering,
			markdown_theme: config.markdown_theme.clone(),
		}
	}

	/// Resolve a step's prompt the same way chat sessions resolve user
	/// input. Three passes, in order:
	///
	/// 1. Workflow-specific `{{var}}` — `{{input}}` and prior step names
	///    from `self.outputs`. Unknown `{{var}}` are preserved literally
	///    so the next pass can claim its built-ins.
	/// 2. `process_placeholders_async` — the canonical chat helper that
	///    expands `{{DATE}} {{CWD}} {{SHELL}} {{OS}} {{BINARIES}}
	///    {{ROLE}} {{SYSTEM}} {{CONTEXT}} {{GIT_STATUS}} {{GIT_TREE}}
	///    {{README}}`.
	/// 3. `expand_context_blocks` — replaces any `<context>path</context>`
	///    or `<context>path:start:end</context>` blocks with the actual
	///    file contents rendered as XML, same as chat's compression /
	///    file-context path. Lets a step emit a context block in its
	///    response and have the next step receive the inlined file.
	async fn substitute(&self, prompt: &str, input: &str) -> String {
		let re = validate::var_regex();
		let after_wf = re
			.replace_all(prompt, |caps: &regex::Captures| {
				let var = &caps[1];
				if var == "input" {
					input.to_string()
				} else if let Some(val) = self.outputs.get(var) {
					val.clone()
				} else {
					caps.get(0).unwrap().as_str().to_string()
				}
			})
			.into_owned();

		let project_dir = crate::mcp::get_thread_working_directory();
		let after_placeholders =
			crate::session::helper_functions::process_placeholders_async(&after_wf, &project_dir)
				.await;
		crate::utils::file_renderer::expand_context_blocks(&after_placeholders)
	}

	/// Drive one sequential step with retries / session handling.
	///
	/// `header_suffix` is appended after the step name in the `╭ name`
	/// title and `╰ ✓ name` close — empty for top-level, `"  [i/max]
	/// loop-name"` inside a loop, etc. The block is opened with
	/// [`box_open`] and closed via [`box_close_ok`] / [`box_close_err`].
	async fn exec_sequential(
		&mut self,
		s: &Sequential,
		input: &str,
		header_suffix: &str,
	) -> Result<StepStats> {
		let templated_prompt = self.substitute(&s.prompt, input).await;
		let max_attempts = s.retries + 1;
		let mut last_err: Option<String> = None;

		for attempt in 1..=max_attempts {
			let attempt_tag = if max_attempts > 1 {
				format!(
					"  {}",
					format!("(attempt {attempt}/{max_attempts})").bright_black()
				)
			} else {
				String::new()
			};
			box_open(&format!(
				"{name}{suffix}{attempt}",
				name = s.name.bright_white(),
				suffix = header_suffix,
				attempt = attempt_tag,
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

			let event_prefix = format!("{} ", "│".bright_black());
			let spinner = if self.interactive {
				let sp = ProgressBar::new_spinner();
				sp.set_style(
					ProgressStyle::default_spinner()
						.template("{prefix} {spinner:.cyan} {msg}")
						.unwrap()
						.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
				);
				sp.set_prefix(format!("{}", "│".bright_black()));
				sp.set_message("starting…".bright_black().to_string());
				sp.enable_steady_tick(Duration::from_millis(80));
				Some(sp)
			} else {
				None
			};

			let has_spinner = spinner.is_some();
			let args = RunStepArgs {
				role: s.role.clone(),
				prompt: prompt_for_run,
				session_name,
				model: s.model.clone(),
				timeout_secs: s.timeout,
				event_prefix: if has_spinner {
					None
				} else {
					Some(event_prefix)
				},
				spinner,
				wf_start: self.started,
				prior_cost: self.totals.cost,
				prior_tools: self.totals.tools,
			};
			let outcome = run_step(args).await;

			match outcome {
				RunOutcome::Ok(stats) => {
					if s.session == SessionMode::Continue {
						self.used_continue.insert(s.name.clone(), true);
					}
					box_close_ok(&s.name.bright_white(), &fmt_stats(&stats));
					print_response(&stats.output, self.markdown_enabled, &self.markdown_theme);
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

			box_close_err(
				&s.name.bright_white(),
				last_err.as_deref().unwrap_or("failed"),
			);
			eprintln!();
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
		// outer scope — sub-steps cannot reference each other. Each
		// substitution may touch disk (project context placeholders), so
		// we collect sequentially before kicking off the parallel tasks.
		let mut prepared: Vec<(Sequential, String)> = Vec::with_capacity(p.run.len());
		for s in &p.run {
			let resolved = self.substitute(&s.prompt, input).await;
			prepared.push((s.clone(), resolved));
		}

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
			let model = s.model.clone();
			// Parallel sub-steps cannot use `session = "continue"` semantics
			// across iterations of an outer loop because there is no outer
			// loop concept here — they get fresh sessions per call.
			handles.push(tokio::spawn(async move {
				let mut last_err: Option<String> = None;
				let max_attempts = retries + 1;
				for attempt in 1..=max_attempts {
					let args = RunStepArgs {
						role: role.clone(),
						prompt: prompt.clone(),
						session_name: None,
						model: model.clone(),
						timeout_secs: timeout,
						event_prefix: None,
						spinner: None,
						wf_start: Instant::now(),
						prior_cost: 0.0,
						prior_tools: 0,
					};
					let outcome = run_step(args).await;
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

		box_open(&format!(
			"{name}  {tag}",
			name = p.name.bright_white(),
			tag = format!("({} in parallel)", p.run.len()).bright_black(),
		));

		let results = futures::future::join_all(handles).await;
		let mut sub_outputs: Vec<(String, String)> = Vec::new();
		for r in results {
			match r {
				Ok(Ok((name, stats))) => {
					box_line(&format!(
						"{tick} {name}  {stats}",
						tick = "✓".green(),
						name = name.bright_white(),
						stats = fmt_stats(&stats),
					));
					self.totals.add(&stats);
					sub_outputs.push((name.clone(), stats.output.clone()));
					self.outputs.insert(name.clone(), stats.output);
					self.last_step = Some(name);
				}
				Ok(Err(e)) => bail!("parallel step '{}' failed: {}", p.name, e),
				Err(e) => bail!("parallel step '{}' panicked: {}", p.name, e),
			}
		}
		box_close_ok(&p.name.bright_white(), "done");
		// Print each sub-step's response under a dim label so the user
		// can see what each branch produced. Final blank line separates
		// from the next top-level step.
		for (name, out) in &sub_outputs {
			let t = out.trim();
			if !t.is_empty() {
				eprintln!();
				eprintln!("{}", format!("── {name} ──").bright_black());
				print_response(t, self.markdown_enabled, &self.markdown_theme);
			}
		}
		eprintln!();
		Ok(())
	}

	async fn exec_loop(&mut self, l: &LoopStep, input: &str) -> Result<()> {
		let max = l.max_iterations;
		for i in 1..=max {
			for sub in &l.run {
				let suffix = format!(
					"  {tag}",
					tag = format!("[{i}/{max}] {}", l.name).bright_magenta(),
				);
				let stats = self.exec_sequential(sub, input, &suffix).await?;
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
					info_line(&format!(
						"loop '{name}' exit at iteration {i}",
						name = l.name
					));
					eprintln!();
					return Ok(());
				}
			}
		}
		info_line(&format!(
			"{warn} loop '{name}' reached max_iterations ({max}) without exit condition matching",
			warn = "⚠".yellow(),
			name = l.name,
			max = max,
		));
		eprintln!();
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
		info_line(&format!(
			"{name}: condition {res} → [{branch}]",
			name = c.name.bright_white(),
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
			let stats = self.exec_sequential(s, input, "").await?;
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

/// Open a step block with `╭ title`. Title is caller-colored so the
/// helper stays format-agnostic.
fn box_open(title: &str) {
	eprintln!("{} {}", "╭".bright_black(), title);
}

/// Close a step block with `╰ ✓ name  stats` on success.
fn box_close_ok(name_colored: &str, stats: &str) {
	eprintln!(
		"{} {} {}  {}",
		"╰".bright_black(),
		"✓".green(),
		name_colored,
		stats,
	);
}

/// Close a step block with `╰ ✗ name  msg` on failure.
fn box_close_err(name_colored: &str, msg: &str) {
	eprintln!(
		"{} {} {}  {}",
		"╰".bright_black(),
		"✗".red(),
		name_colored,
		msg.red(),
	);
}

/// Print one line inside an open step block — `│ text`. Used for
/// per-sub-step results inside a parallel block.
fn box_line(text: &str) {
	eprintln!("{} {}", "│".bright_black(), text);
}

/// Plain `· text` info line — used between step blocks for things that
/// don't belong inside any box (loop exits, conditional decisions).
fn info_line(text: &str) {
	eprintln!("{} {}", "·".bright_black(), text);
}

/// Emit a step's assistant response so the user can see what each step
/// actually produced. Goes to stderr — stdout is reserved for the
/// workflow's final result. When `markdown_enabled` and the content
/// looks like markdown, render through the same `MarkdownRenderer` the
/// interactive chat session uses (with the configured theme); falls
/// back to plain text on render failure. Trailing blank line provides
/// visual separation before the next step block.
fn print_response(output: &str, markdown_enabled: bool, markdown_theme: &str) {
	let t = output.trim();
	if t.is_empty() {
		eprintln!();
		return;
	}
	eprintln!();
	if markdown_enabled && is_markdown_content(t) {
		let theme = markdown_theme.parse().unwrap_or_default();
		let renderer = MarkdownRenderer::with_theme(theme);
		match renderer.render_and_print(t) {
			Ok(_) => {}
			Err(_) => eprintln!("{t}"),
		}
	} else {
		eprintln!("{t}");
	}
	eprintln!();
}

/// Compact one-line stats summary for a finished step: duration, cost,
/// total tokens, total tool calls + any failures.
fn fmt_stats(s: &StepStats) -> String {
	let bullet = "·".bright_black();
	let tools = fmt_tools(s.tool_count, s.tool_failed);
	format!(
		"{dur}  {b} ${cost:.4}  {b} {tok} tok  {b} {tools}",
		dur = fmt_dur(s.duration),
		cost = s.cost,
		tok = s.total_tokens,
		b = bullet,
	)
}

/// `⚒N` if no failures, `⚒N ✗F` (✗ in red) when one or more tools failed.
fn fmt_tools(count: u64, failed: u64) -> String {
	if failed > 0 {
		format!("⚒{count} {}", format!("✗{failed}").red())
	} else {
		format!("⚒{count}")
	}
}

/// Public entry — runs a fully-validated workflow.
///
/// Returns the text that should be written to stdout (the resolved
/// `result` step's output, or the last step if `result` is unset).
pub async fn execute(wf: &WorkflowDef, input: &str, config: &Config) -> Result<String> {
	let mut ex = Executor::new(wf.name.clone(), config);

	// In TTY mode, suppress the controlling terminal's keypress echo for
	// the lifetime of the workflow so stray Enter / Ctrl-C presses don't
	// ghost into the spinner row. stdin is typically piped here (we read
	// `input` from it), so the tty fd lives on stderr.
	#[cfg(unix)]
	let _echo_guard = if ex.interactive {
		crate::utils::term_echo::CtrlCEchoGuard::install_on(libc::STDERR_FILENO)
	} else {
		None
	};
	#[cfg(not(unix))]
	let _echo_guard: Option<crate::utils::term_echo::CtrlCEchoGuard> = None;

	eprintln!(
		"{label} {sep} {name}",
		label = "workflow".bright_black(),
		sep = "·".bright_black(),
		name = wf.name.bright_cyan(),
	);
	eprintln!();

	let mut last_top_level: Option<String> = None;

	for step in &wf.steps {
		match step {
			Step::Sequential(s) => {
				let stats = ex.exec_sequential(s, input, "").await?;
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

	let bullet = "·".bright_black();
	eprintln!(
		"{label} {sep} {dur}  {b} ${cost:.4}  {b} {tok} tok  {b} {tools}",
		label = "total".bright_black(),
		sep = "·".bright_black(),
		dur = fmt_dur(ex.totals.duration),
		cost = ex.totals.cost,
		tok = ex.totals.tokens,
		tools = fmt_tools(ex.totals.tools, ex.totals.tools_failed),
		b = bullet,
	);

	// Drop any keypresses the user typed during animation so they don't
	// leak into the shell's input queue when control returns.
	if ex.interactive {
		#[cfg(unix)]
		crate::utils::term_echo::drain_fd(libc::STDERR_FILENO);
	}

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
