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

//! Upload a session JSONL log to the share API:
//!   1. Read file off disk.
//!   2. Gzip in memory (sessions compress >10× — typically <50 KB on the wire).
//!   3. POST to `<share_host>/api/share` with `Content-Encoding: gzip` and an
//!      `X-Title` hint derived from the first user message.
//!   4. Parse the `{ id, url }` reply.
//!
//! The host comes from `OCTOMIND_SHARE_URL`; the default is the local dev
//! server while the share endpoint is in development. Flip to
//! `https://octomind.run` before shipping.

use anyhow::{Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Default upload host. Override at runtime via `OCTOMIND_SHARE_URL`.
/// TODO(deploy): flip to `https://octomind.run` once the worker is on prod.
const DEFAULT_SHARE_HOST: &str = "http://localhost:5173";

/// What `/share` resolves to.
#[derive(Debug, Clone, Deserialize)]
pub struct ShareResult {
	pub id: String,
	pub url: String,
}

fn share_host() -> String {
	std::env::var("OCTOMIND_SHARE_URL")
		.ok()
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| DEFAULT_SHARE_HOST.to_string())
}

/// Upload the given session log file to the share API. Reads the file, gzips,
/// POSTs, returns the share URL.
pub async fn share_session(session_file: &Path) -> Result<ShareResult> {
	let raw = fs::read(session_file)
		.with_context(|| format!("failed to read session file {}", session_file.display()))?;
	if raw.is_empty() {
		anyhow::bail!("session file is empty — nothing to share");
	}

	let title = extract_title(&raw);
	let gzipped = gzip(&raw)?;

	let host = share_host();
	let endpoint = format!("{}/api/share", host.trim_end_matches('/'));

	let client = reqwest::Client::builder()
		.timeout(std::time::Duration::from_secs(60))
		.build()
		.context("failed to build HTTP client")?;

	let mut req = client
		.post(&endpoint)
		.header("Content-Type", "application/x-ndjson")
		.header("Content-Encoding", "gzip");
	if let Some(t) = title {
		// Truncate header value to a safe length; the server caps at 200 anyway.
		let trimmed: String = t.chars().take(200).collect();
		req = req.header("X-Title", trimmed);
	}
	let res = req
		.body(gzipped)
		.send()
		.await
		.context("share upload failed")?;

	let status = res.status();
	if !status.is_success() {
		let body = res.text().await.unwrap_or_default();
		anyhow::bail!(
			"share API rejected upload: HTTP {} — {}",
			status,
			body.trim()
		);
	}
	let parsed: ShareResult = res
		.json()
		.await
		.context("share API returned malformed JSON")?;
	Ok(parsed)
}

fn gzip(data: &[u8]) -> Result<Vec<u8>> {
	let mut enc = GzEncoder::new(Vec::new(), Compression::default());
	enc.write_all(data).context("gzip encoder write failed")?;
	enc.finish().context("gzip encoder finish failed")
}

/// First user message of the session, single-line, for the `X-Title` hint.
/// Best-effort: tolerant of missing fields, partial lines, weird formats.
fn extract_title(raw: &[u8]) -> Option<String> {
	let text = std::str::from_utf8(raw).ok()?;
	for line in text.lines() {
		let line = line.trim();
		if line.is_empty() {
			continue;
		}
		let v: Value = match serde_json::from_str(line) {
			Ok(v) => v,
			Err(_) => continue,
		};
		let role = v.get("role").and_then(|r| r.as_str())?;
		if role != "user" {
			continue;
		}
		let content = v.get("content").and_then(|c| c.as_str())?;
		let stripped = content.trim();
		if stripped.is_empty() {
			continue;
		}
		// Skip bootstrap-looking entries (skill / identity / role-doc injections).
		let first_char = stripped.chars().next()?;
		if first_char == '<' {
			continue;
		}
		if stripped.starts_with("# Octomind") || stripped.starts_with("## Lessons") {
			continue;
		}
		// First conversational user message — that's the title.
		let single_line: String = stripped
			.lines()
			.next()
			.unwrap_or(stripped)
			.chars()
			.take(200)
			.collect();
		return Some(single_line);
	}
	None
}
