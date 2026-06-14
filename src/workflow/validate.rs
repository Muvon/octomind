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
	for step in &wf.steps {
		collect_names(step, &mut all_names)?;
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
			Ok(())
		}
		Step::Parallel(ParallelStep { name, run }) => {
			if run.len() < 2 {
				bail!("parallel step '{}' must have at least 2 sub-steps", name);
			}
			for s in run {
				validate_fields(s)?;
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
