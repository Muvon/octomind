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

//! Pre-flight validation: name uniqueness and `{{var}}` reference resolution.

use super::schema::{ConditionalStep, LoopStep, ParallelStep, Sequential, Step, WorkflowDef};
use anyhow::{bail, Result};
use regex::Regex;
use std::collections::HashSet;

pub fn validate(wf: &WorkflowDef) -> Result<()> {
	if wf.steps.is_empty() {
		bail!("workflow has no steps");
	}

	if let Some(cap) = wf.max_cost {
		if !cap.is_finite() || cap <= 0.0 {
			bail!("max_cost must be a positive number (got {cap})");
		}
	}

	// Collect names + uniqueness check (recurses into sub-steps).
	let mut all_names: HashSet<String> = HashSet::new();
	for step in &wf.steps {
		collect_names(step, &mut all_names)?;
	}

	// Structural checks per step.
	for (i, step) in wf.steps.iter().enumerate() {
		structural_check(step)?;
		// A dynamic parallel maps over the PREVIOUS step's output, so it needs one.
		if i == 0 {
			if let Step::Parallel(p) = step {
				if p.match_pattern.is_some() {
					bail!(
						"dynamic parallel '{}': needs a preceding step to map over (cannot be the first step)",
						p.name
					);
				}
			}
		}
	}

	// Reference resolution — walk in execution order, tracking what names
	// are available at each prompt.
	let mut available: HashSet<String> = HashSet::new();
	available.insert("input".into());

	for step in &wf.steps {
		check_step_refs(step, &mut available)?;
	}

	Ok(())
}

fn collect_names(step: &Step, names: &mut HashSet<String>) -> Result<()> {
	insert_unique(step.name(), names)?;
	let subs: &[Sequential] = match step {
		Step::Sequential(_) => &[],
		Step::Parallel(p) => &p.run,
		Step::Loop(l) => &l.run,
		Step::Conditional(c) => &c.run,
	};
	for s in subs {
		insert_unique(&s.name, names)?;
	}
	Ok(())
}

fn insert_unique(name: &str, names: &mut HashSet<String>) -> Result<()> {
	if name == "input" {
		bail!("step name 'input' is reserved (it's the substitution variable for stdin)");
	}
	if name.trim().is_empty() {
		bail!("step name must be non-empty");
	}
	if !names.insert(name.to_string()) {
		bail!("duplicate step name: '{}'", name);
	}
	Ok(())
}

fn structural_check(step: &Step) -> Result<()> {
	match step {
		Step::Sequential(s) => {
			validate_fields(s)?;
			reject_expansion(s)?;
			Ok(())
		}
		Step::Parallel(ParallelStep {
			name,
			match_pattern,
			run,
			min_success,
			max_parallel,
		}) => {
			if let Some(pattern) = match_pattern {
				// Dynamic: exactly one template sub-step; branches come from
				// matching the previous step at run time (count unknown here).
				if run.len() != 1 {
					bail!(
						"dynamic parallel '{}' (match set) must have exactly 1 sub-step (the per-item template)",
						name
					);
				}
				Regex::new(pattern).map_err(|e| {
					anyhow::anyhow!("dynamic parallel '{}': invalid match regex: {}", name, e)
				})?;
				let template = &run[0];
				validate_fields(template)?;
				validate_expansion(template)?;
				if let Some(m) = min_success {
					if *m == 0 {
						bail!("dynamic parallel '{}': min_success must be >= 1", name);
					}
				}
			} else {
				if run.len() < 2 {
					bail!("parallel step '{}' must have at least 2 sub-steps", name);
				}
				for s in run {
					validate_fields(s)?;
					validate_expansion(s)?;
				}
				let total: u32 = run.iter().map(|s| s.replica_count()).sum();
				if let Some(m) = min_success {
					if *m == 0 || *m > total {
						bail!(
							"parallel step '{}': min_success {} must be between 1 and {} (total replicas)",
							name,
							m,
							total
						);
					}
				}
			}
			if let Some(mp) = max_parallel {
				if *mp == 0 {
					bail!("parallel step '{}': max_parallel must be >= 1", name);
				}
			}
			Ok(())
		}
		Step::Loop(LoopStep {
			name,
			run,
			exit_when,
			..
		}) => {
			if run.is_empty() {
				bail!("loop step '{}' must have at least 1 sub-step", name);
			}
			for s in run {
				validate_fields(s)?;
				reject_expansion(s)?;
			}
			let exit_when = match exit_when {
				Some(c) => c,
				None => bail!("loop step '{}' requires exit_when", name),
			};
			if exit_when.contains.is_none() && exit_when.matches.is_none() {
				bail!(
					"loop step '{}' exit_when must set 'contains' or 'matches'",
					name
				);
			}
			if let Some(pat) = &exit_when.matches {
				Regex::new(pat).map_err(|e| {
					anyhow::anyhow!(
						"loop step '{}' exit_when.matches invalid regex: {}",
						name,
						e
					)
				})?;
			}
			Ok(())
		}
		Step::Conditional(ConditionalStep {
			name,
			condition,
			on_match,
			on_no_match,
			run,
		}) => {
			if condition.contains.is_none() && condition.matches.is_none() {
				bail!(
					"conditional step '{}' condition must set 'contains' or 'matches'",
					name
				);
			}
			if let Some(pat) = &condition.matches {
				Regex::new(pat).map_err(|e| {
					anyhow::anyhow!(
						"conditional step '{}' condition.matches invalid regex: {}",
						name,
						e
					)
				})?;
			}
			if on_match.is_empty() && on_no_match.is_empty() {
				bail!(
					"conditional step '{}' requires on_match and/or on_no_match",
					name
				);
			}
			let sub_names: HashSet<&str> = run.iter().map(|s| s.name.as_str()).collect();
			for n in on_match.iter().chain(on_no_match.iter()) {
				if !sub_names.contains(n.as_str()) {
					bail!(
						"conditional step '{}': branch references unknown sub-step '{}'",
						name,
						n
					);
				}
			}
			for s in run {
				validate_fields(s)?;
				reject_expansion(s)?;
			}
			Ok(())
		}
	}
}

fn validate_fields(s: &Sequential) -> Result<()> {
	if let Some(m) = &s.model {
		if m.trim().is_empty() {
			bail!("step '{}': model must not be empty when specified", s.name);
		}
	}
	if let Some(w) = &s.workdir {
		if w.trim().is_empty() {
			bail!(
				"step '{}': workdir must not be empty when specified",
				s.name
			);
		}
	}
	Ok(())
}

/// `count` fans a sub-step into replicas — only meaningful inside a parallel
/// block. Reject it anywhere else so the config fails loudly rather than
/// silently ignoring the field.
fn reject_expansion(s: &Sequential) -> Result<()> {
	if s.count.is_some() {
		bail!(
			"step '{}': 'count' is only valid on parallel sub-steps",
			s.name
		);
	}
	Ok(())
}

/// Validate the `count` fan-out field on a parallel sub-step.
fn validate_expansion(s: &Sequential) -> Result<()> {
	if let Some(c) = s.count {
		if c < 2 {
			bail!(
				"step '{}': count must be >= 2 (omit it for a single run)",
				s.name
			);
		}
	}
	Ok(())
}

fn check_step_refs(step: &Step, available: &mut HashSet<String>) -> Result<()> {
	match step {
		Step::Sequential(s) => {
			check_refs(&s.name, &s.prompt, available)?;
			available.insert(s.name.clone());
		}
		Step::Parallel(p) => {
			if p.match_pattern.is_some() {
				// Dynamic `match`: splits the previous step's output into items and
				// loops the single template over them. The block's own name is the
				// loop variable — in scope only for the template, bound to each
				// item at run time. The accumulated OUTPUT lives under the
				// sub-step's name, which is the only name visible downstream.
				let mut scope = available.clone();
				scope.insert(p.name.clone());
				let tpl = &p.run[0];
				check_refs(&tpl.name, &tpl.prompt, &scope)?;
				available.insert(tpl.name.clone());
			} else {
				// Static: sub-step prompts may reference outer scope but not each
				// other. Both the sub-step names and the block's own name (which
				// aggregates them) become available downstream.
				let outer = available.clone();
				for s in &p.run {
					check_refs(&s.name, &s.prompt, &outer)?;
				}
				for s in &p.run {
					available.insert(s.name.clone());
				}
				available.insert(p.name.clone());
			}
		}
		Step::Loop(l) => {
			// Inside the loop, sub-steps run sequentially; each iteration
			// makes prior siblings AND the loop's own outputs visible.
			let mut inner = available.clone();
			// Every loop sub-step name is visible to every other within
			// the loop because iterations re-bind them; relax forward-ref.
			for s in &l.run {
				inner.insert(s.name.clone());
			}
			for s in &l.run {
				check_refs(&s.name, &s.prompt, &inner)?;
			}
			for s in &l.run {
				available.insert(s.name.clone());
			}
			// The loop's own name is deliberately NOT referenceable: the executor
			// never stores an aggregate under it (unlike parallel blocks), so a
			// `{{loop-name}}` reference or exit_when.output = "loop-name" would
			// silently resolve to nothing at runtime. Fail here instead.

			// exit_when.output must be a known step (or omitted → last).
			if let Some(cond) = &l.exit_when {
				if let Some(o) = &cond.output {
					if !available.contains(o) {
						bail!(
							"loop step '{}': exit_when.output references unknown step '{}'",
							l.name,
							o
						);
					}
				}
			}
		}
		Step::Conditional(c) => {
			if let Some(o) = &c.condition.output {
				if !available.contains(o) {
					bail!(
						"conditional step '{}': condition.output references unknown step '{}'",
						c.name,
						o
					);
				}
			}
			let outer = available.clone();
			// Branch sub-steps run sequentially within their branch.
			let mut branch_scope = outer.clone();
			for s in &c.run {
				check_refs(&s.name, &s.prompt, &branch_scope)?;
				branch_scope.insert(s.name.clone());
			}
			for s in &c.run {
				available.insert(s.name.clone());
			}
			// Like loops, the conditional's own name is NOT referenceable — the
			// executor only stores branch sub-step outputs, never c.name itself.
		}
	}
	Ok(())
}

/// Built-in placeholders expanded at run time by
/// `helper_functions::process_placeholders_async` (pass 2 of step prompt
/// substitution). They are not step outputs, so reference-checking must treat
/// them as always-available — otherwise a prompt using `{{CONTEXT}}` etc. fails
/// pre-flight and never reaches the expansion pass. Keep in sync with that
/// function's `needs_*` checks (the source of truth).
const BUILTIN_PLACEHOLDERS: &[&str] = &[
	"DATE",
	"SHELL",
	"OS",
	"BINARIES",
	"CWD",
	"ROLE",
	"SYSTEM",
	"CONTEXT",
	"GIT_STATUS",
	"GIT_TREE",
	"README",
];

fn check_refs(step_name: &str, prompt: &str, available: &HashSet<String>) -> Result<()> {
	let re = var_regex();
	for cap in re.captures_iter(prompt) {
		let var = &cap[1];
		if BUILTIN_PLACEHOLDERS.contains(&var) || available.contains(var) {
			continue;
		}
		bail!(
			"step '{}' references unknown variable '{{{{{}}}}}",
			step_name,
			var
		);
	}
	Ok(())
}

pub fn var_regex() -> Regex {
	// Allow word chars and dashes.
	Regex::new(r"\{\{([A-Za-z_][A-Za-z0-9_\-]*)\}\}").expect("static regex")
}

/// Validate that every step role is a *public tap role* — a `category:variant`
/// tag present in `public_roles` (built from `taps::list_agent_tags()`).
///
/// Applied to tap-fetched (public) workflows only: they may reference public
/// roles installed via taps, never local config roles, so the workflow stays
/// portable to anyone with the same taps. Local workflow files keep full
/// freedom (they can use local roles).
pub fn validate_public_roles(wf: &WorkflowDef, public_roles: &HashSet<String>) -> Result<()> {
	for step in &wf.steps {
		for s in step_sequentials(step) {
			if !public_roles.contains(&s.role) {
				bail!(
					"step '{}': role '{}' is not a public tap role. \
					 Public workflows may only use 'category:variant' roles available in taps \
					 (run `octomind tap` to see installed taps).",
					s.name,
					s.role
				);
			}
		}
	}
	Ok(())
}

/// All leaf `Sequential` steps reachable from a top-level step.
fn step_sequentials(step: &Step) -> Vec<&Sequential> {
	match step {
		Step::Sequential(s) => vec![s],
		Step::Parallel(p) => p.run.iter().collect(),
		Step::Loop(l) => l.run.iter().collect(),
		Step::Conditional(c) => c.run.iter().collect(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn parse(toml_src: &str) -> WorkflowDef {
		toml::from_str(toml_src).expect("valid TOML")
	}

	#[test]
	fn builtin_placeholders_pass_validation() {
		// Built-ins are expanded at run time, not step outputs — they must not
		// be rejected as unknown variables.
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "s1"
			role = "developer:general"
			prompt = "Today is {{DATE}} in {{CWD}}. Context:\n{{CONTEXT}}\n\nRequest: {{input}}"
			"#,
		);
		validate(&wf).expect("built-in placeholders should validate");
	}

	#[test]
	fn genuinely_unknown_variable_still_fails() {
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "s1"
			role = "developer:general"
			prompt = "Hello {{nope}}"
			"#,
		);
		let err = validate(&wf).expect_err("unknown variable must fail");
		assert!(err.to_string().contains("nope"), "got: {err}");
	}

	#[test]
	fn max_cost_must_be_positive() {
		let wf = parse(
			r#"
			name = "wf"
			max_cost = 0.0
			[[steps]]
			name = "s1"
			role = "developer:general"
			prompt = "{{input}}"
			"#,
		);
		assert!(validate(&wf).is_err(), "zero max_cost must fail");

		let ok = parse(
			r#"
			name = "wf"
			max_cost = 1.5
			[[steps]]
			name = "s1"
			role = "developer:general"
			prompt = "{{input}}"
			"#,
		);
		validate(&ok).expect("positive max_cost should pass");
	}

	#[test]
	fn count_sweep_in_parallel_validates() {
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "candidates"
			parallel = true
			min_success = 2
			  [[steps.run]]
			  name = "candidate"
			  role = "developer:general"
			  prompt = "{{input}}"
			  count = 3
			  [[steps.run]]
			  name = "other"
			  role = "developer:general"
			  prompt = "{{input}}"
			"#,
		);
		validate(&wf).expect("count sweep + min_success in range should pass");
	}

	#[test]
	fn expansion_fields_rejected_outside_parallel() {
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "s1"
			role = "developer:general"
			prompt = "{{input}}"
			count = 3
			"#,
		);
		let err = validate(&wf).expect_err("count on a sequential step must fail");
		assert!(
			err.to_string().contains("only valid on parallel"),
			"got: {err}"
		);
	}

	#[test]
	fn count_below_two_fails() {
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "p"
			parallel = true
			  [[steps.run]]
			  name = "a"
			  role = "developer:general"
			  prompt = "{{input}}"
			  count = 1
			  [[steps.run]]
			  name = "b"
			  role = "developer:general"
			  prompt = "{{input}}"
			"#,
		);
		assert!(validate(&wf).is_err(), "count = 1 must fail");
	}

	#[test]
	fn min_success_out_of_range_fails() {
		// One sub-step with count = 2 + one plain sub-step = 3 total replicas.
		// min_success = 4 exceeds that.
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "p"
			parallel = true
			min_success = 4
			  [[steps.run]]
			  name = "a"
			  role = "developer:general"
			  prompt = "{{input}}"
			  count = 2
			  [[steps.run]]
			  name = "b"
			  role = "developer:general"
			  prompt = "{{input}}"
			"#,
		);
		let err = validate(&wf).expect_err("min_success > total replicas must fail");
		assert!(err.to_string().contains("min_success"), "got: {err}");
	}

	#[test]
	fn dynamic_parallel_validates() {
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "plan"
			role = "researcher:general"
			prompt = "List tasks in <task>..</task>:\n{{input}}"
			[[steps]]
			name = "research"
			parallel = true
			match = "(?s)<task>(.*?)</task>"
			max_parallel = 4
			min_success = 1
			  [[steps.run]]
			  name = "researcher"
			  role = "researcher:general"
			  prompt = "Research:\n{{research}}"
			[[steps]]
			name = "summary"
			role = "developer:general"
			prompt = "Summarize:\n{{researcher}}"
			"#,
		);
		validate(&wf)
			.expect("dynamic parallel referencing its own name in the template should pass");
	}

	#[test]
	fn dynamic_parallel_requires_single_template() {
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "plan"
			role = "researcher:general"
			prompt = "{{input}}"
			[[steps]]
			name = "research"
			parallel = true
			match = "(.+)"
			  [[steps.run]]
			  name = "a"
			  role = "researcher:general"
			  prompt = "{{research}}"
			  [[steps.run]]
			  name = "b"
			  role = "researcher:general"
			  prompt = "{{research}}"
			"#,
		);
		let err = validate(&wf).expect_err("dynamic parallel with 2 sub-steps must fail");
		assert!(err.to_string().contains("exactly 1 sub-step"), "got: {err}");
	}

	#[test]
	fn dynamic_parallel_cannot_be_first_step() {
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "research"
			parallel = true
			match = "(.+)"
			  [[steps.run]]
			  name = "researcher"
			  role = "researcher:general"
			  prompt = "Research the item"
			"#,
		);
		let err = validate(&wf).expect_err("dynamic parallel as first step must fail");
		assert!(err.to_string().contains("preceding step"), "got: {err}");
	}

	#[test]
	fn dynamic_parallel_invalid_regex_fails() {
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "plan"
			role = "researcher:general"
			prompt = "{{input}}"
			[[steps]]
			name = "research"
			parallel = true
			match = "(unclosed"
			  [[steps.run]]
			  name = "researcher"
			  role = "researcher:general"
			  prompt = "Research:\n{{research}}"
			"#,
		);
		let err = validate(&wf).expect_err("invalid match regex must fail");
		assert!(
			err.to_string().contains("invalid match regex"),
			"got: {err}"
		);
	}

	#[test]
	fn parallel_block_name_reference_resolves() {
		// The parallel block's own name is referenceable downstream (it now
		// aggregates every sub-step's output at runtime).
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "candidates"
			parallel = true
			  [[steps.run]]
			  name = "a"
			  role = "developer:general"
			  prompt = "{{input}}"
			  [[steps.run]]
			  name = "b"
			  role = "developer:general"
			  prompt = "{{input}}"
			[[steps]]
			name = "judge"
			role = "developer:general"
			prompt = "Pick best:\n{{candidates}}"
			"#,
		);
		validate(&wf).expect("reference to parallel block name should validate");
	}

	#[test]
	fn step_output_reference_resolves() {
		let wf = parse(
			r#"
			name = "wf"
			[[steps]]
			name = "spec"
			role = "developer:general"
			prompt = "{{input}}"
			[[steps]]
			name = "build"
			role = "developer:general"
			prompt = "Build {{spec}} on {{DATE}}"
			"#,
		);
		validate(&wf).expect("forward-valid step reference + built-in should pass");
	}
}
