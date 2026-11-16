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

//! Finds a plugin's program, and downloads it from the signed release the
//! plugin's config entry names when it is not installed.
//!
//! The release is a JSON manifest with a detached Ed25519 signature next to it
//! (`<manifest url>.sig`). The signature is checked over the exact manifest
//! bytes before anything in the manifest is read, the archive must come over
//! https and match the manifest's SHA-256 before it is opened, and only the one
//! entry named like the command is taken out of it. An installed program is
//! never replaced by a newer release.

use crate::config::{decode_install_key, PluginConfig, INSTALL_KEY_BYTES};
use crate::mcp::client::resolve_env_placeholders;
use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tempfile::NamedTempFile;

/// Connect and per-read limit of every download. A total limit would cut off
/// a slow but working download of a large archive.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 200 * 1024 * 1024;
/// Upper limit for the unpacked program, against an archive that inflates far
/// beyond its size.
const MAX_PROGRAM_BYTES: u64 = 1024 * 1024 * 1024;
const SIGNATURE_SUFFIX: &str = ".sig";
const PLUGINS_DIR: &str = "plugins";
const VERSION_FILE: &str = "version";
#[cfg(unix)]
const PROGRAM_MODE: u32 = 0o755;
const SHA256_BYTES: usize = 32;

/// What `resolve_command` did when it had to download the program.
struct Installed {
	path: PathBuf,
	/// One line for the user: what was installed and where from.
	notice: String,
}

/// The release entry for this machine, read from a verified manifest.
#[derive(Debug, PartialEq)]
struct Download {
	version: String,
	url: String,
	sha256: String,
}

#[derive(Deserialize)]
struct Manifest {
	version: String,
	targets: HashMap<String, TargetEntry>,
}

#[derive(Deserialize)]
struct TargetEntry {
	url: String,
	sha256: String,
}

/// The program of `config` as a path that can be started: the command itself
/// when it names a file, else the first hit on `PATH`, else the copy in the
/// plugin directory, else a download from the signed release the entry names.
pub async fn resolve_command(config: &PluginConfig) -> Result<PathBuf> {
	let command = config
		.command
		.as_deref()
		.map(resolve_env_placeholders)
		.with_context(|| format!("Plugin '{}' has no command", config.name))?;
	let plugin_dir = plugin_dir(&config.name)?;
	if let Some(found) = locate(&command, &plugin_dir, std::env::var_os("PATH")) {
		return Ok(found);
	}
	let (Some(manifest_url), Some(key)) = (config.install_manifest.as_deref(), config.install_key.as_deref()) else {
		bail!(
			"Plugin '{}': program '{}' was not found on PATH or in {}, and the entry has no install_manifest",
			config.name,
			command,
			plugin_dir.display()
		);
	};
	let installed = install(&config.name, &command, &plugin_dir, manifest_url, key).await?;
	crate::log_debug!("{}", installed.notice);
	Ok(installed.path)
}

/// The program as far as it is already on this machine, without downloading.
/// `command` has its `{{ENV:KEY}}` values filled in.
pub fn find_installed(command: &str, plugin_name: &str) -> Result<Option<PathBuf>> {
	Ok(locate(command, &plugin_dir(plugin_name)?, std::env::var_os("PATH")))
}

fn locate(command: &str, plugin_dir: &Path, search_path: Option<OsString>) -> Option<PathBuf> {
	if command.contains(['/', std::path::MAIN_SEPARATOR]) {
		return Some(PathBuf::from(command));
	}
	if let Ok(found) = which::which_in(command, search_path, ".") {
		return Some(found);
	}
	let installed = plugin_dir.join(program_file_name(command, std::env::consts::EXE_SUFFIX));
	installed.is_file().then_some(installed)
}

/// File name of the program `command` names: on Windows programs end in `.exe`
/// (`exe_suffix`), which a plugin entry's `command` normally leaves out, and
/// both the release archive and the installed file carry it.
fn program_file_name(command: &str, exe_suffix: &str) -> String {
	if command.to_ascii_lowercase().ends_with(exe_suffix) {
		command.to_string()
	} else {
		format!("{command}{exe_suffix}")
	}
}

fn plugin_dir(plugin_name: &str) -> Result<PathBuf> {
	plain_file_name(plugin_name).context("plugin name cannot be used as a directory name")?;
	Ok(crate::directories::get_octomind_data_dir()?
		.join(PLUGINS_DIR)
		.join(plugin_name))
}

/// `name` must be one ordinary path component: no separators, no `..`.
fn plain_file_name(name: &str) -> Result<()> {
	let mut components = Path::new(name).components();
	match (components.next(), components.next()) {
		(Some(Component::Normal(_)), None) => Ok(()),
		_ => bail!("'{}' is not a plain file name", name),
	}
}

async fn install(
	plugin_name: &str,
	command: &str,
	plugin_dir: &Path,
	manifest_url: &str,
	key: &str,
) -> Result<Installed> {
	plain_file_name(command).context("only a plain program name can be installed")?;
	let key = decode_install_key(key)?;
	let client = download_client()?;

	let manifest = fetch(&client, manifest_url, MAX_MANIFEST_BYTES).await?;
	let signature = fetch(
		&client,
		&format!("{manifest_url}{SIGNATURE_SUFFIX}"),
		MAX_MANIFEST_BYTES,
	)
	.await?;
	verify_manifest(&manifest, &signature, &key)?;
	let download = plan_download(&manifest, this_target()?)?;

	std::fs::create_dir_all(plugin_dir)
		.with_context(|| format!("Failed to create the plugin directory {}", plugin_dir.display()))?;
	let mut archive =
		NamedTempFile::new_in(plugin_dir).context("Failed to create a temporary file for the download")?;
	let mut hasher = Sha256::new();
	fetch_into(&client, &download.url, MAX_ARCHIVE_BYTES, |chunk| {
		hasher.update(chunk);
		archive.write_all(chunk).context("Failed to write the download")
	})
	.await?;
	check_sha256(&hex::encode(hasher.finalize()), &download.sha256)?;

	let archive_path = archive.path().to_path_buf();
	let program_name = program_file_name(command, std::env::consts::EXE_SUFFIX);
	let (entry_name, dir) = (program_name.clone(), plugin_dir.to_path_buf());
	let program = tokio::task::spawn_blocking(move || unpack_program(&archive_path, &entry_name, &dir))
		.await
		.context("The unpack task failed")??;
	let path = plugin_dir.join(program_name);
	// A second octomind installing at the same moment replaces this file with
	// an identical one, which is harmless.
	program
		.persist(&path)
		.map_err(|error| error.error)
		.with_context(|| format!("Failed to move the program to {}", path.display()))?;
	std::fs::write(plugin_dir.join(VERSION_FILE), &download.version)
		.context("Failed to record the installed version")?;

	let source = url::Url::parse(manifest_url)?;
	Ok(Installed {
		path,
		notice: format!(
			"installed plugin '{}' {} from {}",
			plugin_name,
			download.version,
			source.host_str().unwrap_or(manifest_url)
		),
	})
}

/// Https only, redirects included, so no hop of a download is readable on the way.
fn download_client() -> Result<reqwest::Client> {
	reqwest::Client::builder()
		.https_only(true)
		.connect_timeout(NETWORK_TIMEOUT)
		.read_timeout(NETWORK_TIMEOUT)
		.build()
		.context("Failed to build the HTTP client for the plugin download")
}

async fn fetch(client: &reqwest::Client, url: &str, max_bytes: u64) -> Result<Vec<u8>> {
	let mut body = Vec::new();
	fetch_into(client, url, max_bytes, |chunk| {
		body.extend_from_slice(chunk);
		Ok(())
	})
	.await?;
	Ok(body)
}

/// Hand the body of `url` to `sink` chunk by chunk, failing once it is larger
/// than `max_bytes`.
async fn fetch_into(
	client: &reqwest::Client,
	url: &str,
	max_bytes: u64,
	mut sink: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
	let mut response = client
		.get(url)
		.send()
		.await
		.and_then(reqwest::Response::error_for_status)
		.with_context(|| format!("Failed to download {url}"))?;
	let mut received = 0u64;
	while let Some(chunk) = response
		.chunk()
		.await
		.with_context(|| format!("Failed to download {url}"))?
	{
		received += chunk.len() as u64;
		ensure!(received <= max_bytes, "{} is larger than {} bytes", url, max_bytes);
		sink(&chunk)?;
	}
	Ok(())
}

/// Check the detached signature (standard base64) over the exact manifest bytes.
fn verify_manifest(manifest: &[u8], signature: &[u8], key: &[u8; INSTALL_KEY_BYTES]) -> Result<()> {
	let key = VerifyingKey::from_bytes(key).context("install_key is not a valid Ed25519 public key")?;
	let signature = std::str::from_utf8(signature).context("The manifest signature is not text")?;
	let signature = STANDARD
		.decode(signature.trim())
		.context("The manifest signature is not valid base64")?;
	let signature = Signature::from_slice(&signature).context("The manifest signature has the wrong length")?;
	key.verify_strict(manifest, &signature)
		.map_err(|_| anyhow!("The manifest signature does not match install_key"))
}

/// Read a manifest whose signature was already verified and pick `target`.
fn plan_download(manifest: &[u8], target: &str) -> Result<Download> {
	let manifest: Manifest = serde_json::from_slice(manifest).context("The manifest is not valid")?;
	let entry = manifest
		.targets
		.get(target)
		.with_context(|| format!("Release {} has no build for {}", manifest.version, target))?;
	let url = url::Url::parse(&entry.url).with_context(|| format!("Invalid archive url '{}'", entry.url))?;
	ensure!(
		url.scheme() == "https",
		"Archive url '{}' must start with https://",
		entry.url
	);
	ensure!(
		hex::decode(&entry.sha256).is_ok_and(|bytes| bytes.len() == SHA256_BYTES),
		"The SHA-256 of {} is not 64 hex digits",
		target
	);
	Ok(Download {
		version: manifest.version,
		url: entry.url.clone(),
		sha256: entry.sha256.to_ascii_lowercase(),
	})
}

/// The release target of this machine. Linux always takes the static musl build.
fn this_target() -> Result<&'static str> {
	let os = if cfg!(target_os = "linux") {
		"linux"
	} else if cfg!(target_os = "macos") {
		"macos"
	} else if cfg!(target_os = "windows") {
		"windows"
	} else {
		bail!("There are no plugin releases for this operating system")
	};
	let arm = cfg!(target_arch = "aarch64");
	if !arm && !cfg!(target_arch = "x86_64") {
		bail!("There are no plugin releases for this processor");
	}
	Ok(match (os, arm) {
		("linux", false) => "x86_64-unknown-linux-musl",
		("linux", true) => "aarch64-unknown-linux-musl",
		("macos", false) => "x86_64-apple-darwin",
		("macos", true) => "aarch64-apple-darwin",
		(_, false) => "x86_64-pc-windows-msvc",
		(_, true) => "aarch64-pc-windows-msvc",
	})
}

fn check_sha256(actual: &str, expected: &str) -> Result<()> {
	ensure!(
		actual.eq_ignore_ascii_case(expected),
		"The download does not match the SHA-256 in the manifest (got {}, expected {})",
		actual,
		expected
	);
	Ok(())
}

/// Take the entry named `command` out of `archive` into a new temporary file
/// in `out_dir`, ready to be renamed into place.
fn unpack_program(archive: &Path, command: &str, out_dir: &Path) -> Result<NamedTempFile> {
	let mut program = NamedTempFile::new_in(out_dir).context("Failed to create a temporary file for the program")?;
	#[cfg(windows)]
	extract_zip(archive, command, program.as_file_mut())?;
	#[cfg(not(windows))]
	extract_tar_gz(archive, command, program.as_file_mut())?;
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		program
			.as_file()
			.set_permissions(std::fs::Permissions::from_mode(PROGRAM_MODE))?;
	}
	Ok(program)
}

/// Whether an archive entry is the program. An entry that would land outside
/// the directory (absolute, or with `..`) fails the whole archive.
fn entry_is_program(path: &Path, command: &str) -> Result<bool> {
	let mut names = Vec::new();
	for component in path.components() {
		match component {
			Component::CurDir => {}
			Component::Normal(name) => names.push(name),
			_ => bail!("Archive entry '{}' leaves the plugin directory", path.display()),
		}
	}
	Ok(names.len() == 1 && names[0] == command)
}

fn copy_program(entry: impl Read, out: &mut std::fs::File) -> Result<()> {
	let copied = std::io::copy(&mut entry.take(MAX_PROGRAM_BYTES + 1), out).context("Failed to unpack the program")?;
	ensure!(
		copied <= MAX_PROGRAM_BYTES,
		"The program is larger than {} bytes",
		MAX_PROGRAM_BYTES
	);
	Ok(())
}

#[cfg(not(windows))]
fn extract_tar_gz(archive: &Path, command: &str, out: &mut std::fs::File) -> Result<()> {
	let file = std::fs::File::open(archive).context("Failed to open the download")?;
	let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
	for entry in tar.entries().context("The download is not a tar.gz archive")? {
		let mut entry = entry.context("The download is not a readable tar.gz archive")?;
		let path = entry.path()?.into_owned();
		if !entry_is_program(&path, command)? {
			continue;
		}
		ensure!(
			entry.header().entry_type().is_file(),
			"'{}' in the archive is not a regular file",
			command
		);
		return copy_program(&mut entry, out);
	}
	bail!("The archive has no entry named '{}'", command)
}

#[cfg(windows)]
fn extract_zip(archive: &Path, command: &str, out: &mut std::fs::File) -> Result<()> {
	let file = std::fs::File::open(archive).context("Failed to open the download")?;
	let mut zip = zip::ZipArchive::new(file).context("The download is not a zip archive")?;
	for index in 0..zip.len() {
		let entry = zip
			.by_index(index)
			.context("The download is not a readable zip archive")?;
		if !entry_is_program(Path::new(entry.name()), command)? {
			continue;
		}
		ensure!(entry.is_file(), "'{}' in the archive is not a regular file", command);
		return copy_program(entry, out);
	}
	bail!("The archive has no entry named '{}'", command)
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
