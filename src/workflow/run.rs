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
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::proc::{run_step, send_done, RunOutcome, RunStepArgs, StepStats};
use super::schema::{
	Condition, ConditionalStep, Edge, LoopStep, ParallelStep, Sequential, SessionMode, Step,
	WorkflowDef, END_NODE,
};
use super::validate;
use crate::config::Config;
use crate::session::chat::markdown::{is_markdown_content, MarkdownRenderer};
use crate::session::{JsonlSink, OutputSink};
use crate::websocket::{AssistantPayload, CostPayload, ServerMessage};

/// Final summed totals printed once at the end.
#[derive(Debug, Default, Clone, Copy)]
struct Totals {
	duration: Duration,
	cost: f64,
	tokens: u64,
	input_tokens: u64,
	output_tokens: u64,
	cache_read_tokens: u64,
	cache_write_tokens: u64,
	reasoning_tokens: u64,
	tools: u64,
	tools_failed: u64,
}

impl Totals {
	fn add(&mut self, s: &StepStats) {
		self.duration += s.duration;
		self.cost += s.cost;
		self.tokens += s.total_tokens;
		self.input_tokens += s.input_tokens;
		self.output_tokens += s.output_tokens;
		self.cache_read_tokens += s.cache_read_tokens;
		self.cache_write_tokens += s.cache_write_tokens;
		self.reasoning_tokens += s.reasoning_tokens;
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
	/// `--format jsonl` — emit a per-step `assistant` event to stdout as each
	/// step completes, plus an aggregated `cost` event at the end.
	jsonl: bool,
	/// Optional hard spending cap (USD) for the whole workflow. None = no cap.
	max_cost: Option<f64>,
	/// Graph mode uses explicit routes rather than declaration order. Kept here
	/// so block executors can preserve legacy `last_step` semantics exactly.
	graph_mode: bool,
	/// Every declared top-level and sub-step output name. In graph mode this
	/// lets prompt resolution fail clearly when a route reaches a node before a
	/// referenced producer has run.
	known_outputs: HashSet<String>,
	/// Last cumulative stats snapshot per `session = "continue"` step (keyed by
	/// step name, same key as `session_ids`). Used to fold per-invocation deltas
	/// so a resumed session's cumulative cost/tokens aren't re-counted every
	/// loop iteration / retry. Fresh and parallel steps never populate this.
	cost_baseline: HashMap<String, StepStats>,
}

impl Executor {
	fn new(
		wf_name: String,
		config: &Config,
		jsonl: bool,
		max_cost: Option<f64>,
		graph_mode: bool,
		known_outputs: HashSet<String>,
	) -> Self {
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
			jsonl,
			max_cost,
			graph_mode,
			known_outputs,
			cost_baseline: HashMap::new(),
		}
	}

	/// Fold a step's reported stats into per-step deltas for accurate totals.
	///
	/// `octomind run --format jsonl` reports CUMULATIVE session figures (cost +
	/// all token counts). A `continue` session resumed across loop iterations or
	/// retries therefore re-reports the running total every invocation; summing
	/// those raw would over-count quadratically and trip `max_cost` far too
	/// early. For continue steps we subtract the per-step baseline (then advance
	/// it), yielding just this turn's spend. Fresh steps are a brand-new session
	/// each time and are returned unchanged. `tool_count`/`tool_failed`/
	/// `duration` are per-invocation and never folded.
	fn fold_stats(&mut self, step_name: &str, mode: SessionMode, stats: &StepStats) -> StepStats {
		if mode != SessionMode::Continue {
			return stats.clone();
		}
		let base = self.cost_baseline.entry(step_name.to_string()).or_default();
		continue_delta(base, stats)
	}

	/// Abort the workflow when the accumulated cost has crossed `max_cost`.
	/// Called after a step's cost is folded into `totals` so the cap stops
	/// spend before the NEXT step runs — the responsive guard a runaway loop
	/// needs (per-session caps reset every subprocess and can't bound the
	/// aggregate). `after_step` names the step that pushed it over.
	fn enforce_budget(&self, after_step: &str) -> Result<()> {
		if let Some(cap) = self.max_cost {
			if self.totals.cost > cap {
				bail!(
					"workflow cost budget exceeded: spent ${:.4} exceeds max_cost ${:.4} (stopped after step '{}')",
					self.totals.cost,
					cap,
					after_step,
				);
			}
		}
		Ok(())
	}

	/// Emit a completed step's result as a JSONL `assistant` event on stdout
	/// when running with `--format jsonl`. Mirrors the per-step response the
	/// human display writes to stderr, so machine consumers see each step's
	/// outcome — not just the final one. `step` carries the step name.
	fn emit_step(&self, name: &str, content: &str) {
		if self.jsonl {
			JsonlSink.emit(ServerMessage::Assistant(AssistantPayload {
				content: content.to_string(),
				session_id: String::new(),
				step: Some(name.to_string()),
			}));
		}
	}

	/// Resolve a step's prompt the same way chat sessions resolve user
	/// input. Three passes, in order:
	///
	/// 1. Workflow-specific `{{var}}` — `{{input}}` and prior step names
	///    from `self.outputs`. Unknown `{{var}}` are preserved literally
	///    so the next pass can claim its built-ins.
	/// 2. `process_placeholders_async_with_role` — the canonical chat helper that
	///    expands `{{DATE}} {{CWD}} {{SHELL}} {{OS}} {{BINARIES}}
	///    {{ROLE}} {{SYSTEM}} {{CONTEXT}} {{GIT_STATUS}} {{GIT_TREE}}
	///    {{README}}`.
	/// 3. `expand_context_blocks` — replaces any `<context>path</context>`
	///    or `<context>path:start:end</context>` blocks with the actual
	///    file contents rendered as XML, same as chat's compression /
	///    file-context path. Lets a step emit a context block in its
	///    response and have the next step receive the inlined file.
	///
	/// Substitution reads `self.outputs`; it does not mutate it. In a dynamic
	/// `match` block the executor binds the block's own name to each branch's
	/// matched item (the loop variable) for that branch's substitution; the
	/// accumulated output lands under the sub-step's name. From this function's
	/// perspective, all of that is just `self.outputs.get(name)`.
	async fn substitute(&self, prompt: &str, input: &str, role: &str) -> Result<String> {
		let re = validate::var_regex();
		if self.graph_mode {
			for captures in re.captures_iter(prompt) {
				let name = &captures[1];
				if name != "input"
					&& !validate::is_builtin_placeholder(name)
					&& self.known_outputs.contains(name)
					&& !self.outputs.contains_key(name)
				{
					bail!("workflow output '{{{{{name}}}}}' is unavailable on the current route");
				}
			}
		}
		// `{{name}}` resolves against `self.outputs`; `{{input}}` resolves to the
		// workflow stdin. That is the entire substitution contract. A dynamic
		// `match` block manages `self.outputs` itself: the block name holds the
		// per-branch item during fan-out; each branch's output accumulates under
		// the sub-step's name. This function does not know about any of that.
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
		// Pass the step's role so `{{ROLE}}` resolves to the actual role rather
		// than "unknown" — the step subprocess runs under this role.
		let after_placeholders =
			crate::session::helper_functions::process_placeholders_async_with_role(
				&after_wf,
				&project_dir,
				Some(role),
			)
			.await;
		Ok(crate::utils::file_renderer::expand_context_blocks(
			&after_placeholders,
		))
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
		let templated_prompt = self.substitute(&s.prompt, input, &s.role).await?;
		let workdir = resolve_workdir(&s.name, s.workdir.as_deref())?;
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
						let _ = send_done(&id, workdir.as_deref()).await;
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
				workdir: workdir.clone(),
				skills: s.skills.clone(),
				capabilities: s.capabilities.clone(),
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
					// Fold cumulative continue-session figures into this turn's
					// delta before display/emit/totals so the per-step line,
					// the running total, and max_cost all count each turn once.
					let stats = self.fold_stats(&s.name, s.session, &stats);
					box_close_ok(&s.name.bright_white(), &fmt_stats(&stats));
					print_response(&stats.output, self.markdown_enabled, &self.markdown_theme);
					self.emit_step(&s.name, &stats.output);
					self.totals.add(&stats);
					self.enforce_budget(&s.name)?;
					return Ok(stats);
				}
				RunOutcome::Empty(stats) => {
					let stats = self.fold_stats(&s.name, s.session, &stats);
					self.totals.add(&stats);
					last_err = Some(format!(
						"produced no assistant output (attempt {attempt}/{max_attempts})"
					));
				}
				RunOutcome::NonZero {
					stats,
					code,
					stderr_tail,
				} => {
					let stats = self.fold_stats(&s.name, s.session, &stats);
					self.totals.add(&stats);
					last_err = Some(with_stderr(
						format!("failed exit code {code:?} (attempt {attempt}/{max_attempts})"),
						&stderr_tail,
					));
				}
				RunOutcome::Timeout(elapsed) => {
					last_err = Some(format!(
						"timed out after {}s (attempt {attempt}/{max_attempts})",
						elapsed.as_secs()
					));
				}
				RunOutcome::SpawnError {
					source: e,
					stderr_tail,
				} => {
					last_err = Some(with_stderr(format!("spawn error: {e}"), &stderr_tail));
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
		// Build the branch list, substituting prompts up-front against the SAME
		// outer scope (sub-steps cannot reference each other). Substitution may
		// touch disk and workdirs resolve here too, so a bad one fails the whole
		// block before any subprocess spawns. Two sourcing modes:
		//   - dynamic (`match` set): `match` splits the named source output
		//     into items and loops the single listed sub-step over them. The
		//     block's own name is the loop variable: for each branch the executor
		//     binds `self.outputs[block_name]` to that branch's matched item, so
		//     the template's `{{<block_name>}}` resolves to the one task. Each
		//     branch's output accumulates under the sub-step's name — that is what
		//     downstream steps read.
		//   - static: each listed sub-step, expanded by its `count`.
		let is_dynamic = p.match_pattern.is_some();
		let mut replicas: Vec<PreparedReplica> = Vec::new();
		if let Some(pattern) = &p.match_pattern {
			// Pre-flight guarantees exactly one sub-step and a non-first step.
			let template = &p.run[0];
			let source_name = p
				.source
				.as_ref()
				.expect("validation requires dynamic fan-out source");
			let source = self.outputs.get(source_name).ok_or_else(|| {
				anyhow::anyhow!(
					"dynamic parallel '{}' source output '{}' is unavailable on the current route",
					p.name,
					source_name
				)
			})?;
			let re = Regex::new(pattern).map_err(|e| {
				anyhow::anyhow!("dynamic parallel '{}': invalid match regex: {e}", p.name)
			})?;
			let items = extract_items(&re, source);
			if items.is_empty() {
				bail!(
					"dynamic parallel '{}': match pattern found 0 items in '{}' output",
					p.name,
					source_name,
				);
			}
			let workdir = resolve_workdir(&template.name, template.workdir.as_deref())?;
			for (i, item) in items.iter().enumerate() {
				// The block's own name is the loop variable: bind it to this
				// branch's matched item so the template's `{{<block_name>}}`
				// resolves to the one task. The branch's output is collected and
				// accumulated under the sub-step's name once all branches finish.
				self.outputs.insert(p.name.clone(), item.clone());
				let prompt = self
					.substitute(&template.prompt, input, &template.role)
					.await?;
				// One replica per match. `count` on the dynamic template is
				// ignored — the number of replicas is exactly the number of
				// matches, which is the user's knob for fan-out. Labels stay
				// stable: "researcher #1", "researcher #2", ... regardless of
				// `count`.
				replicas.push(PreparedReplica {
					base: template.name.clone(),
					label: format!("{} #{}", template.name, i + 1),
					seq: template.clone(),
					prompt,
					workdir: workdir.clone(),
				});
			}
		} else {
			for s in &p.run {
				let prompt = self.substitute(&s.prompt, input, &s.role).await?;
				let workdir = resolve_workdir(&s.name, s.workdir.as_deref())?;
				for rep in expand_substep(s) {
					replicas.push(PreparedReplica {
						base: s.name.clone(),
						label: rep.label,
						seq: rep.seq,
						prompt: prompt.clone(),
						workdir: workdir.clone(),
					});
				}
			}
		}

		let total = replicas.len();
		// `max_parallel` throttles concurrency via a semaphore; None = unbounded.
		let sem = p.max_parallel.map(|n| Arc::new(Semaphore::new(n.max(1))));

		// We can't borrow &mut self across the join, so each task owns its own
		// data and we DON'T touch self. Parallel sub-steps always get a fresh
		// session — `session = "continue"` only makes sense across loop iters.
		let mut handles = Vec::new();
		for r in replicas {
			let sem = sem.clone();
			handles.push(tokio::spawn(async move {
				let _permit = match &sem {
					Some(s) => Some(s.clone().acquire_owned().await.expect("semaphore open")),
					None => None,
				};
				let max_attempts = r.seq.retries + 1;
				let mut last_err: Option<String> = None;
				for attempt in 1..=max_attempts {
					let args = RunStepArgs {
						role: r.seq.role.clone(),
						prompt: r.prompt.clone(),
						session_name: None,
						model: r.seq.model.clone(),
						workdir: r.workdir.clone(),
						skills: r.seq.skills.clone(),
						capabilities: r.seq.capabilities.clone(),
						timeout_secs: r.seq.timeout,
						event_prefix: None,
						spinner: None,
						wf_start: Instant::now(),
						prior_cost: 0.0,
						prior_tools: 0,
					};
					match run_step(args).await {
						RunOutcome::Ok(stats) => {
							return ParallelResult {
								base: r.base,
								label: r.label,
								outcome: Ok(stats),
							}
						}
						RunOutcome::Empty(_) => {
							last_err =
								Some(format!("empty output (attempt {attempt}/{max_attempts})"));
						}
						RunOutcome::NonZero {
							code, stderr_tail, ..
						} => {
							last_err = Some(with_stderr(
								format!(
									"non-zero exit {code:?} (attempt {attempt}/{max_attempts})"
								),
								&stderr_tail,
							));
						}
						RunOutcome::Timeout(e) => {
							last_err = Some(format!(
								"timed out after {}s (attempt {attempt}/{max_attempts})",
								e.as_secs()
							));
						}
						RunOutcome::SpawnError {
							source: e,
							stderr_tail,
						} => {
							last_err = Some(with_stderr(format!("spawn error: {e}"), &stderr_tail));
						}
					}
				}
				ParallelResult {
					base: r.base,
					label: r.label,
					outcome: Err(last_err.unwrap_or_default()),
				}
			}));
		}

		let tag = if is_dynamic {
			format!("({total} items in parallel)")
		} else if total == p.run.len() {
			format!("({total} in parallel)")
		} else {
			format!("({} sub-steps → {total} runs in parallel)", p.run.len())
		};
		box_open(&format!(
			"{name}  {tag}",
			name = p.name.bright_white(),
			tag = tag.bright_black(),
		));

		// Group results by sub-step base name in declaration order so a base's
		// replicas aggregate together and the block aggregates bases in order.
		let mut by_base: Vec<(String, Vec<(String, String)>)> =
			p.run.iter().map(|s| (s.name.clone(), Vec::new())).collect();
		let idx_of: HashMap<&str, usize> = p
			.run
			.iter()
			.enumerate()
			.map(|(i, s)| (s.name.as_str(), i))
			.collect();

		let mut succeeded = 0usize;
		for res in futures::future::join_all(handles).await {
			let res = match res {
				Ok(r) => r,
				Err(e) => bail!("parallel step '{}' panicked: {}", p.name, e),
			};
			match res.outcome {
				Ok(stats) => {
					box_line(&format!(
						"{tick} {label}  {stats}",
						tick = "✓".green(),
						label = res.label.bright_white(),
						stats = fmt_stats(&stats),
					));
					self.totals.add(&stats);
					succeeded += 1;
					by_base[idx_of[res.base.as_str()]]
						.1
						.push((res.label, stats.output));
				}
				Err(msg) => {
					box_line(&format!(
						"{cross} {label}  {msg}",
						cross = "✗".red(),
						label = res.label.bright_white(),
						msg = msg.red(),
					));
				}
			}
		}

		// `min_success` (None = all) sets how many replicas must succeed.
		let threshold = p.min_success.map_or(total, |m| m as usize);
		if succeeded < threshold {
			box_close_err(
				&p.name.bright_white(),
				&format!("{succeeded}/{total} succeeded (need {threshold})"),
			);
			eprintln!();
			self.enforce_budget(&p.name)?;
			bail!(
				"parallel step '{}': only {}/{} replicas succeeded (min_success {})",
				p.name,
				succeeded,
				total,
				threshold,
			);
		}
		box_close_ok(
			&p.name.bright_white(),
			&format!("{succeeded}/{total} succeeded"),
		);

		// Wire outputs. Each sub-step's OUTPUT is stored under its OWN name:
		// replicas (`count`, or dynamic `match` items) accumulate with
		// `── label ──` headers; a single un-expanded sub-step maps straight to
		// its raw output.
		let mut block_parts: Vec<(String, String)> = Vec::new();
		for (i, (base, reps)) in by_base.iter().enumerate() {
			let expanded = is_dynamic || p.run[i].replica_count() > 1;
			let agg = if reps.is_empty() {
				String::new()
			} else if expanded {
				join_labeled(reps)
			} else {
				reps[0].1.clone()
			};
			self.outputs.insert(base.clone(), agg.clone());
			block_parts.push((base.clone(), agg));
		}
		// Every top-level node exposes a canonical output under its own name.
		// During dynamic fan-out `p.name` is temporarily the per-item variable;
		// once all branches join it becomes their aggregate like any other block.
		let block_agg = if block_parts.len() == 1 {
			block_parts[0].1.clone()
		} else {
			join_labeled(&block_parts)
		};
		self.outputs.insert(p.name.clone(), block_agg);
		if self.graph_mode || !is_dynamic {
			self.last_step = Some(p.name.clone());
		} else {
			// Preserve ordered-workflow behavior: a dynamic block's last output was
			// historically its one expanded sub-step.
			self.last_step = Some(by_base[0].0.clone());
		}

		// Print each base's aggregated response under a dim label + emit jsonl.
		for (base, _) in &by_base {
			let out = self.outputs.get(base).cloned().unwrap_or_default();
			let t = out.trim();
			if !t.is_empty() {
				eprintln!();
				eprintln!("{}", format!("── {base} ──").bright_black());
				print_response(t, self.markdown_enabled, &self.markdown_theme);
			}
			self.emit_step(base, &out);
		}
		eprintln!();
		self.enforce_budget(&p.name)?;
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
			// Pre-flight guarantees the target is a known step; a miss here is an
			// executor bug — fail loudly instead of silently burning iterations.
			let value = self.outputs.get(&target).ok_or_else(|| {
				anyhow::anyhow!(
					"loop '{}': exit_when target '{}' has no output at iteration {}",
					l.name,
					target,
					i
				)
			})?;
			if condition_matches(exit_when, value) {
				info_line(&format!(
					"loop '{name}' exit at iteration {i}",
					name = l.name
				));
				eprintln!();
				let canonical = self
					.last_step
					.as_ref()
					.and_then(|name| self.outputs.get(name))
					.cloned()
					.unwrap_or_default();
				self.outputs.insert(l.name.clone(), canonical);
				if self.graph_mode {
					self.last_step = Some(l.name.clone());
				}
				return Ok(());
			}
		}
		info_line(&format!(
			"{warn} loop '{name}' reached max_iterations ({max}) without exit condition matching",
			warn = "⚠".yellow(),
			name = l.name,
			max = max,
		));
		eprintln!();
		let canonical = self
			.last_step
			.as_ref()
			.and_then(|name| self.outputs.get(name))
			.cloned()
			.unwrap_or_default();
		self.outputs.insert(l.name.clone(), canonical);
		if self.graph_mode {
			self.last_step = Some(l.name.clone());
		}
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
		// Skipped-branch steps resolve to empty entries (inserted below), so a
		// genuine miss means the target never ran — an executor bug, not a
		// condition that should silently evaluate against "".
		let value = self.outputs.get(&target).cloned().ok_or_else(|| {
			anyhow::anyhow!(
				"conditional step '{}': condition target '{}' has no output",
				c.name,
				target
			)
		})?;
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

		let mut selected_output = String::new();
		for s in chosen {
			let stats = self.exec_sequential(s, input, "").await?;
			selected_output = stats.output.clone();
			self.outputs.insert(s.name.clone(), stats.output);
			self.last_step = Some(s.name.clone());
		}
		// Skipped branch outputs resolve to empty string.
		for s in skipped {
			self.outputs.entry(s.name.clone()).or_default();
		}
		self.outputs.insert(c.name.clone(), selected_output);
		if self.graph_mode {
			self.last_step = Some(c.name.clone());
		}
		Ok(())
	}

	async fn exec_node(&mut self, step: &Step, input: &str) -> Result<()> {
		match step {
			Step::Sequential(s) => {
				let stats = self.exec_sequential(s, input, "").await?;
				self.outputs.insert(s.name.clone(), stats.output);
				self.last_step = Some(s.name.clone());
			}
			Step::Parallel(p) => self.exec_parallel(p, input).await?,
			Step::Loop(l) => self.exec_loop(l, input).await?,
			Step::Conditional(c) => self.exec_conditional(c, input).await?,
		}
		Ok(())
	}

	fn next_graph_node(&self, wf: &WorkflowDef, current: &str) -> Result<String> {
		select_graph_edge(&wf.edges, &self.outputs, current)
	}
}

/// Resolve a step's optional `workdir` to an absolute path. Relative
/// paths resolve against the orchestrator's cwd. Checked at execution
/// time rather than pre-flight so a directory created by an earlier
/// step is legal. A missing directory is a hard error — the subprocess
/// would otherwise die with an opaque spawn failure.
fn resolve_workdir(step_name: &str, workdir: Option<&str>) -> Result<Option<PathBuf>> {
	let Some(w) = workdir else {
		return Ok(None);
	};
	let p = Path::new(w);
	let abs = if p.is_absolute() {
		p.to_path_buf()
	} else {
		std::env::current_dir()?.join(p)
	};
	if !abs.is_dir() {
		bail!(
			"step '{}': workdir '{}' is not a directory",
			step_name,
			abs.display()
		);
	}
	Ok(Some(abs))
}

/// One concrete run produced by expanding a parallel sub-step.
struct Replica {
	/// Human-facing label shown in progress lines / response headers.
	label: String,
	/// The sub-step to run, with its model forced for `models`-mode replicas.
	seq: Sequential,
}

/// A [`Replica`] with its prompt and workdir resolved, ready to spawn.
struct PreparedReplica {
	/// The declaring sub-step's name — replicas of the same base aggregate
	/// under it.
	base: String,
	label: String,
	seq: Sequential,
	prompt: String,
	workdir: Option<PathBuf>,
}

/// Result handed back from a spawned replica task.
struct ParallelResult {
	base: String,
	label: String,
	outcome: std::result::Result<StepStats, String>,
}

/// Expand a parallel sub-step into its replicas:
/// - `count = N` → N identical replicas (same role/model/prompt) for best-of-N
/// - none        → the single step as-is
fn expand_substep(s: &Sequential) -> Vec<Replica> {
	if let Some(c) = s.count {
		(1..=c)
			.map(|i| Replica {
				label: format!("{} #{i}", s.name),
				seq: s.clone(),
			})
			.collect()
	} else {
		vec![Replica {
			label: s.name.clone(),
			seq: s.clone(),
		}]
	}
}

/// Extract dynamic-fan-out items from `source` with `re`: one per match, the
/// first capture group (the regex must define one — `{{...}}`-style content).
/// Trimmed; empty items dropped.
fn extract_items(re: &Regex, source: &str) -> Vec<String> {
	re.captures_iter(source)
		.filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
		.filter(|s| !s.is_empty())
		.collect()
}

/// Join labeled outputs with `── label ──` headers, blank-line separated.
/// Entries whose output trims to empty (e.g. a failed replica) are skipped.
fn join_labeled(parts: &[(String, String)]) -> String {
	parts
		.iter()
		.filter(|(_, out)| !out.trim().is_empty())
		.map(|(label, out)| format!("── {label} ──\n{}", out.trim()))
		.collect::<Vec<_>>()
		.join("\n\n")
}

/// Subtract `base` (the last cumulative snapshot for a continue-session step)
/// from `current` to recover this turn's spend, then advance `base` to
/// `current`. Cost/token figures from `octomind run` are cumulative session
/// totals, so without this a resumed session's running total is re-counted
/// every loop iteration / retry. `output`/`duration`/tool counts are
/// per-invocation and pass through unchanged.
fn continue_delta(base: &mut StepStats, current: &StepStats) -> StepStats {
	let folded = StepStats {
		output: current.output.clone(),
		duration: current.duration,
		cost: (current.cost - base.cost).max(0.0),
		input_tokens: current.input_tokens.saturating_sub(base.input_tokens),
		output_tokens: current.output_tokens.saturating_sub(base.output_tokens),
		total_tokens: current.total_tokens.saturating_sub(base.total_tokens),
		cache_read_tokens: current
			.cache_read_tokens
			.saturating_sub(base.cache_read_tokens),
		cache_write_tokens: current
			.cache_write_tokens
			.saturating_sub(base.cache_write_tokens),
		reasoning_tokens: current
			.reasoning_tokens
			.saturating_sub(base.reasoning_tokens),
		tool_count: current.tool_count,
		tool_failed: current.tool_failed,
	};
	*base = current.clone();
	folded
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

fn select_graph_edge(
	edges: &[Edge],
	outputs: &HashMap<String, String>,
	current: &str,
) -> Result<String> {
	for edge in edges.iter().filter(|edge| edge.from == current) {
		let selected = match &edge.when {
			None => true,
			Some(condition) => {
				let output_name = condition.output.as_deref().unwrap_or(current);
				let value = outputs.get(output_name).ok_or_else(|| {
					anyhow::anyhow!(
						"edge '{} -> {}' condition output '{}' is unavailable",
						edge.from,
						edge.to,
						output_name
					)
				})?;
				condition_matches(condition, value)
			}
		};
		if selected {
			return Ok(edge.to.clone());
		}
	}
	bail!("graph node '{}' has no matching route", current)
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

/// Append a captured stderr tail to a failure message, if any was
/// captured — the subprocess may die before emitting a structured
/// `ServerMessage::Error` on its stdout stream, in which case this is
/// the only diagnostic available (see `proc::RunOutcome::NonZero` /
/// `SpawnError`).
fn with_stderr(msg: String, stderr_tail: &str) -> String {
	if stderr_tail.trim().is_empty() {
		msg
	} else {
		format!("{msg}\n  stderr: {stderr_tail}")
	}
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

fn workflow_output_names(wf: &WorkflowDef) -> HashSet<String> {
	let mut names = HashSet::new();
	for step in &wf.steps {
		names.insert(step.name().to_string());
		let sub_steps: &[Sequential] = match step {
			Step::Sequential(_) => &[],
			Step::Parallel(p) => &p.run,
			Step::Loop(l) => &l.run,
			Step::Conditional(c) => &c.run,
		};
		for sub in sub_steps {
			names.insert(sub.name.clone());
		}
	}
	names
}

/// Public entry — runs a fully-validated workflow.
///
/// Each step's last assistant message is already printed (with markdown
/// rendering when enabled) as it completes, so the workflow produces no
/// stdout — callers consume per-step output from stderr instead.
pub async fn execute(
	wf: &WorkflowDef,
	input: &str,
	config: &Config,
	format: Option<&str>,
) -> Result<()> {
	let jsonl = matches!(format, Some("jsonl"));
	let mut ex = Executor::new(
		wf.name.clone(),
		config,
		jsonl,
		wf.max_cost,
		wf.is_graph(),
		workflow_output_names(wf),
	);

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

	if wf.is_graph() {
		let mut current = wf
			.entry
			.clone()
			.expect("validated graph workflows set entry");
		let max = wf.graph_max_transitions();
		let mut transitions = 0u32;
		loop {
			if transitions >= max {
				bail!("graph exceeded max_transitions ({max}) before reaching {END_NODE}");
			}
			let step = wf
				.steps
				.iter()
				.find(|step| step.name() == current)
				.expect("validator guarantees every graph node exists");
			ex.exec_node(step, input).await?;
			transitions += 1;

			let next = ex.next_graph_node(wf, &current)?;
			info_line(&format!("route: {current} -> {next}"));
			if next == END_NODE {
				break;
			}
			current = next;
		}
	} else {
		for step in &wf.steps {
			ex.exec_node(step, input).await?;
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

	// In jsonl mode, each step already emitted its own `assistant` event as it
	// completed (so the final result is the last such event). Close the stream
	// with one aggregated `cost` event. There is no single resumable session
	// for a workflow, so `session_id` is left empty.
	if jsonl {
		JsonlSink.emit(ServerMessage::Cost(CostPayload {
			session_tokens: ex.totals.tokens,
			session_cost: ex.totals.cost,
			input_tokens: ex.totals.input_tokens,
			output_tokens: ex.totals.output_tokens,
			cache_read_tokens: ex.totals.cache_read_tokens,
			cache_write_tokens: ex.totals.cache_write_tokens,
			reasoning_tokens: ex.totals.reasoning_tokens,
			session_id: String::new(),
		}));
	}

	// Drop any keypresses the user typed during animation so they don't
	// leak into the shell's input queue when control returns.
	if ex.interactive {
		#[cfg(unix)]
		crate::utils::term_echo::drain_fd(libc::STDERR_FILENO);
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn cumulative(cost: f64, tokens: u64) -> StepStats {
		StepStats {
			cost,
			total_tokens: tokens,
			input_tokens: tokens,
			..Default::default()
		}
	}

	#[test]
	fn continue_delta_counts_each_turn_once() {
		// A continue-session step reports CUMULATIVE session totals every
		// iteration: 0.10 → 0.25 → 0.45 (turn costs 0.10 / 0.15 / 0.20).
		let mut base = StepStats::default();
		let d1 = continue_delta(&mut base, &cumulative(0.10, 100));
		let d2 = continue_delta(&mut base, &cumulative(0.25, 250));
		let d3 = continue_delta(&mut base, &cumulative(0.45, 450));
		assert!((d1.cost - 0.10).abs() < 1e-9);
		assert!((d2.cost - 0.15).abs() < 1e-9);
		assert!((d3.cost - 0.20).abs() < 1e-9);
		// Summed deltas equal the final cumulative — counted once, not the
		// ~3x overcount that summing raw cumulative figures would produce.
		let summed = d1.cost + d2.cost + d3.cost;
		assert!((summed - 0.45).abs() < 1e-9, "summed={summed}");
		assert_eq!(d1.total_tokens + d2.total_tokens + d3.total_tokens, 450);
	}

	fn seq(name: &str) -> Sequential {
		Sequential {
			name: name.to_string(),
			role: "developer:general".to_string(),
			prompt: "{{input}}".to_string(),
			session: SessionMode::Fresh,
			timeout: 0,
			retries: 0,
			model: None,
			workdir: None,
			count: None,
			skills: None,
			capabilities: None,
		}
	}

	#[test]
	fn expand_count_replicates_with_own_model() {
		let mut s = seq("candidate");
		s.count = Some(3);
		s.model = Some("openai:gpt-5".into());
		let reps = expand_substep(&s);
		assert_eq!(reps.len(), 3);
		assert!(reps
			.iter()
			.all(|r| r.seq.model.as_deref() == Some("openai:gpt-5")));
		assert_eq!(reps[2].label, "candidate #3");
	}

	#[test]
	fn expand_none_is_single_passthrough() {
		let reps = expand_substep(&seq("solo"));
		assert_eq!(reps.len(), 1);
		assert_eq!(reps[0].label, "solo");
		assert!(reps[0].seq.model.is_none());
	}

	#[test]
	fn extract_items_xml_capture_group() {
		let re = Regex::new(r"(?s)<task>(.*?)</task>").unwrap();
		let src = "Here are tasks:\n<task>research A\nspanning lines</task>\nnoise\n<task>research B</task>";
		let items = extract_items(&re, src);
		assert_eq!(items, vec!["research A\nspanning lines", "research B"]);
	}

	#[test]
	fn extract_items_requires_capture_group() {
		// No capture group → the regex matches but produces no items, because
		// the caller has to express what part of the match is the item.
		let re = Regex::new(r"\d+").unwrap();
		assert!(extract_items(&re, "a1 b22 c333").is_empty());

		// A capture group on a similar pattern yields the groups.
		let re2 = Regex::new(r"(\d+)").unwrap();
		assert_eq!(extract_items(&re2, "a1 b22 c333"), vec!["1", "22", "333"]);
	}

	#[test]
	fn extract_items_skips_empty() {
		let re = Regex::new(r"(?s)<t>(.*?)</t>").unwrap();
		let items = extract_items(&re, "<t>keep</t><t>   </t><t>also</t>");
		assert_eq!(items, vec!["keep", "also"]);
	}

	#[test]
	fn join_labeled_skips_empty_and_headers_rest() {
		let parts = vec![
			("a".to_string(), "one".to_string()),
			("b".to_string(), "   ".to_string()),
			("c".to_string(), "two".to_string()),
		];
		let joined = join_labeled(&parts);
		assert_eq!(joined, "── a ──\none\n\n── c ──\ntwo");
	}

	#[test]
	fn continue_delta_clamps_nonmonotonic_drop() {
		// Cumulative figures should never drop, but guard against it anyway.
		let mut base = StepStats::default();
		let _ = continue_delta(&mut base, &cumulative(0.50, 500));
		let d = continue_delta(&mut base, &cumulative(0.40, 400));
		assert_eq!(d.cost, 0.0);
		assert_eq!(d.total_tokens, 0);
	}

	#[test]
	fn graph_edge_selects_condition_then_default() {
		let edges = vec![
			Edge {
				from: "review".into(),
				to: END_NODE.into(),
				when: Some(Condition {
					output: None,
					contains: Some("PASS".into()),
					matches: None,
				}),
			},
			Edge {
				from: "review".into(),
				to: "fix".into(),
				when: None,
			},
		];
		let mut outputs = HashMap::from([("review".to_string(), "needs work".to_string())]);
		assert_eq!(
			select_graph_edge(&edges, &outputs, "review").unwrap(),
			"fix"
		);

		outputs.insert("review".into(), "PASS".into());
		assert_eq!(
			select_graph_edge(&edges, &outputs, "review").unwrap(),
			END_NODE
		);
	}

	#[test]
	fn graph_edge_rejects_unavailable_condition_output() {
		let edges = vec![Edge {
			from: "review".into(),
			to: END_NODE.into(),
			when: Some(Condition {
				output: Some("verdict".into()),
				contains: Some("PASS".into()),
				matches: None,
			}),
		}];
		let err = select_graph_edge(&edges, &HashMap::new(), "review")
			.expect_err("missing route output must fail");
		assert!(err.to_string().contains("unavailable"), "got: {err}");
	}
}
