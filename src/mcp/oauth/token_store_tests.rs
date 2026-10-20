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

use super::*;

// Isolated per-process keystore file so tests never touch the real one.
pub(super) fn test_keystore_path() -> PathBuf {
	std::env::temp_dir()
		.join(format!("octomind-keystore-test-{}", std::process::id()))
		.join("keystore.json")
}

#[tokio::test]
async fn save_load_clear_roundtrip() {
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
	assert_eq!(load_token("never-saved-server").await.unwrap(), None);
}

fn now_secs() -> u64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.expect("clock after epoch")
		.as_secs()
}

fn meta_for(server: &str, expires_at: u64) -> TokenMetadata {
	TokenMetadata {
		server_name: server.to_string(),
		access_token: "tok".to_string(),
		refresh_token: None,
		expires_at,
		scopes: Vec::new(),
	}
}

#[tokio::test]
async fn get_valid_token_missing_server_is_none() {
	assert_eq!(get_valid_token("get-valid-missing", 0).await.unwrap(), None);
}

#[tokio::test]
async fn get_valid_token_non_expiring_token_is_always_valid() {
	let server = "get-valid-nonexpiring";
	save_token(server, &meta_for(server, 0)).await.unwrap();
	let loaded = get_valid_token(server, 0).await.unwrap();
	assert_eq!(loaded.as_ref(), Some(&meta_for(server, 0)));
	clear_token(server, false, None, None, None).await.unwrap();
}

#[tokio::test]
async fn get_valid_token_expired_token_is_none() {
	let server = "get-valid-expired";
	save_token(server, &meta_for(server, 1)).await.unwrap();
	assert_eq!(get_valid_token(server, 0).await.unwrap(), None);
	clear_token(server, false, None, None, None).await.unwrap();
}

#[tokio::test]
async fn get_valid_token_buffer_can_expire_a_future_token() {
	let server = "get-valid-buffer";
	let expires_at = now_secs() + 10;
	save_token(server, &meta_for(server, expires_at)).await.unwrap();
	// The refresh buffer pushes the token past its expiry window…
	assert_eq!(get_valid_token(server, 60).await.unwrap(), None);
	// …but without a buffer it is still valid.
	assert!(get_valid_token(server, 0).await.unwrap().is_some());
	clear_token(server, false, None, None, None).await.unwrap();
}

#[tokio::test]
async fn clear_missing_server_is_ok_and_keeps_others() {
	let keeper = "clear-missing-keeper";
	save_token(keeper, &meta_for(keeper, 0)).await.unwrap();
	clear_token("clear-missing-absent", false, None, None, None)
		.await
		.unwrap();
	assert!(load_token(keeper).await.unwrap().is_some());
	clear_token(keeper, false, None, None, None).await.unwrap();
}

#[tokio::test]
async fn saving_a_second_server_keeps_the_first() {
	let first = "multi-first";
	let second = "multi-second";
	save_token(first, &meta_for(first, 0)).await.unwrap();
	save_token(second, &meta_for(second, 0)).await.unwrap();
	assert_eq!(load_token(first).await.unwrap(), Some(meta_for(first, 0)));
	assert_eq!(load_token(second).await.unwrap(), Some(meta_for(second, 0)));
	clear_token(first, false, None, None, None).await.unwrap();
	clear_token(second, false, None, None, None).await.unwrap();
}
