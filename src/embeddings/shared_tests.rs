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

/// The whole point of the service: a second participant gets real vectors
/// without loading weights of its own.
///
/// One `join` elects an owner (loading the model); the next `join` finds the
/// endpoint file, connects, and takes the client role. The client's vectors go
/// over loopback to the owner, so they must match the owner's own embeds
/// exactly and carry the model's tokenizer for chunking.
#[tokio::test]
#[serial_test::serial(embed_model)]
async fn second_participant_becomes_a_client_of_the_owner() {
	let first = join().await.expect("first join elects or attaches");
	let second = join().await.expect("second join attaches to the owner");

	assert!(
		matches!(second.role, Role::Client(_)),
		"a second participant must not load its own copy of the model"
	);
	assert_eq!(
		first.revision, second.revision,
		"both sides must agree on which weights are in play"
	);

	let Role::Client(client) = &second.role else {
		unreachable!("asserted above");
	};
	let texts = vec!["shared embedding service round trip".to_string()];
	let remote = client.embed_many(&texts).await.expect("remote embed");
	assert_eq!(remote.len(), 1);
	assert_eq!(remote[0].len(), super::super::EMBED_DIM);

	// The client's tokenizer came over the wire; it must chunk like the owner's.
	assert_eq!(
		second
			.tokenizer
			.encode("shared model", false)
			.unwrap()
			.len(),
		first.tokenizer.encode("shared model", false).unwrap().len()
	);

	// Same text through the owner's local path must be bit-identical.
	let local = super::super::embed_many(&texts).await.expect("local embed");
	assert_eq!(local[0], remote[0]);
}

/// An owner that died without cleaning up must not strand everyone else: the
/// stale endpoint is dropped and the caller re-elects itself.
#[tokio::test]
#[serial_test::serial(embed_model)]
async fn stale_endpoint_is_replaced() {
	let path = endpoint_path().expect("run dir");
	let dead = Endpoint {
		// Port 1 is privileged and unbound in test environments; connecting
		// fails the same way a dead owner's port does.
		port: 1,
		token: "stale".to_string(),
		model: super::super::MODEL_NAME.to_string(),
		pid: 0,
	};
	let _ = std::fs::remove_file(&path);
	std::fs::write(&path, serde_json::to_string(&dead).unwrap()).unwrap();

	let elected = join()
		.await
		.expect("join must recover from a stale endpoint");
	assert!(
		matches!(elected.role, Role::Owner),
		"the first live process after a dead owner must take ownership"
	);
	let current = read_endpoint(&path).expect("endpoint rewritten");
	assert_ne!(current.token, "stale", "stale claim must be replaced");
	assert_eq!(current.pid, std::process::id());
}
