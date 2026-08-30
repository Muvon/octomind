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

// --- protect/restore escaped braces ---

#[test]
fn test_protect_restore_roundtrip() {
	// protect then restore converts {{{{...}}}} → {{...}} (that's the intended transformation)
	let input = "Use {{{{INPUT:KEY}}}} or {{{{ENV:URL}}}} as examples";
	assert_eq!(
		restore_escaped_braces(&protect_escaped_braces(input)),
		"Use {{INPUT:KEY}} or {{ENV:URL}} as examples"
	);
}

#[test]
fn test_restore_basic() {
	// After protect+restore the escaped form becomes literal {{...}}
	let protected = protect_escaped_braces("{{{{INPUT:KEY}}}}");
	assert_eq!(restore_escaped_braces(&protected), "{{INPUT:KEY}}");
	let protected = protect_escaped_braces("{{{{ENV:KEY}}}}");
	assert_eq!(restore_escaped_braces(&protected), "{{ENV:KEY}}");
	let protected = protect_escaped_braces("{{{{CWD}}}}");
	assert_eq!(restore_escaped_braces(&protected), "{{CWD}}");
}

#[test]
fn test_protect_hides_from_substitution() {
	// protect must replace {{ so a naive str.replace("{{CWD}}") won't match
	let protected = protect_escaped_braces("{{{{CWD}}}}");
	assert!(
		!protected.contains("{{"),
		"sentinel must not contain {{: {protected}"
	);
}

#[test]
fn test_no_escaped_braces_unchanged() {
	// Strings without escape sequences pass through unchanged
	let plain = "no placeholders here";
	assert_eq!(protect_escaped_braces(plain), plain);
	assert_eq!(restore_escaped_braces(plain), plain);
}

#[test]
fn test_multiple_escaped_occurrences() {
	let input = "Use {{{{INPUT:TOKEN}}}} or {{{{ENV:URL}}}} as examples";
	let result = restore_escaped_braces(&protect_escaped_braces(input));
	assert_eq!(result, "Use {{INPUT:TOKEN}} or {{ENV:URL}} as examples");
}

// --- process_placeholders_async_with_role (escaped syntax survives substitution) ---

#[tokio::test]
async fn test_escaped_placeholder_survives_substitution() {
	// {{{{CWD}}}} must not be replaced by the real CWD — it should become {{CWD}}
	let prompt = "Example: {{{{CWD}}}}";
	let dir = std::path::Path::new("/tmp");
	let result =
		crate::session::helper_functions::process_placeholders_async_with_role(prompt, dir, None)
			.await;
	assert_eq!(result, "Example: {{CWD}}");
}

#[tokio::test]
async fn test_real_and_escaped_placeholder_together() {
	// {{CWD}} gets replaced, {{{{CWD}}}} becomes literal {{CWD}}
	let prompt = "Real: {{CWD}}, Escaped: {{{{CWD}}}}";
	let dir = std::path::Path::new("/tmp");
	let result =
		crate::session::helper_functions::process_placeholders_async_with_role(prompt, dir, None)
			.await;
	assert_eq!(result, "Real: /tmp, Escaped: {{CWD}}");
}

// --- resolve_inputs / resolve_env_vars must not extract keys from escaped braces ---

#[test]
fn test_extract_input_keys_ignores_escaped() {
	// After protect_escaped_braces, `{{{{INPUT:KEY}}}}` becomes a sentinel
	// that does NOT contain `{{INPUT:` — so no keys should be extracted.
	let raw = "Use {{{{INPUT:KEY}}}} as an example";
	let protected = protect_escaped_braces(raw);
	let keys = extract_input_keys(&protected);
	assert!(
		keys.is_empty(),
		"Escaped {{{{INPUT:KEY}}}} must not produce any keys, got: {:?}",
		keys
	);
}

#[test]
fn test_extract_env_keys_ignores_escaped() {
	let raw = "Use {{{{ENV:BASE_URL}}}} as an example";
	let protected = protect_escaped_braces(raw);
	let keys = extract_env_keys(&protected);
	assert!(
		keys.is_empty(),
		"Escaped {{{{ENV:BASE_URL}}}} must not produce any keys, got: {:?}",
		keys
	);
}

#[tokio::test]
async fn test_resolve_inputs_no_prompt_for_escaped() {
	// resolve_inputs on a string with ONLY escaped placeholders must return
	// the literal `{{INPUT:KEY}}` without prompting the user.
	let raw = "Example: {{{{INPUT:SECRET}}}}";
	let result = resolve_inputs(raw).await.unwrap();
	assert_eq!(result, "Example: {{INPUT:SECRET}}");
}

#[tokio::test]
async fn test_resolve_env_vars_no_prompt_for_escaped() {
	// resolve_env_vars on a string with ONLY escaped placeholders must return
	// the literal `{{ENV:URL}}` without prompting the user.
	let raw = "Example: {{{{ENV:URL}}}}";
	let result = resolve_env_vars(raw).await.unwrap();
	assert_eq!(result, "Example: {{ENV:URL}}");
}
