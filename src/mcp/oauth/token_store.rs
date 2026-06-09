// Copyright 2026 Muvon Un Limited
//
// Secure OAuth Token Storage
//
//! OAuth bearer-token storage for remote MCP servers (RFC 9728 discovery → PKCE
//! flow). Backed by a single db-keystore SQLite store via the keyring-core API:
//! cross-platform and works headless (no system keychain / dbus / Secret Service),
//! which is the dominant environment for token *reuse* and *refresh* (ACP under an
//! IDE, websocket server, CI). Tokens are keyed per MCP server name.

use anyhow::{anyhow, Result};
use keyring_core::Entry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

const CREDENTIAL_SERVICE: &str = "octomind-oauth";

// Use server_name as the credential user to support multiple OAuth servers
fn credential_user(server_name: &str) -> String {
	format!("oauth-token-{}", server_name)
}

/// SQLite keystore file: `<data_dir>/octomind/keystore.db`.
fn keystore_path() -> PathBuf {
	let mut dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("~/.local/share"));
	dir.push("octomind");
	dir.push("keystore.db");
	dir
}

/// Register the db-keystore SQLite store as the process-wide credential store,
/// exactly once. A store registered earlier (e.g. by tests) takes precedence and
/// short-circuits this. The registration result is cached, so a failure surfaces
/// on every call rather than being silently retried.
fn ensure_store() -> Result<()> {
	// Honor a pre-registered store (tests inject an isolated one).
	if keyring_core::get_default_store().is_some() {
		return Ok(());
	}

	static INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
	INIT.get_or_init(|| {
		let path = keystore_path();
		let path_str = path.to_string_lossy().into_owned();
		let modifiers = HashMap::from([("path", path_str.as_str())]);
		let store = db_keystore::DbKeyStore::new_with_modifiers(&modifiers)
			.map_err(|e| format!("failed to open keystore at {}: {e}", path.display()))?;
		keyring_core::set_default_store(store);
		// Restrict the keystore directory so other users can't read the db file.
		harden_dir(path.parent());
		Ok(())
	})
	.clone()
	.map_err(|e| anyhow!(e))
}

#[cfg(unix)]
fn harden_dir(dir: Option<&Path>) {
	use std::os::unix::fs::PermissionsExt;
	if let Some(dir) = dir {
		let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
	}
}
#[cfg(not(unix))]
fn harden_dir(_dir: Option<&Path>) {}

/// Build a credential entry for a server, ensuring the store is registered.
fn entry(server_name: &str) -> Result<Entry> {
	ensure_store()?;
	Entry::new(CREDENTIAL_SERVICE, &credential_user(server_name))
		.map_err(|e| anyhow!("keyring entry error: {e}"))
}

pub async fn save_token(server_name: &str, metadata: &TokenMetadata) -> Result<()> {
	let json = serde_json::to_string(metadata).map_err(TokenStoreError::SerializationError)?;
	crate::log_debug!(
		"🔍 SAVE_TOKEN: server_name='{}', token_prefix='{}...'",
		server_name,
		metadata.access_token.chars().take(10).collect::<String>()
	);
	entry(server_name)?
		.set_password(&json)
		.map_err(|e| anyhow!("failed to save token: {e}"))?;
	crate::log_debug!("✅ SAVE_TOKEN: stored token for server '{}'", server_name);
	Ok(())
}

pub async fn load_token(server_name: &str) -> TokenResult {
	crate::log_debug!(
		"🔍 LOAD_TOKEN: server_name='{}', credential_user='{}'",
		server_name,
		credential_user(server_name)
	);
	match entry(server_name)?.get_password() {
		Ok(json) => {
			let metadata: TokenMetadata = serde_json::from_str(&json)?;
			crate::log_debug!(
				"✅ LOAD_TOKEN: loaded token, token_prefix='{}...'",
				metadata.access_token.chars().take(10).collect::<String>()
			);
			Ok(Some(metadata))
		}
		// No stored credential for this server — not an error, just unauthenticated.
		Err(keyring_core::Error::NoEntry) => {
			crate::log_debug!("LOAD_TOKEN: no token stored for server '{}'", server_name);
			Ok(None)
		}
		// A genuine store failure (corruption, no access) must surface, not silently
		// degrade into an endless re-auth loop.
		Err(e) => Err(TokenStoreError::CredentialStoreError(anyhow!(
			"failed to load token for '{server_name}': {e}"
		))),
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

	match entry(server_name)?.delete_credential() {
		Ok(()) | Err(keyring_core::Error::NoEntry) => {}
		Err(e) => crate::log_debug!("clear_token: delete failed for '{}': {}", server_name, e),
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
mod tests {
	use super::*;
	use std::sync::Once;

	// Register an isolated temp-file keystore as the process-wide store, once.
	// File-backed (not :memory:) so it survives across separate store operations.
	static TEST_STORE: Once = Once::new();
	fn init_test_store() {
		TEST_STORE.call_once(|| {
			let path = std::env::temp_dir()
				.join(format!("octomind-keystore-test-{}", std::process::id()))
				.join("keystore.db");
			let path_str = path.to_string_lossy().into_owned();
			let modifiers = HashMap::from([("path", path_str.as_str())]);
			let store = db_keystore::DbKeyStore::new_with_modifiers(&modifiers).unwrap();
			keyring_core::set_default_store(store);
		});
	}

	#[tokio::test]
	async fn save_load_clear_roundtrip() {
		init_test_store();
		let server = "test-roundtrip";
		let meta = TokenMetadata {
			server_name: server.to_string(),
			access_token: "abc123".to_string(),
			refresh_token: Some("refresh".to_string()),
			expires_at: 0,
			scopes: vec!["read".to_string()],
		};

		save_token(server, &meta).await.unwrap();
		assert_eq!(load_token(server).await.unwrap(), Some(meta));

		clear_token(server, false, None, None, None).await.unwrap();
		assert_eq!(load_token(server).await.unwrap(), None);
	}

	#[tokio::test]
	async fn load_missing_is_none() {
		init_test_store();
		assert_eq!(load_token("never-saved-server").await.unwrap(), None);
	}
}
