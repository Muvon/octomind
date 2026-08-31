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

#[test]
fn new_sets_role_and_plain_mode_with_blank_optionals() {
	let args = GenericSessionArgs::new("developer".to_string());
	assert_eq!(args.role, "developer");
	assert_eq!(args.mode, "plain");
	assert_eq!(args.name, None);
	assert_eq!(args.resume, None);
	assert!(!args.resume_recent);
	assert_eq!(args.model, None);
	assert_eq!(args.max_tokens, None);
	assert_eq!(args.temperature, None);
	assert!(!args.role_explicit);
	assert_eq!(args.max_retries, None);
	assert!(!args.daemon);
	assert!(args.hooks.is_empty());
	assert_eq!(args.schema, None);
}

#[test]
fn resume_sets_session_id_and_keeps_plain_mode() {
	let args = GenericSessionArgs::resume("abc-123".to_string(), "reviewer".to_string());
	assert_eq!(args.resume.as_deref(), Some("abc-123"));
	assert_eq!(args.role, "reviewer");
	assert_eq!(args.mode, "plain");
	assert_eq!(args.name, None);
	assert!(!args.resume_recent);
}

#[test]
fn default_is_fully_blank() {
	let args = GenericSessionArgs::default();
	assert_eq!(args.role, "");
	assert_eq!(args.mode, "");
	assert_eq!(args.name, None);
	assert_eq!(args.resume, None);
	assert!(!args.resume_recent);
	assert_eq!(args.model, None);
	assert!(!args.daemon);
	assert!(args.hooks.is_empty());
	assert_eq!(args.schema, None);
}

#[test]
fn clone_copies_every_field() {
	let args = GenericSessionArgs {
		name: Some("session".to_string()),
		resume: Some("abc".to_string()),
		resume_recent: true,
		model: Some("anthropic/claude-3-5-sonnet".to_string()),
		max_tokens: Some(4096),
		temperature: Some(0.2),
		role: "developer".to_string(),
		role_explicit: true,
		max_retries: Some(3),
		mode: "jsonl".to_string(),
		daemon: true,
		hooks: vec!["hook_a".to_string()],
		schema: Some(serde_json::json!({"type": "object"})),
	};
	let cloned = args.clone();
	assert_eq!(cloned.name, args.name);
	assert_eq!(cloned.resume, args.resume);
	assert_eq!(cloned.resume_recent, args.resume_recent);
	assert_eq!(cloned.model, args.model);
	assert_eq!(cloned.max_tokens, args.max_tokens);
	assert_eq!(cloned.temperature, args.temperature);
	assert_eq!(cloned.role, args.role);
	assert_eq!(cloned.role_explicit, args.role_explicit);
	assert_eq!(cloned.max_retries, args.max_retries);
	assert_eq!(cloned.mode, args.mode);
	assert_eq!(cloned.daemon, args.daemon);
	assert_eq!(cloned.hooks, args.hooks);
	assert_eq!(cloned.schema, args.schema);
}

#[test]
fn debug_representation_names_the_struct_and_role() {
	let args = GenericSessionArgs::new("developer".to_string());
	let rendered = format!("{args:?}");
	assert!(rendered.contains("GenericSessionArgs"), "{rendered}");
	assert!(rendered.contains("developer"), "{rendered}");
}
