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

	// Collect names + uniqueness check (recurses into sub-steps).
	let mut all_names: HashSet<String> = HashSet::new();
	// Sequential step names (top-level + sub-steps) — these are the only
	// names that produce a stored output addressable from `result` or
	// `{{var}}` substitution. Composite step names (parallel/loop/conditional
	// containers) live only in `all_names` for uniqueness checks.
	let mut output_names: HashSet<String> = HashSet::new();
	for step in &wf.steps {
		collect_names(step, &mut all_names, &mut output_names)?;
	}

	// Structural checks per step.
	for step in &wf.steps {
		structural_check(step)?;
	}

	// Reference resolution — walk in execution order, tracking what names
	// are available at each prompt.
	let mut available: HashSet<String> = HashSet::new();
	available.insert("input".into());

	for step in &wf.steps {
		check_step_refs(step, &mut available)?;
	}

	// Result field must point at a step that actually produces output.
	// Composite container names (parallel/loop/conditional) don't store
	// output themselves — only their sub-steps do.
	if let Some(r) = &wf.result {
		if !all_names.contains(r) {
			bail!("workflow result '{}' does not match any step name", r);
		}
		if !output_names.contains(r) {
			bail!(
				"workflow result '{}' refers to a composite step (parallel/loop/conditional) which has no output of its own; point `result` at a sub-step name",
				r
			);
		}
	}

	Ok(())
}

fn collect_names(
	step: &Step,
	names: &mut HashSet<String>,
	outputs: &mut HashSet<String>,
) -> Result<()> {
	insert_unique(step.name(), names)?;
	let (subs, container_produces_output): (&[Sequential], bool) = match step {
		Step::Sequential(_) => (&[], true),
		Step::Parallel(p) => (&p.run, false),
		Step::Loop(l) => (&l.run, false),
		Step::Conditional(c) => (&c.run, false),
	};
	if container_produces_output {
		outputs.insert(step.name().to_string());
	}
	for s in subs {
		insert_unique(&s.name, names)?;
		outputs.insert(s.name.clone());
	}
	Ok(())
}

fn insert_unique(name: &str, names: &mut HashSet<String>) -> Result<()> {
	if name == "input" {
		bail!(
			"step name 'input' is reserved (it's the substitution variable for stdin)"
		);
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
		Step::Sequential(_) => Ok(()),
		Step::Parallel(ParallelStep { name, run }) => {
			if run.len() < 2 {
				bail!("parallel step '{}' must have at least 2 sub-steps", name);
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
					anyhow::anyhow!("loop step '{}' exit_when.matches invalid regex: {}", name, e)
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
			Ok(())
		}
	}
}

fn check_step_refs(step: &Step, available: &mut HashSet<String>) -> Result<()> {
	match step {
		Step::Sequential(s) => {
			check_refs(&s.name, &s.prompt, available)?;
			available.insert(s.name.clone());
		}
		Step::Parallel(p) => {
			// Sub-step prompts may reference outer scope but not each other.
			let outer = available.clone();
			for s in &p.run {
				check_refs(&s.name, &s.prompt, &outer)?;
			}
			for s in &p.run {
				available.insert(s.name.clone());
			}
			available.insert(p.name.clone());
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
			available.insert(l.name.clone());

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
			available.insert(c.name.clone());
		}
	}
	Ok(())
}

fn check_refs(step_name: &str, prompt: &str, available: &HashSet<String>) -> Result<()> {
	let re = var_regex();
	for cap in re.captures_iter(prompt) {
		let var = &cap[1];
		if !available.contains(var) {
			bail!(
				"step '{}' references unknown variable '{{{{{}}}}}",
				step_name,
				var
			);
		}
	}
	Ok(())
}

pub fn var_regex() -> Regex {
	// Allow word chars and dashes.
	Regex::new(r"\{\{([A-Za-z_][A-Za-z0-9_\-]*)\}\}").expect("static regex")
}
