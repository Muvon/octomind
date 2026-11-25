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

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};
use uuid::Uuid;

use super::persistence::SessionLog;

mod index;
pub(crate) use index::search;

const MAX_CHUNK_BYTES: usize = 768;
const MAX_EXCERPT_BYTES: usize = 3 * 1024;
const MAX_ANCESTORS: usize = 16;
const BOUNDARY_SAMPLE_BYTES: u64 = 4096;
pub(crate) const DEFAULT_LIMIT: usize = 5;
pub(crate) const MAX_LIMIT: usize = 12;
pub(crate) const MAX_QUERY_BYTES: usize = 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Source {
	session: String,
	end: u64,
	boundary: [u8; 32],
	delegated: bool,
}

impl Source {
	pub(crate) fn capture(session: &str, path: &Path, delegated: bool) -> Result<Self> {
		let expected = super::logger::get_session_log_file(session)?;
		anyhow::ensure!(
			expected == path,
			"delegation source does not match the current session log"
		);
		let mut file = std::fs::File::open(path).context("locating the delegation source log")?;
		file.lock_shared()?;
		let end = file.metadata()?.len();
		let boundary = boundary_digest(&mut file, end)?;
		Ok(Self {
			session: session.to_owned(),
			end,
			boundary,
			delegated,
		})
	}

	fn path(&self) -> Result<PathBuf> {
		anyhow::ensure!(
			Path::new(&self.session).file_name().and_then(|name| name.to_str()) == Some(self.session.as_str()),
			"invalid delegation source session"
		);
		super::logger::get_session_log_file(&self.session)
	}

	fn verify(&self) -> Result<()> {
		let mut file = std::fs::File::open(self.path()?).context("opening the original delegation log")?;
		anyhow::ensure!(
			file.metadata()?.len() >= self.end,
			"original delegation log was truncated"
		);
		anyhow::ensure!(
			boundary_digest(&mut file, self.end)? == self.boundary,
			"original delegation log has changed"
		);
		Ok(())
	}

	fn open(&self, start: u64, end: u64) -> Result<SessionLog<std::io::BufReader<std::fs::File>>> {
		anyhow::ensure!(
			start <= end && end <= self.end,
			"delegation search range exceeds its bound"
		);
		SessionLog::open(&self.path()?)?.through(end).at_frame(start)
	}
}

fn boundary_digest(file: &mut std::fs::File, end: u64) -> Result<[u8; 32]> {
	use sha2::{Digest, Sha256};
	let length = end.min(BOUNDARY_SAMPLE_BYTES) as usize;
	let mut sample = [0; BOUNDARY_SAMPLE_BYTES as usize];
	let mut hash = Sha256::new();
	file.read_exact(&mut sample[..length])?;
	hash.update(&sample[..length]);
	if end > BOUNDARY_SAMPLE_BYTES {
		file.seek(SeekFrom::Start(end - BOUNDARY_SAMPLE_BYTES))?;
		file.read_exact(&mut sample)?;
		hash.update(sample);
	}
	Ok(hash.finalize().into())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Grant {
	id: String,
	child: String,
	workspace: PathBuf,
	sources: Vec<Source>,
}

pub(crate) const ACP_META: &str = "octomind.delegationContext";
static GRANTS: LazyLock<RwLock<HashMap<String, Arc<Grant>>>> = LazyLock::new(Default::default);
tokio::task_local! {
	pub(crate) static DISPATCH: Option<(PathBuf, Option<String>)>;
}

pub(crate) fn launch_request(messages: &[super::Message]) -> Option<String> {
	if current_grant().is_some() {
		return None;
	}
	let text = super::latest_real_user_task_content(messages)?;
	if contains_credentials(text) {
		return None;
	}
	Some(text[..text.floor_char_boundary(MAX_CHUNK_BYTES)].to_owned())
}

pub(crate) fn current_grant() -> Option<Arc<Grant>> {
	let session = super::context::current_session_id()?;
	GRANTS
		.read()
		.expect("delegation grants poisoned")
		.get(&session)
		.cloned()
}

pub(crate) fn clear_for_session(session: &str) {
	GRANTS.write().expect("delegation grants poisoned").remove(session);
}

fn grant_path(child: &str) -> Result<PathBuf> {
	anyhow::ensure!(
		child.starts_with("tap-") && Path::new(child).file_name().and_then(|name| name.to_str()) == Some(child),
		"invalid delegation child session"
	);
	Ok(crate::directories::get_sessions_dir()?
		.join("delegation")
		.join(format!("{child}.json")))
}

pub(crate) struct ActiveGrant {
	pub(crate) id: Uuid,
	_leases: Vec<index::Lease>,
}

impl ActiveGrant {
	fn new(id: Uuid, sources: Vec<Source>) -> Self {
		Self {
			id,
			_leases: sources.into_iter().filter_map(index::queue).collect(),
		}
	}
}

pub(crate) fn advance(session: &str, path: &Path) {
	if let Err(error) = index::advance(session, path) {
		crate::log_debug!("Delegation index advance {}: {:#}", session, error);
	}
}

pub(crate) fn issue(child: &str, workspace: PathBuf, source: Source) -> Result<ActiveGrant> {
	let mut sources = vec![source];
	if let Some(parent) = current_grant() {
		anyhow::ensure!(parent.workspace == workspace, "delegation workspace changed");
		anyhow::ensure!(
			parent.sources.len() < MAX_ANCESTORS,
			"delegation ancestor depth limit reached"
		);
		sources.extend(parent.sources.iter().cloned());
	}
	let id = Uuid::new_v4();
	let grant = Grant {
		id: id.to_string(),
		child: child.to_owned(),
		workspace,
		sources,
	};
	let path = grant_path(child)?;
	std::fs::create_dir_all(path.parent().expect("grant path has a directory"))?;
	anyhow::ensure!(!path.exists(), "delegation grant already exists for {child}");
	crate::local_inference::publish(&path, &grant)?;
	Ok(ActiveGrant::new(id, grant.sources))
}

fn read_grant(child: &str) -> Result<Option<Grant>> {
	let path = grant_path(child)?;
	let raw = match std::fs::read(&path) {
		Ok(raw) => raw,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
		Err(error) => return Err(error).context("reading delegation grant"),
	};
	let grant: Grant = serde_json::from_slice(&raw).context("invalid delegation grant")?;
	anyhow::ensure!(grant.child == child, "delegation grant belongs to a different child");
	anyhow::ensure!(
		(1..=MAX_ANCESTORS).contains(&grant.sources.len()),
		"invalid delegation ancestor chain"
	);
	Ok(Some(grant))
}

pub(crate) fn resume(child: &str, parent: &str, workspace: &Path) -> Result<Option<ActiveGrant>> {
	let Some(grant) = read_grant(child)? else {
		return Ok(None);
	};
	anyhow::ensure!(
		grant.workspace == workspace.canonicalize()? && grant.sources[0].session == parent,
		"delegation grant belongs to a different parent or workspace"
	);
	Ok(Some(ActiveGrant::new(Uuid::parse_str(&grant.id)?, grant.sources)))
}

pub(crate) fn bind(child: &str, workspace: &Path, id: Uuid) -> Result<()> {
	let grant = read_grant(child)?.context("authorized delegation context is unavailable")?;
	anyhow::ensure!(
		grant.id == id.to_string() && grant.workspace == workspace.canonicalize()?,
		"delegation grant does not match this launch"
	);
	GRANTS
		.write()
		.expect("delegation grants poisoned")
		.insert(child.to_owned(), Arc::new(grant));
	Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Origin {
	UserInstruction,
	Delegation,
	AssistantDiscussion,
	ToolEvidence,
}

#[derive(Debug, Serialize)]
pub(crate) struct Passage {
	depth: usize,
	source: Origin,
	reference: String,
	excerpt: String,
}

static CREDENTIAL_TEXT: LazyLock<regex::Regex> = LazyLock::new(|| {
	regex::Regex::new(
		r#"(?im)(?:-----BEGIN [A-Z ]*PRIVATE KEY-----|\b(?:sk-(?:proj-)?|gh[pousr]_|github_pat_|xox[baprs]-|AKIA)[A-Za-z0-9_-]{12,}|\bauthorization\s*:\s*(?:bearer|basic)\s+\S+|\b(?:[a-z0-9_]*_)?(?:api[_-]?key|access[_-]?token|secret|password)\s*[:=]\s*[^\s,;]{8,}|"(?:[a-z0-9_]*_)?(?:api[_-]?key|access[_-]?token|secret|password)"\s*:\s*"(?:\\.|[^"\\])+"|://[^/\s:@]+:[^/\s@]+@)"#,
	).expect("credential pattern is valid")
});

fn contains_credentials(text: &str) -> bool {
	CREDENTIAL_TEXT.is_match(text)
}

fn project(message: &super::Message, delegated: bool) -> Option<(Origin, String)> {
	let (origin, text) = match message.role.as_str() {
		"user" if super::is_real_user_task_message(message) => (
			if delegated {
				Origin::Delegation
			} else {
				Origin::UserInstruction
			},
			message.content.clone(),
		),
		"assistant" if !crate::session::chat::conversation_compression::is_conversation_summary(message) => (
			Origin::AssistantDiscussion,
			super::output::visible_assistant_content(&message.content),
		),
		// Start acknowledgements are runtime-generated. Monitor listings can
		// incorporate arbitrary labels and remote task status.
		"tool" if message.name.as_deref() == Some("monitor") && message.content.starts_with("Started monitor [") => {
			(Origin::ToolEvidence, message.content.clone())
		}
		_ => return None,
	};
	(!contains_credentials(&text)).then_some((origin, text))
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Scope {
	Ancestors,
	Parent,
	Root,
}

impl Scope {
	fn includes(self, index: usize, count: usize) -> bool {
		match self {
			Self::Ancestors => true,
			Self::Parent => index == 0,
			Self::Root => index + 1 == count,
		}
	}
}

#[cfg(test)]
#[path = "delegation_context_tests.rs"]
mod tests;
