// Copyright 2026 Muvon Un Limited
//
// Secure OAuth Token Storage
//
//! OAuth bearer-token storage for remote MCP servers (RFC 9728 discovery → PKCE
//! flow). Tokens live in a single plain JSON file (`<data_dir>/keystore.json`,
//! mode 0600) replaced atomically via temp-file + rename: cross-platform, works
//! headless (no system keychain / dbus / Secret Service), and — unlike SQLite
//! in WAL mode — safe on an NFS-mounted home. Tokens are keyed per MCP server
//! name. Cross-process writes are last-writer-wins: the rename keeps the file
//! always valid, and a lost update at worst forces one extra re-auth.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenMetadata {
	pub server_name: String,
	pub access_token: String,
	#[serde(default)]
	pub refresh_token: Option<String>,
	pub expires_at: u64,
	#[serde(default)]
	pub scopes: Vec<String>,
}

pub type TokenResult = Result<Option<TokenMetadata>, TokenStoreError>;

#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
	#[error("Token not found for server: {0}")]
	NotFound(String),
	#[error("Failed to access credential store: {0}")]
	CredentialStoreError(#[from] anyhow::Error),
	#[error("Token is expired")]
	Expired,
	#[error("Token serialization failed: {0}")]
	SerializationError(#[from] serde_json::Error),
}

/// Serializes in-process read-modify-write cycles on the keystore file.
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// Plain JSON keystore file: `<data_dir>/keystore.json`, map of server name →
/// [`TokenMetadata`], mode 0600.
fn keystore_path() -> Result<PathBuf> {
	#[cfg(test)]
	{
		Ok(tests::test_keystore_path())
	}
	#[cfg(not(test))]
	{
		Ok(crate::directories::get_octomind_data_dir()?.join("keystore.json"))
	}
}

fn read_all(path: &Path) -> Result<HashMap<String, TokenMetadata>, TokenStoreError> {
	match std::fs::read_to_string(path) {
		// A corrupt file surfaces as SerializationError — it must not silently
		// degrade into an endless re-auth loop.
		Ok(json) => Ok(serde_json::from_str(&json)?),
		// No file yet — not an error, just no tokens stored.
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
		Err(e) => Err(TokenStoreError::CredentialStoreError(anyhow!(
			"failed to read keystore {}: {e}",
			path.display()
		))),
	}
}

/// Atomically replace the keystore: write a private (0600) temp file in the
/// same directory, then rename over the target. Rename is atomic on POSIX and
/// on NFS, so readers never observe a partial file.
fn write_all(path: &Path, tokens: &HashMap<String, TokenMetadata>) -> Result<()> {
	let json = serde_json::to_string_pretty(tokens)?;
	let dir = path.parent().context("keystore path has no parent")?;
	std::fs::create_dir_all(dir)?;
	let tmp = dir.join(format!(".keystore.{}.tmp", std::process::id()));

	let mut opts = std::fs::OpenOptions::new();
	opts.write(true).create_new(true);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt;
		opts.mode(0o600);
	}
	// create_new never follows a pre-existing symlink; a leftover temp file
	// from a crashed process is removed first.
	let _ = std::fs::remove_file(&tmp);
	let mut file = opts
		.open(&tmp)
		.with_context(|| format!("failed to create {}", tmp.display()))?;
	file.write_all(json.as_bytes())?;
	file.sync_all()?;
	drop(file);
	std::fs::rename(&tmp, path)
		.with_context(|| format!("failed to replace keystore {}", path.display()))?;
	Ok(())
}

pub async fn save_token(server_name: &str, metadata: &TokenMetadata) -> Result<()> {
	crate::log_debug!(
		"🔍 SAVE_TOKEN: server_name='{}', token_prefix='{}...'",
		server_name,
		metadata.access_token.chars().take(10).collect::<String>()
	);
	let path = keystore_path()?;
	let _guard = FILE_LOCK.lock().unwrap();
	let mut tokens = read_all(&path)?;
	tokens.insert(server_name.to_string(), metadata.clone());
	write_all(&path, &tokens)?;
	crate::log_debug!("✅ SAVE_TOKEN: stored token for server '{}'", server_name);
	Ok(())
}

pub async fn load_token(server_name: &str) -> TokenResult {
	let path = keystore_path().map_err(TokenStoreError::CredentialStoreError)?;
	match read_all(&path)?.get(server_name).cloned() {
		Some(metadata) => {
			crate::log_debug!(
				"✅ LOAD_TOKEN: loaded token for '{}', token_prefix='{}...'",
				server_name,
				metadata.access_token.chars().take(10).collect::<String>()
			);
			Ok(Some(metadata))
		}
		None => {
			crate::log_debug!("LOAD_TOKEN: no token stored for server '{}'", server_name);
			Ok(None)
		}
	}
}

pub async fn clear_token(
	server_name: &str,
	revoke: bool,
	token_url: Option<&str>,
	client_id: Option<&str>,
	client_secret: Option<&str>,
) -> Result<()> {
	if revoke {
		if let (Some(url), Some(cid), Some(secret)) = (token_url, client_id, client_secret) {
			let _ = revoke_token(url, cid, secret, server_name).await;
		}
	}

	let path = keystore_path()?;
	let _guard = FILE_LOCK.lock().unwrap();
	let mut tokens = read_all(&path)?;
	if tokens.remove(server_name).is_some() {
		write_all(&path, &tokens)?;
	}

	tracing::debug!("Cleared OAuth token for server: {}", server_name);
	Ok(())
}

pub async fn get_valid_token(server_name: &str, buffer_seconds: u64) -> TokenResult {
	crate::log_debug!(
		"GET_VALID_TOKEN: server_name='{}', buffer_seconds={}",
		server_name,
		buffer_seconds
	);
	let metadata = match load_token(server_name).await? {
		Some(m) => m,
		None => {
			crate::log_debug!("No token found for server_name='{}'", server_name);
			return Ok(None);
		}
	};

	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_secs())
		.unwrap_or(0);

	// Token is invalid only if it has an expiration AND is expired
	// Non-expiring tokens (expires_at == 0) like GitHub tokens are always valid
	if metadata.expires_at > 0 && now + buffer_seconds >= metadata.expires_at {
		return Ok(None);
	}
	Ok(Some(metadata))
}

// Helper function to build form-encoded body
fn build_form_body(params: &[(&str, &str)]) -> String {
	params
		.iter()
		.map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
		.collect::<Vec<_>>()
		.join("&")
}

async fn revoke_token(
	token_url: &str,
	client_id: &str,
	client_secret: &str,
	token: &str,
) -> Result<()> {
	let client = reqwest::Client::new();

	let params = [
		("token", token),
		("client_id", client_id),
		("client_secret", client_secret),
	];

	let body = build_form_body(&params);

	let response = client
		.post(token_url)
		.header(
			reqwest::header::CONTENT_TYPE,
			"application/x-www-form-urlencoded",
		)
		.body(body)
		.send()
		.await;

	match response {
		Ok(r) if r.status().is_success() => {
			tracing::debug!("Successfully revoked token");
			Ok(())
		}
		Ok(r) => {
			tracing::warn!("Token revocation returned status: {}", r.status());
			Ok(())
		}
		Err(e) => {
			tracing::warn!("Failed to revoke token: {}", e);
			Ok(())
		}
	}
}

#[cfg(test)]
#[path = "token_store_tests.rs"]
mod tests;
