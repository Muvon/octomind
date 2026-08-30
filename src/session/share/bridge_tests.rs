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
fn random_token_is_24_chars() {
	assert_eq!(random_token().len(), 24);
}

#[test]
fn random_token_is_alphanumeric() {
	let token = random_token();
	assert!(
		token.chars().all(|c| c.is_ascii_alphanumeric()),
		"non-alphanumeric char in token: {token}"
	);
}

#[test]
fn random_token_differs_between_calls() {
	let a = random_token();
	let b = random_token();
	assert_ne!(a, b, "two consecutive tokens must not collide");
}

#[test]
fn constant_time_eq_true_for_equal_slices() {
	assert!(constant_time_eq(b"bridge-token", b"bridge-token"));
	assert!(constant_time_eq(b"a", b"a"));
}

#[test]
fn constant_time_eq_false_for_unequal_slices() {
	assert!(!constant_time_eq(b"bridge-token", b"bridge-tokem"));
	assert!(!constant_time_eq(b"abc", b"abd"));
	// Same content, different case — still unequal bytes
	assert!(!constant_time_eq(b"Token", b"token"));
}

#[test]
fn constant_time_eq_false_for_different_lengths() {
	assert!(!constant_time_eq(b"short", b"longer-string"));
	assert!(!constant_time_eq(b"", b"x"));
	assert!(!constant_time_eq(b"x", b""));
}

#[test]
fn constant_time_eq_true_for_two_empty_slices() {
	assert!(constant_time_eq(b"", b""));
}

#[test]
fn bridge_info_supports_debug_and_clone() {
	let info = BridgeInfo {
		port: 8080,
		token: "abc123".to_string(),
	};
	let clone = info.clone();

	assert_eq!(clone.port, 8080);
	assert_eq!(clone.token, "abc123");

	let debug = format!("{info:?}");
	assert!(debug.contains("8080"), "Debug output missing port: {debug}");
	assert!(
		debug.contains("abc123"),
		"Debug output missing token: {debug}"
	);
}

#[test]
fn clear_for_session_is_noop_for_unknown_session() {
	// SessionId is a String alias; clearing an absent entry must not panic.
	let session_id: SessionId = "no-such-session".to_string();
	clear_for_session(&session_id);
}

// ── response builders ─────────────────────────────────────────────

#[tokio::test]
async fn plain_response_sets_status_headers_and_body() {
	use http_body_util::BodyExt;

	let res = plain(StatusCode::NOT_FOUND, "Not found\n");
	assert_eq!(res.status(), StatusCode::NOT_FOUND);
	assert_eq!(
		res.headers()
			.get("content-type")
			.and_then(|v| v.to_str().ok()),
		Some("text/plain; charset=utf-8")
	);
	assert_eq!(
		res.headers()
			.get("access-control-allow-origin")
			.and_then(|v| v.to_str().ok()),
		Some("*")
	);
	let body = res
		.into_body()
		.collect()
		.await
		.expect("collect body")
		.to_bytes();
	assert_eq!(&body[..], b"Not found\n");
}

#[tokio::test]
async fn cors_preflight_returns_no_content_with_cors_headers() {
	let res = cors_preflight();
	assert_eq!(res.status(), StatusCode::NO_CONTENT);
	for (header, expected) in [
		("access-control-allow-origin", "*"),
		("access-control-allow-methods", "GET, OPTIONS"),
		(
			"access-control-allow-headers",
			"x-bridge-token, content-type",
		),
		("access-control-max-age", "600"),
	] {
		assert_eq!(
			res.headers().get(header).and_then(|v| v.to_str().ok()),
			Some(expected),
			"wrong {header} header"
		);
	}
}

// ── registry lifecycle ─────────────────────────────────────────────

#[tokio::test]
async fn registry_replaces_entries_and_clear_removes_them() {
	let id: SessionId = format!("bridge-registry-{}", std::process::id());
	let first = tokio::spawn(std::future::pending::<()>());
	registry().lock().insert(
		id.clone(),
		BridgeHandle {
			abort: first.abort_handle(),
		},
	);
	assert!(registry().lock().contains_key(&id));

	// Re-inserting supersedes (and aborts) the previous handle.
	let second = tokio::spawn(std::future::pending::<()>());
	registry().lock().insert(
		id.clone(),
		BridgeHandle {
			abort: second.abort_handle(),
		},
	);
	assert!(registry().lock().contains_key(&id));

	clear_for_session(&id);
	assert!(
		!registry().lock().contains_key(&id),
		"clear must drop the entry"
	);
}

#[tokio::test]
async fn start_for_session_requires_session_context() {
	let err = start_for_session(std::path::PathBuf::from("/nonexistent/session.jsonl.zst"))
		.await
		.expect_err("must fail outside a session context");
	assert!(
		err.to_string().contains("no session id"),
		"unexpected error: {err}"
	);
}

// ── end-to-end HTTP over loopback ──────────────────────────────────

/// Minimal HTTP/1.1 client: one request per connection, read to EOF.
async fn http_request(
	method: &str,
	port: u16,
	path: &str,
	token: Option<&str>,
) -> (u16, Vec<(String, String)>, String) {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
		.await
		.expect("connect");
	let auth = token
		.map(|t| format!("x-bridge-token: {t}\r\n"))
		.unwrap_or_default();
	let request = format!(
		"{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Connection: close\r\nContent-Length: 0\r\n\r\n"
	);
	stream
		.write_all(request.as_bytes())
		.await
		.expect("write request");
	let mut raw = Vec::new();
	stream.read_to_end(&mut raw).await.expect("read response");
	let text = String::from_utf8_lossy(&raw).to_string();

	let (head, body) = text.split_once("\r\n\r\n").expect("header/body split");
	let mut lines = head.lines();
	let status_line = lines.next().expect("status line");
	let status: u16 = status_line
		.split_whitespace()
		.nth(1)
		.expect("status code")
		.parse()
		.expect("numeric status");
	let headers: Vec<(String, String)> = lines
		.filter_map(|l| l.split_once(':'))
		.map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
		.collect();
	(status, headers, body.to_string())
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
	headers
		.iter()
		.find(|(k, _)| k == name)
		.map(|(_, v)| v.as_str())
}

async fn start_test_bridge(session_id: &str, session_file: &std::path::Path) -> BridgeInfo {
	crate::session::context::with_session_id(
		session_id.to_string(),
		start_for_session(session_file.to_path_buf()),
	)
	.await
	.expect("bridge must start")
}

#[tokio::test]
async fn bridge_serves_health_and_rejects_unknown_paths_and_methods() {
	let info = start_test_bridge("bridge-e2e-routing", std::path::Path::new("/dev/null")).await;

	let (status, _, body) = http_request("GET", info.port, "/health", None).await;
	assert_eq!(status, 200, "health must be public");
	assert_eq!(body, "ok\n");

	let (status, _, body) = http_request("GET", info.port, "/no-such-path", None).await;
	assert_eq!(status, 404);
	assert_eq!(body, "Not found\n");

	let (status, _, body) = http_request("POST", info.port, "/health", None).await;
	assert_eq!(status, 405);
	assert_eq!(body, "GET only\n");

	let (status, headers, _) = http_request("OPTIONS", info.port, "/session", None).await;
	assert_eq!(status, 204, "preflight must answer 204");
	assert_eq!(header(&headers, "access-control-allow-origin"), Some("*"));
	assert_eq!(
		header(&headers, "access-control-allow-methods"),
		Some("GET, OPTIONS")
	);

	clear_for_session(&"bridge-e2e-routing".to_string());
}

#[tokio::test]
async fn session_endpoint_checks_token_and_serves_decompressed_jsonl() {
	let tmp = tempfile::TempDir::new().expect("temp dir");
	let file = tmp.path().join("session.jsonl.zst");
	let raw =
		b"{\"role\":\"user\",\"content\":\"hi\"}\n{\"role\":\"assistant\",\"content\":\"hello\"}\n";
	let compressed = zstd::encode_all(raw.as_slice(), 3).expect("compress fixture");
	std::fs::write(&file, &compressed).expect("write fixture");

	let info = start_test_bridge("bridge-e2e-session", &file).await;

	// No token → 401.
	let (status, _, body) = http_request("GET", info.port, "/session", None).await;
	assert_eq!(status, 401, "missing token must be rejected");
	assert_eq!(body, "Bad token\n");

	// Wrong token → 401.
	let (status, _, _) = http_request("GET", info.port, "/session", Some("wrong-token")).await;
	assert_eq!(status, 401, "wrong token must be rejected");

	// Correct token → decompressed JSONL with browser-facing headers.
	let (status, headers, body) =
		http_request("GET", info.port, "/session", Some(&info.token)).await;
	assert_eq!(status, 200);
	assert_eq!(
		body.as_bytes(),
		raw,
		"zstd payload must be decompressed verbatim"
	);
	assert_eq!(
		header(&headers, "content-type"),
		Some("application/x-ndjson")
	);
	assert_eq!(header(&headers, "cache-control"), Some("no-store"));
	assert_eq!(header(&headers, "access-control-allow-origin"), Some("*"));

	clear_for_session(&"bridge-e2e-session".to_string());
}

#[tokio::test]
async fn session_endpoint_reports_read_and_decompress_failures() {
	// Missing file → read failure.
	let tmp = tempfile::TempDir::new().expect("temp dir");
	let missing = tmp.path().join("missing.jsonl.zst");
	let info = start_test_bridge("bridge-e2e-missing", &missing).await;
	let (status, _, body) = http_request("GET", info.port, "/session", Some(&info.token)).await;
	assert_eq!(status, 500, "missing session file must 500");
	assert!(body.starts_with("Read failed:"), "unexpected body: {body}");
	clear_for_session(&"bridge-e2e-missing".to_string());

	// Present but non-zstd file → decompress failure.
	let garbage = tmp.path().join("garbage.jsonl.zst");
	std::fs::write(&garbage, b"this is definitely not zstd").expect("write garbage");
	let info = start_test_bridge("bridge-e2e-garbage", &garbage).await;
	let (status, _, body) = http_request("GET", info.port, "/session", Some(&info.token)).await;
	assert_eq!(status, 500, "corrupt session file must 500");
	assert!(
		body.starts_with("Decompress failed:"),
		"unexpected body: {body}"
	);
	clear_for_session(&"bridge-e2e-garbage".to_string());
}

#[tokio::test]
async fn restarting_supersedes_the_previous_bridge() {
	let tmp = tempfile::TempDir::new().expect("temp dir");
	let file = tmp.path().join("session.jsonl.zst");
	std::fs::write(&file, b"").expect("write fixture");

	let id = "bridge-e2e-restart";
	let first = start_test_bridge(id, &file).await;
	let second = start_test_bridge(id, &file).await;

	assert_ne!(first.port, second.port, "a fresh listener must be bound");
	assert_ne!(first.token, second.token, "a fresh token must be issued");

	// The new bridge answers; the registry holds exactly one entry for the id.
	let (status, _, _) = http_request("GET", second.port, "/health", None).await;
	assert_eq!(status, 200, "replacement bridge must serve");
	assert!(
		!registry().lock().is_empty(),
		"registry must hold the replacement"
	);

	clear_for_session(&id.to_string());
	assert!(!registry().lock().contains_key(id));
}
