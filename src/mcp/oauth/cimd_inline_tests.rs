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

#[test]
fn test_build_client_metadata() {
	let metadata = build_client_metadata(
		"http://localhost:34567/oauth/callback",
		&["read".to_string(), "write".to_string()],
	);
	assert_eq!(metadata.client_name, "Octomind");
	assert_eq!(
		metadata.redirect_uris,
		vec!["http://localhost:34567/oauth/callback"]
	);
	assert_eq!(metadata.grant_types, vec!["authorization_code"]);
	assert_eq!(metadata.token_endpoint_auth_method, "none");
	assert_eq!(metadata.scope, Some("read write".to_string()));
}

#[test]
fn test_build_client_metadata_empty_scopes() {
	let metadata = build_client_metadata("http://localhost:34567/oauth/callback", &[]);
	assert!(metadata.scope.is_none());
}

#[test]
fn test_client_metadata_serialization() {
	let metadata = build_client_metadata(
		"http://localhost:34567/oauth/callback",
		&["openid".to_string()],
	);
	let json = serde_json::to_string(&metadata).unwrap();
	assert!(json.contains("Octomind"));
	assert!(json.contains("authorization_code"));
	assert!(json.contains("none"));
}
