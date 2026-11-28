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

//! Authoring adds a manifest to the router's candidates, never a callable tool.
use crate::agent::{manifest, registry, taps};
use crate::config::Config;
use crate::mcp::{McpToolCall, McpToolResult};
use crate::session::context;
use anyhow::{ensure, Context, Result};
use rmcp::model::{ElicitRequestParams, ElicitationAction};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
	action: String,
	name: String,
	description: String,
	system: String,
	model: Option<String>,
	#[serde(default)]
	tools: Vec<String>,
}

pub(super) async fn execute(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	let result = author(call, config).await;
	Ok(match result {
		Ok(value) => McpToolResult::success(call.tool_name.clone(), call.tool_id.clone(), value.to_string()),
		Err(error) => McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), format!("{error:#}")),
	})
}

async fn author(call: &McpToolCall, config: &Config) -> Result<Value> {
	let session = context::current_session_id().context("tap authoring requires an active session")?;
	let mode = context::current_session_mode();
	ensure!(
		!mode.readonly && !mode.copy,
		"a read-only run or a copy cannot author roles"
	);
	let caller = context::get_session_role(&session).context("session has no role")?;
	let owner = config.tap_manifest.as_ref().filter(|m| m.tag == caller);
	ensure!(
		!owner.is_some_and(|m| m.tag.contains(':')),
		"a tap variant cannot create or promote another specialist"
	);
	if call.parameters.get("action").and_then(Value::as_str).map(str::trim) == Some("persist") {
		let tag = call
			.parameters
			.get("role")
			.and_then(Value::as_str)
			.context("role: the tag returned by create is required")?;
		let m = manifest::private_role(&session, tag)?;
		approve(&session, &call.tool_id, &m).await?;
		// Never apply approval to a different incarnation of the same session/tag.
		ensure!(
			std::sync::Arc::ptr_eq(&m, &manifest::private_role(&session, tag)?),
			"Role changed while waiting for approval"
		);
		return Ok(json!({"role": tag, "path": m.persist()?}));
	}
	let def: Create = serde_json::from_value(call.parameters.clone()).context("invalid tap create parameters")?;
	ensure!(def.action.trim() == "create", "invalid authoring action");
	ensure!(
		registry::is_valid_agent_name(&def.name) && def.name != registry::DEFAULT_VARIANT,
		"name must be a lowercase variant slug, other than 'default'"
	);
	if let Some(model) = &def.model {
		let mut profile = config.model_profile.clone();
		profile.model = model.clone();
		profile.validate("tap create model")?;
	}
	let namespace = owner.map_or("personal", |m| m.tag.as_str());
	let tag = format!("{namespace}:{}", def.name);
	ensure!(
		!taps::has_agent(&tag)? && taps::resolve_alias_tag(&tag)? == tag && !config.has_role(&tag),
		"role '{tag}' already exists"
	);
	let mut grants = manifest::Grants::new();
	if !def.tools.is_empty() {
		let available = crate::mcp::get_available_functions(config).await;
		let child_base = Config::load().context("cannot resolve the child process's server catalog")?;
		for tool in &def.tools {
			ensure!(
				!tool.is_empty() && !tool.contains([':', '*']) && available.iter().any(|f| &f.name == tool),
				"tool '{tool}' is not available to the creator"
			);
			let server = crate::mcp::tool_map::get_server_for_tool(tool)
				.with_context(|| format!("tool '{tool}' has no server"))?;
			ensure!(
				!(server.connection_type() == crate::config::McpConnectionType::Builtin
					&& (["runtime", "orchestration", "local"].contains(&server.name()) || ["code", "search_tools", "context"].contains(&tool.as_str()))),
				"tool '{tool}' manages, delegates or depends on session-local authority; grant a concrete worker tool instead"
			);
			let cap = crate::session::guardrails::resolve_capability(&session, config, Some(server.name()), tool);
			ensure!(
				!crate::session::guardrails::get_rules(&session).is_some_and(|rules| rules
					.guards
					.iter()
					.any(|guard| Some(guard.trigger.capability.as_str()) == cap.as_deref())),
				"tool '{tool}' has creator guard conditions that cannot be transferred"
			);
			let persisted = child_base
				.mcp
				.servers
				.iter()
				.find(|s| s.name() == server.name())
				.with_context(|| format!("tool '{tool}' needs a server configured for child processes"))?;
			let mut expected = persisted.clone();
			*expected.tools_mut() = server.tools().to_vec();
			ensure!(expected == server && crate::mcp::is_tool_allowed_by_patterns(tool,persisted.tools())
				&& !crate::mcp::runtime::dynamic::is_dynamic(server.name()),
				"tool '{tool}' requires the same configured server and permission in the child; session-only registrations cannot be delegated");
			grants.insert(tool.clone(), server.name().to_owned());
		}
	}
	manifest::register(
		&session,
		manifest::Manifest::create(
			tag.clone(),
			&def.description,
			&def.system,
			def.model.as_deref(),
			&grants,
		)?,
	)?;
	Ok(json!({"role":tag, "scope":"session"}))
}

async fn approve(session: &str, tool_id: &str, m: &manifest::Manifest) -> Result<()> {
	let params = ElicitRequestParams::FormElicitationParams {
		meta: None,
		message: format!(
			"Save {} into your personal tap for use by future sessions?\n{}",
			m.tag,
			m.raw.escape_debug()
		),
		requested_schema: serde_json::from_value(
			json!({"type":"object","properties":{"approve":{"type":"boolean","title":"Save this role"}},"required":["approve"]}),
		)
		.expect("valid approval schema"),
	};
	let result = crate::mcp::elicitation::ask("orchestration", Some(session), Some(tool_id), params).await?;
	ensure!(
		result.action == ElicitationAction::Accept
			&& result.content.and_then(|v| v.get("approve").and_then(Value::as_bool)) == Some(true),
		"persist requires explicit user approval"
	);
	Ok(())
}

#[cfg(test)]
#[path = "tap_author_tests.rs"]
mod tests;
