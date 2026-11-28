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

//! The manifest chosen by the router is the one the ACP subprocess loads, also on resume.
use super::{registry, taps};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

const AUTHORED_HEADER: &str = "# Created by octomind tap\n";
pub(crate) type Grants = BTreeMap<String, String>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
	pub tag: String,
	pub(crate) root: PathBuf,
	pub(crate) raw: String,
}

impl Manifest {
	pub(crate) fn header(&self) -> taps::TapAgentHeader {
		taps::TapAgentHeader {
			tag: self.tag.clone(),
			header: registry::parse_agent_header(&self.raw),
		}
	}

	pub(crate) fn authored(&self) -> bool {
		self.raw.starts_with(AUTHORED_HEADER)
	}

	pub(crate) fn create(
		tag: String,
		description: &str,
		system: &str,
		model: Option<&str>,
		grants: &Grants,
	) -> Result<Self> {
		ensure!(
			!description.trim().is_empty() && !description.chars().any(char::is_control),
			"description must be a single non-empty line"
		);
		ensure!(!system.trim().is_empty(), "system must be a non-empty string");
		let (namespace, variant) = registry::manifest_parts(&tag);
		ensure!(
			registry::is_valid_agent_name(namespace)
				&& registry::is_valid_agent_name(variant)
				&& variant != registry::DEFAULT_VARIANT,
			"expected a namespace and non-default variant"
		);
		let refs: std::collections::BTreeSet<_> = grants.values().collect();
		let tools: Vec<_> = grants.iter().map(|(tool, server)| format!("{server}:{tool}")).collect();
		let mut role =
			serde_json::json!({"system":system, "welcome":"", "mcp":{"server_refs":refs, "allowed_tools":tools}});
		if let Some(model) = model {
			role["model"] = serde_json::json!({"name":model});
		}
		let body = toml::to_string(&serde_json::json!({"roles":[role]}))?;
		let raw = format!("{AUTHORED_HEADER}# Title: {variant}\n# Description: {description}\n\n{body}");
		Ok(Self {
			tag,
			raw,
			root: PathBuf::new(),
		})
	}

	pub(crate) fn grants(&self, role: &crate::config::RoleMcpConfig) -> Result<Option<Grants>> {
		if !self.authored() {
			return Ok(None);
		}
		let grants = role
			.allowed_tools
			.iter()
			.map(|grant| {
				let (server, tool) = grant
					.split_once(':')
					.context("authored role needs exact server:tool grants")?;
				ensure!(
					!tool.is_empty() && !server.is_empty() && !grant.contains('*'),
					"invalid authored tool grant"
				);
				Ok((tool.to_owned(), server.to_owned()))
			})
			.collect::<Result<_>>()?;
		Ok(Some(grants))
	}

	pub(crate) fn pin(&self, run: &str) -> Result<()> {
		let path = snapshot_path(run)?;
		publish_new(&path, &serde_json::to_vec(self)?)
	}

	pub fn read_run(run: &str, tag: &str) -> Result<Self> {
		let path = snapshot_path(run)?;
		let raw = std::fs::read(&path)
			.with_context(|| format!("Cannot resume {run} without its pinned tap manifest; start a fresh run"))?;
		let manifest: Self = serde_json::from_slice(&raw).context("invalid tap run manifest")?;
		ensure!(manifest.tag == tag, "tap manifest does not belong to role '{tag}'");
		Ok(manifest)
	}

	pub(crate) fn persist(&self) -> Result<PathBuf> {
		ensure!(self.authored(), "only a created role may be persisted");
		let path = taps::personal_tap().agent_manifest(&self.tag)?;
		ensure!(
			!taps::has_agent(&self.tag)? && taps::resolve_alias_tag(&self.tag)? == self.tag,
			"tap role '{}' already exists",
			self.tag
		);
		publish_new(&path, self.raw.as_bytes())?;
		Ok(path)
	}
}

fn snapshot_path(run: &str) -> Result<PathBuf> {
	ensure!(
		run.starts_with("tap-") && run.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
		"invalid tap run id"
	);
	Ok(crate::directories::get_sessions_dir()?
		.join("tap-manifests")
		.join(format!("{run}.json")))
}

fn publish_new(path: &Path, bytes: &[u8]) -> Result<()> {
	let parent = path.parent().context("manifest needs a parent directory")?;
	std::fs::create_dir_all(parent)?;
	let mut file = tempfile::NamedTempFile::new_in(parent)?;
	file.write_all(bytes)?;
	file.persist_noclobber(path)
		.with_context(|| format!("Cannot publish manifest at {}", path.display()))?;
	Ok(())
}

type Definitions = BTreeMap<String, Arc<Manifest>>;
static PRIVATE: LazyLock<RwLock<HashMap<String, Definitions>>> = LazyLock::new(Default::default);

pub(crate) fn init(session: &str) {
	PRIVATE
		.write()
		.expect("private role lock poisoned")
		.entry(session.to_owned())
		.or_default();
}
pub(crate) fn clear(session: &str) {
	PRIVATE.write().expect("private role lock poisoned").remove(session);
}
pub(crate) fn register(session: &str, manifest: Manifest) -> Result<()> {
	let mut sessions = PRIVATE.write().expect("private role lock poisoned");
	let defs = sessions
		.get_mut(session)
		.context("tap authoring requires an active session")?;
	ensure!(
		!defs.contains_key(&manifest.tag),
		"private role '{}' already exists",
		manifest.tag
	);
	defs.insert(manifest.tag.clone(), Arc::new(manifest));
	Ok(())
}

pub(crate) fn private_role(session: &str, tag: &str) -> Result<Arc<Manifest>> {
	PRIVATE
		.read()
		.expect("private role lock poisoned")
		.get(session)
		.and_then(|defs| defs.get(tag))
		.cloned()
		.context("no such private role in this session")
}

pub(crate) fn catalog(session: Option<&str>, caller: Option<&Arc<Manifest>>) -> Result<Definitions> {
	let mut result = taps::installed_manifests()?
		.into_iter()
		.map(|m| (m.tag.clone(), Arc::new(m)))
		.collect::<Definitions>();
	if let Some(owned) = session.and_then(|id| PRIVATE.read().expect("private role lock poisoned").get(id).cloned()) {
		result.extend(owned);
	}
	if let Some(caller) = caller {
		result.insert(caller.tag.clone(), caller.clone());
	}
	Ok(result)
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
