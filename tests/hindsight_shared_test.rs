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

use anyhow::{Context, Result};
use octolib::hindsight::trace::Event;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn main() -> Result<()> {
	let args: Vec<String> = std::env::args().skip(1).collect();
	let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hindsight-shared");
	if args.first().is_some_and(|arg| arg == "hindsight-worker") {
		let model = args.get(2).context("worker model missing")?;
		return tokio::runtime::Runtime::new()?.block_on(octomind::supervisor::hindsight::shared::serve(model));
	}
	if args.first().is_some_and(|arg| arg == "client") {
		let model = args.get(1).context("client model missing")?;
		return tokio::runtime::Runtime::new()?.block_on(async {
			let (scores, count) =
				octomind::supervisor::hindsight::shared::score(model, "shared-session", events()).await?;
			anyhow::ensure!(
				count == 2 && (scores.p_correction - 0.5).abs() < 1e-6,
				"wrong shared inference result"
			);
			Ok(())
		});
	}
	let directory = tempfile::tempdir()?;
	let runtime = directory.path().join("runtime");
	std::fs::create_dir(&runtime)?;
	for name in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
		std::env::set_var(name, &runtime);
	}
	std::env::set_var("OCTOMIND_DATA_DIR", directory.path().join("data"));
	tokio::runtime::Runtime::new()?
		.block_on(async { tokio::time::timeout(Duration::from_secs(90), exercise(&fixture)).await })??;
	Ok(())
}

fn events() -> Vec<Event> {
	vec![Event::user_turn("hello"), Event::assistant("done")]
}

fn endpoint() -> Result<(PathBuf, Value)> {
	let paths: Vec<_> = std::fs::read_dir(octomind::directories::get_run_dir()?)?
		.collect::<std::io::Result<Vec<_>>>()?
		.into_iter()
		.filter(|entry| {
			entry.file_name().to_string_lossy().starts_with("hindsight-")
				&& entry.path().extension().is_some_and(|ext| ext == "json")
		})
		.map(|entry| entry.path())
		.collect();
	anyhow::ensure!(
		paths.len() == 1,
		"expected exactly one published owner, got {}",
		paths.len()
	);
	Ok((paths[0].clone(), serde_json::from_slice(&std::fs::read(&paths[0])?)?))
}

async fn rpc(endpoint: &Value, request: Value, token: &str) -> Result<Value> {
	let port = endpoint["port"].as_u64().context("port missing")?;
	let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await?;
	let mut reader = BufReader::new(stream);
	let body = json!({"token":token,"request":request}).to_string() + "\n";
	reader.get_mut().write_all(body.as_bytes()).await?;
	let mut reply = String::new();
	anyhow::ensure!(reader.read_line(&mut reply).await? > 0, "owner closed the connection");
	Ok(serde_json::from_str(&reply)?)
}

async fn stop_owner(path: &Path, endpoint: &Value) -> Result<()> {
	let token = endpoint["token"].as_str().context("token missing")?;
	let response = rpc(endpoint, json!({"op":"shutdown"}), token).await?;
	anyhow::ensure!(response["Ok"]["op"] == "stopping", "shutdown was rejected");
	let path = path.with_extension("owner");
	tokio::task::spawn_blocking(move || -> Result<()> {
		let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
		file.lock()?;
		Ok(())
	})
	.await??;
	Ok(())
}

async fn exercise(fixture: &Path) -> Result<()> {
	// Three independent client processes race a cold start. None is the model owner.
	let executable = std::env::current_exe()?;
	let mut clients = Vec::new();
	let mut client_pids = Vec::new();
	for _ in 0..3 {
		let child = tokio::process::Command::new(&executable)
			.arg("client")
			.arg(fixture)
			.kill_on_drop(true)
			.spawn()?;
		client_pids.push(child.id().context("client pid missing")?);
		clients.push(child);
	}
	for mut child in clients {
		anyhow::ensure!(child.wait().await?.success(), "sharing client failed");
	}
	let (path, first) = endpoint()?;
	let first_pid = first["pid"].as_u64().context("owner pid missing")?;
	anyhow::ensure!(
		!client_pids.iter().any(|pid| u64::from(*pid) == first_pid),
		"client loaded the model itself"
	);
	let spec = fixture.to_str().context("fixture path is not UTF-8")?;
	let (scores, count) = octomind::supervisor::hindsight::shared::score(spec, "shared-session", events()).await?;
	anyhow::ensure!(count == 2 && (scores.p_correction - 0.5).abs() < 1e-6, "wrong score");
	anyhow::ensure!(
		endpoint()?.1["pid"] == first["pid"],
		"owner did not survive client exit"
	);
	let token = first["token"].as_str().context("token missing")?;
	anyhow::ensure!(
		rpc(&first, json!({"op":"ping"}), "wrong-token").await.is_err(),
		"unauthenticated request accepted"
	);
	let rejected = rpc(&first, json!({"op":"score","session":"bad","events":[]}), token).await?;
	anyhow::ensure!(rejected.get("Err").is_some(), "invalid event count accepted");
	// Repeated input and rewritten history must both be safe.
	for input in [events(), vec![Event::user_turn("rewritten")], events()] {
		octomind::supervisor::hindsight::shared::score(spec, "shared-session", input).await?;
	}
	anyhow::ensure!(endpoint()?.1["pid"] == first["pid"], "bad clients killed the worker");
	stop_owner(&path, &first).await?;
	octomind::supervisor::hindsight::shared::score(spec, "after-restart", events()).await?;
	let (path, second) = endpoint()?;
	anyhow::ensure!(second["token"] != first["token"], "stale owner was reused");
	stop_owner(&path, &second).await?;
	println!("shared Hindsight: process ownership, client exit, input validation, and restart checks passed");
	Ok(())
}
