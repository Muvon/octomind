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

use super::{ai::PreparedDecision, schema::CompressionSummary, FoldContext, FoldJob};
use crate::config::Config;
use crate::providers::TokenUsage;
use crate::session::chat::session::ChatSession;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const COMPACT_READY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize)]
struct FoldRequest {
	prepared: PreparedDecision,
	ctx: FoldContext,
	session_name: String,
}

#[derive(Serialize, Deserialize)]
struct FoldResult {
	summary: CompressionSummary,
	usage: Option<TokenUsage>,
}

/// Hand an unfinished one-shot fold to a child so a later resume can collect it.
pub(crate) async fn detach_fold_before_exit<F, Fut>(
	session: &mut ChatSession,
	config: &Config,
	spawn: F,
) -> Result<bool>
where
	F: FnOnce(PathBuf, PathBuf) -> Fut,
	Fut: std::future::Future<Output = Result<()>>,
{
	let Some(job) = session.fold_job.take() else {
		return Ok(false);
	};
	if job.handle.is_finished() {
		return super::collect_fold_job(session, config, job, false, false).await;
	}
	if job.ctx.detached {
		return Ok(false);
	}
	let Some(session_file) = session.session.session_file.clone() else {
		return Ok(false);
	};
	let sidecar = session_file.with_added_extension("fold.json");
	let FoldJob {
		ctx,
		prepared,
		cancel: _cancel,
		..
	} = job;
	let request = FoldRequest {
		prepared,
		ctx,
		session_name: session.session.info.name.clone(),
	};
	let handoff = async {
		octolib::utils::config_file::atomic_write(&sidecar, &serde_json::to_vec(&request)?, None)?;
		spawn(sidecar.clone(), session_file).await
	}
	.await;
	if let Err(error) = handoff {
		crate::log_error!("Detached fold handoff failed: {:#}", error);
		super::note_fold_failure(session);
		match std::fs::remove_file(&sidecar) {
			Ok(()) => {}
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
			Err(error) => return Err(error).context("failed to remove pending fold request"),
		}
	}
	Ok(false)
}

pub(crate) async fn spawn_compact_process(sidecar: PathBuf, session_file: PathBuf) -> Result<()> {
	let mut child = tokio::process::Command::new(std::env::current_exe()?)
		.arg("compact")
		.arg("--fold")
		.arg(sidecar)
		.arg("--session-file")
		.arg(session_file)
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.context("failed to spawn detached compact child")?;
	let mut stdout = child.stdout.take().expect("compact stdout is piped");
	let mut ready = [0; b"ready\n".len()];
	let handshake = async {
		tokio::time::timeout(COMPACT_READY_TIMEOUT, stdout.read_exact(&mut ready))
			.await
			.context("detached compact ready handshake timed out")?
			.context("detached compact exited before ready")?;
		anyhow::ensure!(&ready == b"ready\n", "invalid detached compact ready handshake");
		Ok(())
	}
	.await;
	if let Err(error) = handshake {
		child
			.start_kill()
			.context("failed to stop compact child after failed handshake")?;
		return Err(error);
	}
	Ok(())
}

pub(crate) fn restore_fold(session_file: &Path) -> Result<Option<FoldJob>> {
	let sidecar = session_file.with_added_extension("fold.json");
	let raw = match std::fs::read(&sidecar) {
		Ok(raw) => raw,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
		Err(error) => return Err(error).with_context(|| format!("failed to read {}", sidecar.display())),
	};
	let mut request: FoldRequest = serde_json::from_slice(&raw).context("invalid pending fold request")?;
	request.ctx.detached = true;
	let lock_path = sidecar.with_added_extension("lock");
	let result_path = sidecar.with_added_extension("result.json");
	// A standard thread can outlive a dropped job without delaying runtime shutdown.
	let (lock_tx, lock_rx) = tokio::sync::oneshot::channel();
	std::thread::spawn(move || {
		let locked = (|| -> Result<std::fs::File> {
			let lock = std::fs::OpenOptions::new()
				.read(true)
				.write(true)
				.open(&lock_path)
				.context("failed to open detached fold lock")?;
			lock.lock().context("failed to lock detached fold")?;
			Ok(lock)
		})();
		// A dropped job no longer needs the file; the unsent lock is released on drop.
		let _ = lock_tx.send(locked);
	});
	let (cancel, mut cancel_rx) = tokio::sync::watch::channel(false);
	let handle = tokio::spawn(async move {
		let lock = tokio::select! {
			biased;
			_ = cancel_rx.changed() => return Err(crate::session::cancellation::Cancelled.into()),
			locked = lock_rx => locked.context("detached fold lock thread failed")??,
		};
		let outcome = match std::fs::read(&result_path) {
			Ok(raw) => serde_json::from_slice::<FoldResult>(&raw).context("invalid detached fold result"),
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
				Err(anyhow::anyhow!("detached fold produced no result"))
			}
			Err(error) => Err(error).context("failed to read detached fold result"),
		};
		// Windows requires closing the lock file before removing it.
		drop(lock);
		for path in [&sidecar, &result_path, &sidecar.with_added_extension("lock")] {
			match std::fs::remove_file(path) {
				Ok(()) => {}
				Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
				Err(error) => return Err(error).with_context(|| format!("failed to remove {}", path.display())),
			}
		}
		let result = outcome?;
		Ok((result.summary, result.usage))
	});
	Ok(Some(FoldJob {
		handle,
		ctx: request.ctx,
		prepared: request.prepared,
		cancel,
	}))
}

pub async fn run_detached_fold(fold: &Path, session_file: &Path, config: &Config) -> Result<()> {
	let lock = std::fs::OpenOptions::new()
		.read(true)
		.write(true)
		.create(true)
		.truncate(false)
		.open(fold.with_added_extension("lock"))?;
	lock.lock().context("failed to lock pending fold")?;
	crate::println!("ready");
	std::io::stdout().flush()?;

	let request: FoldRequest = serde_json::from_slice(&std::fs::read(fold)?)
		.with_context(|| format!("invalid pending fold request {}", fold.display()))?;
	crate::config::set_thread_config(config);
	let (_cancel, cancel_rx) = tokio::sync::watch::channel(false);
	let (summary, usage) =
		crate::supervisor::accounting::run_background(request.session_name, Some(session_file.to_path_buf()), async {
			let (summary, usage) = super::ai::run_decision_call(
				config,
				request.prepared.system_content,
				request.prepared.user_content,
				request.prepared.schema,
				cancel_rx,
			)
			.await?;
			if let Some(usage) = &usage {
				crate::supervisor::stats::record_call(
					crate::supervisor::stats::CallKind::Compression,
					usage.input_tokens,
					usage.output_tokens,
					usage.reasoning_tokens,
					usage.request_time_ms.unwrap_or(0),
					usage.cost.unwrap_or(0.0),
				);
				crate::supervisor::accounting::checkpoint()?;
			}
			Ok((summary, usage))
		})
		.await?;
	octolib::utils::config_file::atomic_write(
		&fold.with_added_extension("result.json"),
		&serde_json::to_vec(&FoldResult { summary, usage })?,
		None,
	)?;
	drop(lock);
	Ok(())
}

#[cfg(test)]
#[path = "detached_tests.rs"]
mod tests;
