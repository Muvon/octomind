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

use super::{ModelProfile, ModelProfileOverride};
use crate::config::ReasoningEffortConfig;

fn main_profile() -> ModelProfile {
	ModelProfile {
		model: "openai:main".into(),
		reasoning_effort: ReasoningEffortConfig::Medium,
		max_tokens: 1000,
		temperature: 0.3,
		top_p: 0.8,
		top_k: 20,
		max_retries: 2,
		retry_timeout: 10,
		request_timeout_seconds: 60,
	}
}

#[test]
fn partial_override_inherits_every_unspecified_main_field() {
	let resolved = ModelProfileOverride {
		model: Some("anthropic:worker".into()),
		reasoning_effort: Some(ReasoningEffortConfig::High),
		..Default::default()
	}
	.resolve(&main_profile());

	assert_eq!(resolved.model, "anthropic:worker");
	assert_eq!(resolved.reasoning_effort, ReasoningEffortConfig::High);
	assert_eq!(resolved.max_tokens, 1000);
	assert_eq!(resolved.temperature, 0.3);
	assert_eq!(resolved.top_p, 0.8);
	assert_eq!(resolved.top_k, 20);
	assert_eq!(resolved.max_retries, 2);
	assert_eq!(resolved.retry_timeout, 10);
	assert_eq!(resolved.request_timeout_seconds, 60);
}

#[test]
fn later_override_wins_field_by_field() {
	let role = ModelProfileOverride {
		model: Some("openai:role".into()),
		temperature: Some(0.5),
		..Default::default()
	};
	let runtime = ModelProfileOverride {
		model: Some("google:runtime".into()),
		max_tokens: Some(42),
		..Default::default()
	};
	let resolved = role.overlay(&runtime).resolve(&main_profile());

	assert_eq!(resolved.model, "google:runtime");
	assert_eq!(resolved.temperature, 0.5);
	assert_eq!(resolved.max_tokens, 42);
}
