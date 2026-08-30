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

use super::*;
use crate::config::guardrails::Target;
use regex::Regex;

fn target(capability: &str, arg_name: Option<&str>, regex: Option<&str>) -> Target {
	Target {
		capability: capability.to_string(),
		arg_name: arg_name.map(|s| s.to_string()),
		regex: regex.map(|r| Regex::new(r).expect("valid regex")),
	}
}

fn hook(on: HookOn, trigger: Option<Target>, result: Option<&str>) -> CompiledHook {
	CompiledHook {
		trigger,
		result_regex: result.map(|r| Regex::new(r).expect("valid regex")),
		on,
		script: PathBuf::from("hook.sh"),
		evolution: None,
	}
}

#[test]
fn on_filter_gates_by_tool_outcome() {
	let params = json!({});
	let any = hook(HookOn::Any, None, None);
	assert!(hook_matches(&any, Some("shell"), &params, "out", true));
	assert!(hook_matches(&any, Some("shell"), &params, "out", false));

	let success = hook(HookOn::Success, None, None);
	assert!(hook_matches(&success, Some("shell"), &params, "out", true));
	assert!(!hook_matches(
		&success,
		Some("shell"),
		&params,
		"out",
		false
	));

	let error = hook(HookOn::Error, None, None);
	assert!(!hook_matches(&error, Some("shell"), &params, "out", true));
	assert!(hook_matches(&error, Some("shell"), &params, "out", false));
}

#[test]
fn trigger_narrows_to_one_capability() {
	let h = hook(HookOn::Any, Some(target("shell", None, None)), None);
	let params = json!({"command": "ls"});
	assert!(hook_matches(&h, Some("shell"), &params, "out", true));
	assert!(!hook_matches(&h, Some("read"), &params, "out", true));
	// An uncapability-mapped tool never matches a targeted hook.
	assert!(!hook_matches(&h, None, &params, "out", true));
}

#[test]
fn trigger_can_match_on_an_argument() {
	let h = hook(
		HookOn::Any,
		Some(target("shell", Some("command"), Some("^git push"))),
		None,
	);
	assert!(hook_matches(
		&h,
		Some("shell"),
		&json!({"command": "git push origin"}),
		"",
		true
	));
	assert!(!hook_matches(
		&h,
		Some("shell"),
		&json!({"command": "git status"}),
		"",
		true
	));
}

#[test]
fn result_regex_filters_on_output_text() {
	let h = hook(HookOn::Any, None, Some("(?i)panic"));
	let params = json!({});
	assert!(hook_matches(
		&h,
		Some("shell"),
		&params,
		"thread 'main' PANICKED",
		true
	));
	assert!(!hook_matches(&h, Some("shell"), &params, "all good", true));
}

#[test]
fn all_configured_filters_must_agree() {
	let h = hook(
		HookOn::Error,
		Some(target("shell", None, None)),
		Some("denied"),
	);
	let params = json!({});
	// Every filter satisfied.
	assert!(hook_matches(
		&h,
		Some("shell"),
		&params,
		"permission denied",
		false
	));
	// Right tool + text, wrong outcome.
	assert!(!hook_matches(
		&h,
		Some("shell"),
		&params,
		"permission denied",
		true
	));
	// Right outcome + text, wrong tool.
	assert!(!hook_matches(
		&h,
		Some("read"),
		&params,
		"permission denied",
		false
	));
	// Right tool + outcome, wrong text.
	assert!(!hook_matches(&h, Some("shell"), &params, "ok", false));
}

fn validator(when_used: Vec<Target>, when_unused: Vec<Target>) -> CompiledValidator {
	CompiledValidator {
		name: "v".to_string(),
		match_regex: None,
		when_used,
		when_unused,
		roles: Vec::new(),
		script: PathBuf::from("v.sh"),
		evolution: None,
	}
}

#[test]
fn when_satisfied_is_vacuously_true_with_no_conditions() {
	assert!(when_satisfied(&validator(vec![], vec![]), &[]));
}

#[test]
fn when_used_requires_every_target_in_the_slice() {
	let v = validator(
		vec![target("edit", None, None), target("shell", None, None)],
		vec![],
	);
	let only_edit = [(Some("edit".to_string()), json!({}))];
	assert!(!when_satisfied(&v, &only_edit));

	let both = [
		(Some("edit".to_string()), json!({})),
		(Some("shell".to_string()), json!({})),
	];
	assert!(when_satisfied(&v, &both));
}

#[test]
fn when_unused_requires_the_target_to_be_absent() {
	let v = validator(vec![], vec![target("shell", None, None)]);
	assert!(when_satisfied(&v, &[(Some("edit".to_string()), json!({}))]));
	assert!(!when_satisfied(
		&v,
		&[(Some("shell".to_string()), json!({}))]
	));
}

#[test]
fn used_and_unused_are_combined() {
	// "edited a file but never ran the tests"
	let v = validator(
		vec![target("edit", None, None)],
		vec![target("shell", Some("command"), Some("cargo test"))],
	);
	let edited_only = [(Some("edit".to_string()), json!({"path": "a.rs"}))];
	assert!(when_satisfied(&v, &edited_only));

	let edited_and_tested = [
		(Some("edit".to_string()), json!({"path": "a.rs"})),
		(
			Some("shell".to_string()),
			json!({"command": "cargo test --lib"}),
		),
	];
	assert!(!when_satisfied(&v, &edited_and_tested));
}
