// Copyright 2025 Muvon Un Limited
//
// Secure OAuth Token Storage

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

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

fn credential_service() -> &'static str {
	"octomind-oauth"
}

// Use server_name as the credential user to support multiple OAuth servers
fn credential_user(server_name: &str) -> String {
	format!("oauth-token-{}", server_name)
}

fn token_storage_dir() -> PathBuf {
	let mut dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("~/.local/share"));
	dir.push("octomind");
	dir.push("tokens");
	dir
}

pub async fn save_token(server_name: &str, metadata: &TokenMetadata) -> Result<()> {
	let json = serde_json::to_string(metadata).map_err(TokenStoreError::SerializationError)?;
	crate::log_debug!(
		"🔍 SAVE_TOKEN: server_name='{}', token_prefix='{}...'",
		server_name,
		metadata.access_token.chars().take(10).collect::<String>()
	);

	// CRITICAL FIX: Save to fallback storage FIRST (more reliable than keyring)
	crate::log_debug!("🔍 SAVE_TOKEN: Saving to encrypted file storage...");
	save_token_fallback(server_name, &json).await?;
	crate::log_debug!("✅ SAVE_TOKEN: Token saved successfully to file storage");

	// Then try keyring as secondary storage (optional, may fail on some systems)
	let entry = keyring::Entry::new(credential_service(), &credential_user(server_name))
		.map_err(|e| anyhow!("Keyring error: {}", e))?;

	match entry.set_password(&json) {
		Ok(_) => {
			// Verify the save immediately
			match entry.get_password() {
				Ok(read_back) if read_back == json => {
					crate::log_debug!("✅ SAVE_TOKEN: Also saved to system keyring");
				}
				Ok(_) => {
					crate::log_debug!(
						"⚠️  SAVE_TOKEN: Keyring save verification failed (data mismatch)"
					);
				}
				Err(_) => {
					crate::log_debug!(
						"⚠️  SAVE_TOKEN: Keyring save verification failed (cannot read back)"
					);
				}
			}
		}
		Err(_) => {
			crate::log_debug!(
				"⚠️  SAVE_TOKEN: Keyring save failed (file storage is primary, this is OK)"
			);
		}
	}

	Ok(())
}

async fn save_token_fallback(server_name: &str, json: &str) -> Result<()> {
	let dir = token_storage_dir();
	crate::log_debug!(
		"🔍 SAVE_TOKEN_FALLBACK: Creating directory: {}",
		dir.display()
	);
	std::fs::create_dir_all(&dir)
		.map_err(|e| anyhow!("Failed to create token directory: {}", e))?;

	let path = dir.join(format!("{}.json", server_name));
	crate::log_debug!(
		"🔍 SAVE_TOKEN_FALLBACK: Encrypting data for server_name='{}'",
		server_name
	);
	let encrypted = encrypt_data(server_name, json)?;

	crate::log_debug!(
		"🔍 SAVE_TOKEN_FALLBACK: Writing to path: {}",
		path.display()
	);
	std::fs::write(&path, encrypted).map_err(|e| anyhow!("Failed to write token file: {}", e))?;
	crate::log_debug!(
		"✅ SAVE_TOKEN_FALLBACK: Successfully saved to: {}",
		path.display()
	);
	crate::log_debug!(
		"SAVE_TOKEN_FALLBACK: server_name='{}', path='{}'",
		server_name,
		path.display()
	);
	Ok(())
}

pub async fn load_token(server_name: &str) -> TokenResult {
	crate::log_debug!(
		"🔍 LOAD_TOKEN: server_name='{}', credential_user='{}'",
		server_name,
		credential_user(server_name)
	);
	crate::log_debug!(
		"LOAD_TOKEN: server_name='{}', credential_user='{}'",
		server_name,
		credential_user(server_name)
	);
	let entry = keyring::Entry::new(credential_service(), &credential_user(server_name))
		.map_err(|e| TokenStoreError::CredentialStoreError(anyhow!(e)))?;

	crate::log_debug!("🔍 LOAD_TOKEN: Attempting to read from keyring...");
	match entry.get_password() {
		Ok(json) => {
			crate::log_debug!(
				"✅ LOAD_TOKEN: Keyring read SUCCESS for server: {}",
				server_name
			);
			crate::log_debug!(
				"LOAD_TOKEN: Keyring read SUCCESS for server: {}",
				server_name
			);
			let metadata: TokenMetadata =
				serde_json::from_str(&json).map_err(TokenStoreError::SerializationError)?;
			crate::log_debug!(
				"✅ LOAD_TOKEN: Token parsed successfully, token_prefix='{}...'",
				metadata.access_token.chars().take(10).collect::<String>()
			);
			Ok(Some(metadata))
		}
		Err(e) => {
			crate::log_debug!(
				"⚠️  LOAD_TOKEN: Keyring read FAILED for server '{}': {:?}, trying fallback...",
				server_name,
				e
			);
			tracing::warn!(
				"LOAD_TOKEN: Keyring read FAILED for server '{}': {:?}, trying fallback...",
				server_name,
				e
			);
			match load_token_fallback(server_name).await {
				Ok(metadata) => {
					crate::log_debug!(
						"✅ LOAD_TOKEN: Fallback read SUCCESS for server: {}",
						server_name
					);
					crate::log_debug!(
						"✅ LOAD_TOKEN: Token parsed from fallback, token_prefix='{}...'",
						metadata.access_token.chars().take(10).collect::<String>()
					);
					crate::log_debug!(
						"LOAD_TOKEN: Fallback read SUCCESS for server: {}",
						server_name
					);
					Ok(Some(metadata))
				}
				Err(e) => {
					crate::log_debug!(
						"❌ LOAD_TOKEN: Fallback read FAILED for server '{}': {:?}",
						server_name,
						e
					);
					tracing::warn!(
						"LOAD_TOKEN: Fallback read FAILED for server '{}': {:?}",
						server_name,
						e
					);
					Ok(None)
				}
			}
		}
	}
}

async fn load_token_fallback(server_name: &str) -> Result<TokenMetadata> {
	let path = token_storage_dir().join(format!("{}.json", server_name));
	crate::log_debug!("🔍 LOAD_TOKEN_FALLBACK: Checking path: {}", path.display());
	if !path.exists() {
		crate::log_debug!(
			"❌ LOAD_TOKEN_FALLBACK: Token file not found at: {}",
			path.display()
		);
		return Err(anyhow!("Token file not found"));
	}

	crate::log_debug!("🔍 LOAD_TOKEN_FALLBACK: Reading encrypted file...");
	let encrypted =
		std::fs::read(&path).map_err(|e| anyhow!("Failed to read token file: {}", e))?;
	crate::log_debug!(
		"🔍 LOAD_TOKEN_FALLBACK: Read {} bytes, decrypting with server_name='{}'",
		encrypted.len(),
		server_name
	);
	// CRITICAL FIX: Pass server_name as the decryption key
	let (_, json) = decrypt_data(server_name, &encrypted)?;

	crate::log_debug!("✅ LOAD_TOKEN_FALLBACK: Decryption successful, parsing JSON...");
	let metadata =
		serde_json::from_str(&json).map_err(|e| anyhow!("Failed to parse token file: {}", e))?;
	crate::log_debug!("✅ LOAD_TOKEN_FALLBACK: Successfully loaded token");
	Ok(metadata)
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

	let entry = keyring::Entry::new(credential_service(), &credential_user(server_name)).ok();
	if let Some(e) = entry {
		let _ = e.delete_credential();
	}

	let path = token_storage_dir().join(format!("{}.json", server_name));
	let _ = std::fs::remove_file(&path);

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

fn encrypt_data(key: &str, data: &str) -> Result<Vec<u8>> {
	let key_bytes = Sha256::digest(key.as_bytes());
	let key_slice: [u8; 32] = key_bytes.as_slice().try_into().unwrap();
	let data_bytes = data.as_bytes();
	let mut encrypted = Vec::with_capacity(data_bytes.len());

	for (i, byte) in data_bytes.iter().enumerate() {
		encrypted.push(byte ^ key_slice[i % 32]);
	}

	let mut result = b"OCTOMIND_TOKEN_V1".to_vec();
	result.push(b':');
	result.extend_from_slice(&encrypted.len().to_le_bytes());
	result.push(b':');
	result.extend(encrypted);
	Ok(result)
}

fn decrypt_data(key: &str, encrypted: &[u8]) -> Result<(String, String)> {
	if !encrypted.starts_with(b"OCTOMIND_TOKEN_V1:") {
		return Err(anyhow!("Invalid token format"));
	}

	// Skip "OCTOMIND_TOKEN_V1:" (18 bytes)
	let rest = &encrypted[18..];
	let len_bytes: [u8; 8] = rest[..8]
		.try_into()
		.map_err(|_| anyhow!("Invalid token format"))?;
	let len = u64::from_le_bytes(len_bytes) as usize;

	// Skip length (8 bytes) + ':' (1 byte) = 9 bytes
	if rest.len() < 9 + len {
		return Err(anyhow!("Invalid token format: truncated data"));
	}

	let data_bytes = &rest[9..9 + len];
	// CRITICAL FIX: Use the same key derivation as encrypt_data
	let key_bytes = Sha256::digest(key.as_bytes());
	let key_slice: [u8; 32] = key_bytes.as_slice().try_into().unwrap();

	let mut decrypted = String::with_capacity(len);

	for (i, byte) in data_bytes.iter().enumerate() {
		decrypted.push((*byte ^ key_slice[i % 32]) as char);
	}

	let metadata: TokenMetadata = serde_json::from_str(&decrypted)
		.map_err(|e| anyhow!("Failed to parse decrypted token: {}", e))?;

	Ok((metadata.access_token.clone(), decrypted))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_encrypt_decrypt() {
		let key = "test-key";
		let data =
			r#"{"server_name":"test","access_token":"abc","expires_at":1234567890,"scopes":[]}"#;

		let encrypted = encrypt_data(key, data).unwrap();
		// CRITICAL FIX: Pass the same key used for encryption
		let (token, decrypted) = decrypt_data(key, &encrypted).unwrap();
		assert_eq!(token, "abc");
		assert_eq!(decrypted, data);
	}
}
