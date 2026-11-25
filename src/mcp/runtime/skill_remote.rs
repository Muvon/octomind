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

use crate::config::McpServerConfig;
use crate::mcp::client::{self, McpService};
use crate::session::context;
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rmcp::model::{
	sha256_digest, ClientRequest, ElicitRequestParams, ElicitationAction, GetSkillRequest, GetSkillRequestParams,
	ListSkillsRequest, PaginatedRequestParams, ReadResourceRequest, ReadResourceRequestParams, ResourceContents,
	ServerResult, Skill, SkillFrontmatter, SkillResources,
};
use rmcp::service::PeerRequestOptions;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

const MAX_SKILL_FILES: usize = 512;
const MAX_SKILL_BYTES: u64 = 16 * 1024 * 1024;

fn sources() -> BTreeMap<String, (McpServerConfig, Arc<McpService>)> {
	let mut servers = crate::mcp::tool_map::role_servers();
	servers.extend(crate::mcp::runtime::dynamic::get_all_configs());
	servers
		.into_iter()
		.filter_map(|server| {
			let name = server.name().to_owned();
			let connection = client::get(&name)?;
			(!connection.is_closed()
				&& connection
					.peer_info()
					.is_some_and(|info| info.capabilities.supports_skills()))
			.then_some((name, (server, connection)))
		})
		.collect()
}

async fn request(server: &McpServerConfig, service: &McpService, request: ClientRequest) -> Result<ServerResult> {
	// Shared connections otherwise carry the capabilities of the session that opened them.
	let options = PeerRequestOptions::with_timeout(Duration::from_secs(server.timeout_seconds()))
		.with_meta(client::session_request_meta(server));
	Ok(service
		.peer()
		.send_request_with_option(request, options)
		.await?
		.await_response()
		.await?)
}

pub(super) async fn list() -> Result<Vec<(String, Skill)>> {
	let mut skills = Vec::new();
	for (name, (server, connection)) in sources() {
		let mut cursor = None;
		let mut seen = HashSet::new();
		loop {
			let params = PaginatedRequestParams::default().with_cursor(cursor);
			let response = request(
				&server,
				&connection,
				ClientRequest::ListSkillsRequest(ListSkillsRequest::with_param(params)),
			)
			.await
			.with_context(|| format!("skills/list on '{name}'"))?;
			let ServerResult::ListSkillsResult(page) = response else {
				bail!("Unexpected skills/list response from '{name}'");
			};
			for skill in page.skills {
				skill
					.validate()
					.with_context(|| format!("Invalid skill '{}' on '{name}'", skill.uri))?;
				skills.push((name.clone(), skill));
			}
			let Some(next) = page.next_cursor else { break };
			ensure!(seen.insert(next.clone()), "Repeated skills/list cursor from '{name}'");
			cursor = Some(next);
		}
	}
	Ok(skills)
}

pub(super) fn reference(name: &str) -> Option<(Option<&str>, &str)> {
	if name.starts_with("skill://") {
		return Some((None, name));
	}
	let (server, uri) = name.split_once(':')?;
	rmcp::model::skill_name_from_uri(uri)?;
	Some((Some(server), uri))
}

pub(super) async fn load((server_name, uri): (Option<&str>, &str), call: &crate::mcp::McpToolCall) -> Result<String> {
	let session_id = context::current_session_id().context("MCP skill use requires an active session")?;
	let mut sources = sources();
	let (server, connection) = match server_name {
		Some(name) => sources
			.remove(name)
			.with_context(|| format!("No connected MCP skills server '{name}' in this session"))?,
		None if sources.len() == 1 => {
			let (_, source) = sources.pop_first().expect("exactly one MCP skills server");
			source
		}
		None => bail!("Use a server-qualified MCP skill reference from skill list"),
	};
	let name = server.name();
	let reference = format!("{name}:{uri}");
	let response = request(
		&server,
		&connection,
		ClientRequest::GetSkillRequest(GetSkillRequest::new(GetSkillRequestParams::new(uri))),
	)
	.await
	.with_context(|| format!("skills/get for '{uri}' on '{name}'"))?;
	let ServerResult::GetSkillResult(result) = response else {
		bail!("Unexpected skills/get response from '{name}'");
	};
	let skill = result.skill;
	skill.validate().context("Invalid MCP skill manifest")?;
	ensure!(skill.uri == uri, "skills/get returned '{}' for '{uri}'", skill.uri);

	let SkillResources::Files(files) = &skill.resources else {
		bail!("MCP skill '{name}:{uri}' has a dynamic manifest");
	};
	ensure!(
		files.len() <= MAX_SKILL_FILES
			&& files
				.iter()
				.try_fold(0u64, |size, file| size.checked_add(file.size))
				.is_some_and(|size| size <= MAX_SKILL_BYTES),
		"MCP skill '{name}:{uri}' exceeds the manifest size limits"
	);
	let mut sorted_files: Vec<_> = files.iter().collect();
	sorted_files.sort_unstable_by(|a, b| a.uri.cmp(&b.uri));
	let mut frontmatter = serde_json::to_value(&skill.frontmatter)?;
	frontmatter.sort_all_objects();
	let manifest = sha256_digest(&serde_json::to_vec(&(name, &skill.uri, frontmatter, sorted_files))?);
	let resource = match call.parameters.get("resource") {
		Some(Value::String(resource)) if !resource.is_empty() => {
			ensure!(
				call.parameters.get("manifest").and_then(Value::as_str) == Some(manifest.as_str())
					&& context::has_remote_skill_manifest(&session_id, &reference, &manifest),
				"MCP skill manifest changed or is missing. Load '{name}:{uri}' again"
			);
			ensure!(
				rmcp::model::skill_name_from_uri(resource).is_none(),
				"Load a SKILL.md with its own skill reference and user approval"
			);
			resource.as_str()
		}
		Some(_) => bail!("resource must be a non-empty string"),
		None => {
			approve(name, &session_id, uri, &skill.frontmatter.description, &call.tool_id).await?;
			uri
		}
	};
	let file = files
		.iter()
		.find(|file| file.uri == resource)
		.with_context(|| format!("Resource '{resource}' is absent from the MCP skill manifest"))?;
	let response = request(
		&server,
		&connection,
		ClientRequest::ReadResourceRequest(ReadResourceRequest::new(ReadResourceRequestParams::new(resource))),
	)
	.await
	.with_context(|| format!("resources/read for '{resource}' on '{name}'"))?;
	let ServerResult::ReadResourceResult(result) = response else {
		bail!("Unexpected resources/read response from '{name}'");
	};
	let bytes = match result.contents.as_slice() {
		[ResourceContents::TextResourceContents { uri, text, .. }] if uri == resource => text.as_bytes().to_vec(),
		[ResourceContents::BlobResourceContents { uri, blob, .. }] if uri == resource => STANDARD.decode(blob)?,
		_ => bail!("Expected one resource matching '{resource}' from '{name}'"),
	};
	file.verify(&bytes).context("MCP skill resource verification failed")?;

	if resource != uri {
		return Ok(match String::from_utf8(bytes) {
			Ok(text) => format!("MCP skill resource {name}:{resource}\n\n{text}"),
			Err(error) => format!(
				"MCP skill resource {name}:{resource} (base64)\n\n{}",
				STANDARD.encode(error.as_bytes())
			),
		});
	}
	let content = std::str::from_utf8(&bytes).context("MCP SKILL.md must be UTF-8")?;
	let body = verified_body(content, &skill.frontmatter)?;
	let resources = files
		.iter()
		.filter(|file| file.uri != uri)
		.map(|file| file.uri.as_str())
		.collect::<Vec<_>>();
	context::record_remote_skill_manifest(&session_id, reference, manifest.clone());
	Ok(format!(
		"MCP skill {name}:{uri}\nManifest {manifest}\nSupporting resources: {}\n\n{body}",
		resources.join(", ")
	))
}

async fn approve(server: &str, session_id: &str, uri: &str, description: &str, tool_id: &str) -> Result<()> {
	ensure!(
		!crate::agent::inputs::in_non_interactive_scope(),
		"MCP skills require an interactive approver"
	);
	let label = format!("{server}:{uri}").escape_debug().to_string();
	let description = description.escape_debug();
	let params = ElicitRequestParams::FormElicitationParams {
		meta: None,
		message: format!("Load MCP skill {label} ({description}) and let the agent follow its instructions, including requesting local code execution under this session's tool policy?"),
		requested_schema: serde_json::from_value(json!({
			"type": "object",
			"properties": { "approve": { "type": "boolean", "title": "Approve this MCP skill" } },
			"required": ["approve"]
		})).expect("valid skill approval schema"),
	};
	let result = crate::mcp::elicitation::ask(server, Some(session_id), Some(tool_id), params)
		.await
		.context("MCP skill approval failed")?;
	ensure!(
		result.action == ElicitationAction::Accept
			&& result
				.content
				.as_ref()
				.and_then(|content| content.get("approve"))
				.and_then(Value::as_bool)
				== Some(true),
		"MCP skill '{label}' requires explicit user approval"
	);
	Ok(())
}

fn verified_body<'a>(content: &'a str, expected: &SkillFrontmatter) -> Result<&'a str> {
	let content = content.trim_start_matches('\u{feff}');
	let opening = content.split_inclusive('\n').next().context("MCP SKILL.md is empty")?;
	ensure!(
		opening.trim_end_matches(['\r', '\n']) == "---",
		"MCP SKILL.md is missing YAML frontmatter"
	);
	let rest = &content[opening.len()..];
	let mut offset = 0;
	for line in rest.split_inclusive('\n') {
		if line.trim_end_matches(['\r', '\n']) == "---" {
			let actual: SkillFrontmatter =
				yaml_serde::from_str(&rest[..offset]).context("Invalid MCP skill YAML frontmatter")?;
			ensure!(
				&actual == expected,
				"MCP SKILL.md frontmatter does not match its manifest"
			);
			return Ok(&rest[offset + line.len()..]);
		}
		offset += line.len();
	}
	bail!("MCP SKILL.md has no closing frontmatter delimiter")
}

#[cfg(test)]
#[path = "skill_remote_tests.rs"]
mod tests;
