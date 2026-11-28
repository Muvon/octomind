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

//! The default config template plus a local `assistant` role, for tests that need one: the
//! template ships no role of that name, so `assistant` otherwise resolves to the tap agent.
//!
//! One file, included with `#[path]` by the library's, the binary's and the integration tests'
//! test code, so the two `include_str!` paths below hold for all of them.

#![allow(dead_code)]

pub(crate) fn template_with_assistant_role() -> String {
	concat!(
		include_str!("../../config-templates/default.toml"),
		include_str!("assistant-role.toml")
	)
	.to_string()
}
