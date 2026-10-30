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

//! Bounded local inference IPC and atomic owner publication.
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::File;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub(crate) const MAX_SERVICE_LINE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) async fn election_lock(path: &Path) -> Result<File> {
	let path = path.with_extension("lock");
	tokio::task::spawn_blocking(move || {
		let mut options = std::fs::OpenOptions::new();
		options.read(true).write(true).create(true).truncate(false);
		#[cfg(unix)]
		{
			use std::os::unix::fs::OpenOptionsExt;
			options.mode(0o600);
		}
		let file = options
			.open(&path)
			.with_context(|| format!("failed to open {}", path.display()))?;
		file.lock()
			.with_context(|| format!("failed to lock {}", path.display()))?;
		Ok(file)
	})
	.await?
}

pub(crate) fn publish(path: &Path, endpoint: &impl Serialize) -> Result<()> {
	use std::io::Write;
	let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
	let mut options = std::fs::OpenOptions::new();
	options.write(true).create_new(true);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt;
		options.mode(0o600);
	}
	let mut file = options.open(&temporary)?;
	file.write_all(serde_json::to_string(endpoint)?.as_bytes())?;
	file.flush()?;
	drop(file);
	std::fs::rename(temporary, path)?;
	Ok(())
}

pub(crate) async fn read_line<R: tokio::io::AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Result<Option<String>> {
	let mut line = String::new();
	let count = (&mut *reader)
		.take((MAX_SERVICE_LINE_BYTES + 1) as u64)
		.read_line(&mut line)
		.await?;
	anyhow::ensure!(
		count <= MAX_SERVICE_LINE_BYTES,
		"embedding service line exceeds byte limit"
	);
	if count == 0 {
		return Ok(None);
	}
	if !line.ends_with('\n') {
		return Err(std::io::Error::new(
			std::io::ErrorKind::UnexpectedEof,
			"embedding service line is incomplete",
		)
		.into());
	}
	Ok(Some(line))
}

pub(crate) async fn write_line<W: tokio::io::AsyncWrite + Unpin>(stream: &mut W, value: &impl Serialize) -> Result<()> {
	let body = serde_json::to_string(value)?;
	anyhow::ensure!(
		body.len() < MAX_SERVICE_LINE_BYTES,
		"embedding service message exceeds byte limit"
	);
	stream.write_all(body.as_bytes()).await?;
	stream.write_all(b"\n").await?;
	Ok(())
}
