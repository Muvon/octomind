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
