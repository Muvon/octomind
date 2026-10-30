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
use tokio::io::AsyncBufReadExt;

#[tokio::test]
#[serial_test::serial(embed_model)]
async fn simultaneous_joins_elect_one_owner() {
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join("embed.json");
	let (first, second) = tokio::join!(join_at(&path), join_at(&path));
	let first = first.unwrap();
	let second = second.unwrap();
	assert_ne!(
		matches!(first.role, Role::Owner(_)),
		matches!(second.role, Role::Owner(_))
	);
	assert_eq!(first.revision, second.revision);
}

#[tokio::test]
#[serial_test::serial(embed_model)]
async fn a_client_can_rejoin_after_its_owner_exits() {
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join("embed.json");
	let owner = join_at(&path).await.unwrap();
	let client = join_at(&path).await.unwrap();
	let Role::Owner(task) = owner.role else {
		panic!("first participant must own the model")
	};
	task.abort();
	assert!(task.await.unwrap_err().is_cancelled());
	let Role::Client(client) = client.role else {
		panic!("second participant must be a client")
	};
	let error = client.embed_many(&["owner recovery".into()]).await.unwrap_err();
	assert!(is_disconnected(&error));
	let replacement = join_at(&path).await.unwrap();
	assert!(matches!(replacement.role, Role::Owner(_)));
	let rejoined = join_at(&path).await.unwrap();
	let Role::Client(client) = rejoined.role else {
		panic!("rejoined participant must be a client")
	};
	let vectors = client.embed_many(&["owner recovery".into()]).await.unwrap();
	assert_eq!(vectors[0].len(), super::super::EMBED_DIM);
}

#[test]
fn embedding_client_process_fixture() {
	let Some(path) = std::env::var_os("OCTOMIND_EMBED_TEST_ENDPOINT") else {
		return;
	};
	tokio::runtime::Runtime::new().unwrap().block_on(async {
		assert!(super::super::MODEL.get().is_none());
		let elected = join_at(Path::new(&path)).await.unwrap();
		let Role::Client(client) = elected.role else {
			panic!("child must use the parent's model")
		};
		let texts = vec!["ACP specialist model sharing".to_string(); MAX_BATCH_TEXTS + 1];
		let vectors = client.embed_many(&texts).await.unwrap();
		assert_eq!(vectors.len(), texts.len());
		assert_eq!(vectors[0].len(), super::super::EMBED_DIM);
		assert!(vectors.iter().all(|vector| vector == &vectors[0]));
		assert!(
			super::super::MODEL.get().is_none(),
			"client must never load local weights"
		);
	});
}

#[tokio::test]
#[serial_test::serial(embed_model)]
async fn another_process_embeds_without_loading_a_model() {
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join("embed.json");
	let owner = join_at(&path).await.unwrap();
	assert!(matches!(owner.role, Role::Owner(_)));
	let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
	command.args([
		"--exact",
		"embeddings::shared::tests::embedding_client_process_fixture",
		"--nocapture",
	]);
	command.env("OCTOMIND_EMBED_TEST_ENDPOINT", &path).kill_on_drop(true);
	let output = tokio::time::timeout(SERVICE_TIMEOUT, command.output())
		.await
		.unwrap()
		.unwrap();
	assert!(
		output.status.success(),
		"child failed: {} {}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
}

#[test]
fn embedding_recovery_process_fixture() {
	if std::env::var_os("OCTOMIND_EMBED_RECOVERY_FIXTURE").is_none() {
		return;
	}
	tokio::runtime::Runtime::new().unwrap().block_on(async {
		let vector = super::super::embed("before owner exits").await.unwrap();
		assert_eq!(vector.len(), super::super::EMBED_DIM);
		assert!(super::super::MODEL.get().is_none());
		println!("EMBED_READY");
		let mut line = String::new();
		BufReader::new(tokio::io::stdin()).read_line(&mut line).await.unwrap();
		assert_eq!(line, "owner stopped\n");
		let vector = super::super::embed(&format!("after owner exits {}", uuid::Uuid::new_v4()))
			.await
			.unwrap();
		assert_eq!(vector.len(), super::super::EMBED_DIM);
		assert!(
			super::super::MODEL.get().is_some(),
			"surviving client must become the new owner"
		);
	});
}

#[tokio::test]
#[serial_test::serial(embed_model)]
async fn a_surviving_process_recovers_automatically_after_owner_exit() {
	let directory = tempfile::tempdir().unwrap();
	let run_dir = directory.path().join("octomind");
	std::fs::create_dir(&run_dir).unwrap();
	let owner = join_at(&run_dir.join("embed.json")).await.unwrap();
	let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
	command.args([
		"--exact",
		"embeddings::shared::tests::embedding_recovery_process_fixture",
		"--nocapture",
	]);
	command
		.env("OCTOMIND_EMBED_RECOVERY_FIXTURE", "1")
		.env("XDG_RUNTIME_DIR", directory.path())
		.env("TMPDIR", directory.path())
		.env("TEMP", directory.path())
		.env("TMP", directory.path())
		.env("OCTOMIND_DATA_DIR", directory.path().join("data"))
		.stdin(std::process::Stdio::piped())
		.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::piped())
		.kill_on_drop(true);
	let mut child = command.spawn().unwrap();
	tokio::time::timeout(SERVICE_TIMEOUT, async {
		let mut output = BufReader::new(child.stdout.take().unwrap());
		loop {
			let mut line = String::new();
			assert!(
				output.read_line(&mut line).await.unwrap() > 0,
				"client exited before initializing"
			);
			if line.contains("EMBED_READY") {
				break;
			}
		}
		let Role::Owner(task) = owner.role else {
			panic!("parent must own the model")
		};
		task.abort();
		assert!(task.await.unwrap_err().is_cancelled());
		child.stdin.take().unwrap().write_all(b"owner stopped\n").await.unwrap();
		let result = child.wait_with_output().await.unwrap();
		assert!(
			result.status.success(),
			"client recovery failed: {}",
			String::from_utf8_lossy(&result.stderr)
		);
	})
	.await
	.unwrap();
}

#[tokio::test]
async fn service_rejects_oversized_lines() {
	let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
	let addr = listener.local_addr().unwrap();
	let reader = tokio::spawn(async move {
		let (stream, _) = listener.accept().await.unwrap();
		read_line(&mut BufReader::new(stream)).await.unwrap_err()
	});
	let mut stream = TcpStream::connect(addr).await.unwrap();
	stream.write_all(&vec![b'x'; MAX_SERVICE_LINE_BYTES + 1]).await.unwrap();
	assert!(reader.await.unwrap().to_string().contains("exceeds byte limit"));
}

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
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join("embed.json");
	let first = join_at(&path).await.expect("first join elects or attaches");
	let second = join_at(&path).await.expect("second join attaches to the owner");

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
		second.tokenizer.encode("shared model", false).unwrap().len(),
		first.tokenizer.encode("shared model", false).unwrap().len()
	);

	// Same text through the owner's local path must be bit-identical.
	let local = super::super::embed_local(&texts).await.expect("local embed");
	assert_eq!(local[0], remote[0]);
}

/// An owner that died without cleaning up must not strand everyone else: the
/// stale endpoint is dropped and the caller re-elects itself.
#[tokio::test]
#[serial_test::serial(embed_model)]
async fn stale_endpoint_is_replaced() {
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join("embed.json");
	let dead = Endpoint {
		port: {
			let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
			listener.local_addr().unwrap().port()
		},
		token: "stale".to_string(),
		model: super::super::MODEL_NAME.to_string(),
		pid: 0,
	};
	std::fs::write(&path, serde_json::to_string(&dead).unwrap()).unwrap();

	let elected = join_at(&path).await.expect("join must recover from a stale endpoint");
	assert!(
		matches!(elected.role, Role::Owner(_)),
		"the first live process after a dead owner must take ownership"
	);
	let current: Endpoint = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
	assert_ne!(current.token, "stale", "stale claim must be replaced");
	assert_eq!(current.pid, std::process::id());
}
