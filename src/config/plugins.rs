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

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Items of an array of tables cannot take defaults from the template, so the
/// defaults of a `[[plugins]]` entry live here (as `default_hook_timeout`
/// does for hooks). The timeout applies to an event the plugin declared no
/// time for, and to the handshake.
pub const DEFAULT_PLUGIN_TIMEOUT_SECONDS: u64 = 30;
pub const MAX_PLUGIN_TIMEOUT_SECONDS: u64 = 3600;
/// Length in bytes of an Ed25519 public key.
pub const INSTALL_KEY_BYTES: usize = 32;

/// Name of the plugin a new configuration lists.
pub const DEFAULT_PLUGIN_NAME: &str = "besydr";

/// The `plugins` value of a new configuration: Besydr, the program that does
/// the supervising and learning that was built into octomind up to config
/// version 31. `besydr connect` joins Besydr's shared local server and starts
/// it when needed. Octomind downloads the program on first use from the signed
/// release the entry names, and never updates it by itself.
///
/// An inline table, so the key stays above the first `[table]` header of the
/// configuration file.
pub const DEFAULT_PLUGINS_TOML: &str = r#"[
	{ name = "besydr", type = "stdio", command = "besydr", args = ["connect"], install_manifest = "https://cdn.besydr.com/latest.json", install_key = "uU5IoojVdfJZj5/fU4JQE8CXnmYXNBGo5nyLgWcNNrw=" },
]"#;

/// The embedded default configuration. Its `plugins` list is empty, so that a
/// configuration parsed from the template alone, as tests do, starts no program.
const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../../config-templates/default.toml");
const TEMPLATE_PLUGINS_LINE: &str = "\nplugins = []\n";

/// The text of a new configuration: the template with the default plugin in
/// its `plugins` list.
pub fn new_config_text() -> String {
	DEFAULT_CONFIG_TEMPLATE.replacen(
		TEMPLATE_PLUGINS_LINE,
		&format!("\nplugins = {DEFAULT_PLUGINS_TOML}\n"),
		1,
	)
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
	/// Octomind starts the program and talks over its standard input and output.
	Stdio,
	/// Octomind connects to a local unix socket.
	Socket,
	/// Octomind posts each message to a URL.
	Http,
}

impl PluginKind {
	pub fn as_str(self) -> &'static str {
		match self {
			PluginKind::Stdio => "stdio",
			PluginKind::Socket => "socket",
			PluginKind::Http => "http",
		}
	}
}

/// One `[[plugins]]` entry: a separate program that takes part in the agent loop.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PluginConfig {
	/// Unique name, used in notices and logs.
	pub name: String,
	#[serde(rename = "type")]
	pub kind: PluginKind,
	/// Program to start. Required for `stdio`; for `socket` it is started only
	/// when nothing answers on `path`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub command: Option<String>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub args: Vec<String>,
	/// Socket path, for `socket`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub path: Option<String>,
	/// Endpoint, for `http`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub url: Option<String>,
	/// Extra request headers, for `http`. Values support `{{ENV:KEY}}`.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub headers: BTreeMap<String, String>,
	#[serde(default = "default_enabled")]
	pub enabled: bool,
	/// Roles the plugin applies to. Empty means all roles.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub roles: Vec<String>,
	/// Upper limit for one event. A plugin declares the time it needs per
	/// event; with this set, the smaller of the two applies.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub timeout_seconds: Option<u64>,
	/// URL of a signed release manifest. When `command` is not installed,
	/// octomind downloads the program for this machine from it. Needs `install_key`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub install_manifest: Option<String>,
	/// Ed25519 public key (base64 of 32 bytes) the manifest must be signed with.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub install_key: Option<String>,
}

fn default_enabled() -> bool {
	true
}

fn is_blank(value: &Option<String>) -> bool {
	value.as_deref().is_none_or(|text| text.trim().is_empty())
}

impl PluginConfig {
	pub fn validate(&self) -> Result<()> {
		let name = &self.name;
		if name.trim().is_empty() {
			bail!("Plugin has empty name");
		}
		match self.kind {
			PluginKind::Stdio => {
				if is_blank(&self.command) {
					bail!("Plugin '{}' has type stdio and needs `command`", name);
				}
			}
			PluginKind::Socket => {
				if cfg!(windows) {
					bail!(
						"Plugin '{}' has type socket, which is not supported on Windows. Use type stdio",
						name
					);
				}
				if is_blank(&self.path) {
					bail!("Plugin '{}' has type socket and needs `path`", name);
				}
			}
			PluginKind::Http => {
				let Some(url) = self.url.as_deref().filter(|url| !url.trim().is_empty()) else {
					bail!("Plugin '{}' has type http and needs `url`", name);
				};
				let parsed = url::Url::parse(url)
					.map_err(|error| anyhow::anyhow!("Plugin '{}' has invalid url: {}", name, error))?;
				if !matches!(parsed.scheme(), "http" | "https") {
					bail!("Plugin '{}' url must start with http:// or https://", name);
				}
			}
		}
		if let Some(seconds) = self.timeout_seconds {
			if seconds == 0 {
				bail!("Plugin '{}' timeout_seconds must be > 0", name);
			}
			if seconds > MAX_PLUGIN_TIMEOUT_SECONDS {
				bail!(
					"Plugin '{}' timeout_seconds too high: {} (max {})",
					name,
					seconds,
					MAX_PLUGIN_TIMEOUT_SECONDS
				);
			}
		}
		self.validate_install()
	}

	fn validate_install(&self) -> Result<()> {
		let name = &self.name;
		let (manifest, key) = match (self.install_manifest.as_deref(), self.install_key.as_deref()) {
			(None, None) => return Ok(()),
			(Some(manifest), Some(key)) => (manifest, key),
			_ => bail!(
				"Plugin '{}' needs both install_manifest and install_key, or neither",
				name
			),
		};
		if !matches!(self.kind, PluginKind::Stdio | PluginKind::Socket) {
			bail!(
				"Plugin '{}' has type {}; install_manifest works only with stdio and socket",
				name,
				self.kind.as_str()
			);
		}
		if is_blank(&self.command) {
			bail!("Plugin '{}' has install_manifest and needs `command`", name);
		}
		let parsed = url::Url::parse(manifest)
			.map_err(|error| anyhow::anyhow!("Plugin '{}' has invalid install_manifest: {}", name, error))?;
		if parsed.scheme() != "https" {
			bail!("Plugin '{}' install_manifest must start with https://", name);
		}
		decode_install_key(key).with_context(|| format!("Plugin '{}' has invalid install_key", name))?;
		Ok(())
	}

	/// Whether the plugin applies to `role`, with the matching rule of guardrails.
	pub fn applies_to_role(&self, role: &str) -> bool {
		crate::config::guardrails::role_matches(&self.roles, role)
	}
}

/// The raw Ed25519 public key behind an `install_key` value.
pub fn decode_install_key(text: &str) -> Result<[u8; INSTALL_KEY_BYTES]> {
	let bytes = STANDARD.decode(text.trim()).context("not valid base64")?;
	bytes
		.try_into()
		.map_err(|bytes: Vec<u8>| anyhow::anyhow!("key is {} bytes, expected {}", bytes.len(), INSTALL_KEY_BYTES))
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
