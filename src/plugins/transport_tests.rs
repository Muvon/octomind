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

//! Transports against real peers: a unix socket listener, a child process and
//! a local HTTP server, all inside the test.

use super::*;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

const WAIT: Duration = Duration::from_secs(5);
const LIMIT: WaitLimit = WaitLimit {
	silence: WAIT,
	total: WAIT,
};

/// The same time for the silence and for the whole request.
fn limit(time: Duration) -> WaitLimit {
	WaitLimit {
		silence: time,
		total: time,
	}
}

fn config(body: &str) -> PluginConfig {
	toml::from_str(&format!("name = \"fake\"\n{body}")).expect("test config parses")
}

fn response_line(id: Option<u64>, result: Value) -> String {
	format!("{}\n", json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

async fn wait_until_closed(transport: &LineTransport) {
	tokio::time::timeout(WAIT, async {
		while transport.is_connected() {
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("the connection must be seen as closed");
}

// ---------------------------------------------------------------------------
// Unix socket
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod unix_socket {
	use super::*;
	use crate::plugins::socket;
	use tokio::net::UnixListener;

	fn socket_config(path: &std::path::Path) -> PluginConfig {
		config(&format!("type = \"socket\"\npath = \"{}\"\n", path.display()))
	}

	async fn read_request<R: tokio::io::AsyncBufRead + Unpin>(lines: &mut tokio::io::Lines<R>) -> RpcRequest {
		let line = lines.next_line().await.unwrap().expect("a request line");
		serde_json::from_str(&line).expect("a request")
	}

	#[tokio::test]
	async fn answers_are_routed_by_id_whatever_their_order_and_noise_is_skipped() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plugin.sock");
		let listener = UnixListener::bind(&path).unwrap();
		let (notification_sender, notification_receiver) = oneshot::channel();
		let server = tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			let (reader, mut writer) = stream.into_split();
			let mut lines = BufReader::new(reader).lines();
			writer.write_all(b"a log line that is not json\n").await.unwrap();
			writer
				.write_all(response_line(Some(999), json!({})).as_bytes())
				.await
				.unwrap();
			let first = read_request(&mut lines).await;
			let second = read_request(&mut lines).await;
			// Answer the later request first.
			for request in [&second, &first] {
				let result = json!({"n": request.params["n"], "method": request.method});
				writer
					.write_all(response_line(request.id, result).as_bytes())
					.await
					.unwrap();
			}
			let notification = read_request(&mut lines).await;
			notification_sender.send(notification).unwrap();
		});

		let transport = socket::connect_socket(&socket_config(&path)).await.unwrap();
		let (first, second) = tokio::join!(
			transport.request("alpha", json!({"n": 1}), LIMIT),
			transport.request("beta", json!({"n": 2}), LIMIT)
		);
		assert_eq!(first.unwrap(), json!({"n": 1, "method": "alpha"}));
		assert_eq!(second.unwrap(), json!({"n": 2, "method": "beta"}));

		transport
			.notify("session.end", json!({"reason": "exit"}))
			.await
			.unwrap();
		let notification = notification_receiver.await.unwrap();
		assert_eq!(notification.id, None);
		assert_eq!(notification.method, "session.end");
		assert_eq!(notification.params["reason"], "exit");

		server.await.unwrap();
		// The server is gone: the connection is seen as closed and requests fail fast.
		wait_until_closed(&transport).await;
		let error = transport.request("late", json!({}), LIMIT).await.unwrap_err();
		assert!(error.to_string().contains("closed the connection"), "{error}");
	}

	#[tokio::test]
	async fn a_request_waiting_when_the_peer_closes_fails_at_once() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plugin.sock");
		let listener = UnixListener::bind(&path).unwrap();
		let server = tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			let (reader, _writer) = stream.into_split();
			let mut lines = BufReader::new(reader).lines();
			read_request(&mut lines).await;
		});
		let transport = socket::connect_socket(&socket_config(&path)).await.unwrap();
		let error = transport.request("x", json!({}), LIMIT).await.unwrap_err();
		assert!(error.to_string().contains("closed the connection"), "{error}");
		server.await.unwrap();
	}

	#[tokio::test]
	async fn an_error_response_becomes_an_error_with_its_code() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plugin.sock");
		let listener = UnixListener::bind(&path).unwrap();
		tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			let (reader, mut writer) = stream.into_split();
			let mut lines = BufReader::new(reader).lines();
			let request = read_request(&mut lines).await;
			let line = format!(
				"{}\n",
				json!({"jsonrpc": "2.0", "id": request.id, "error": {"code": -32601, "message": "no such method"}})
			);
			writer.write_all(line.as_bytes()).await.unwrap();
			// Hold the connection open so only the error ends the request.
			let _ = lines.next_line().await;
		});
		let transport = socket::connect_socket(&socket_config(&path)).await.unwrap();
		let error = transport.request("x", json!({}), LIMIT).await.unwrap_err().to_string();
		assert!(error.contains("-32601") && error.contains("no such method"), "{error}");
		assert!(transport.is_connected());
	}

	#[tokio::test]
	async fn a_silent_peer_times_the_request_out_and_leaves_no_waiter() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plugin.sock");
		let listener = UnixListener::bind(&path).unwrap();
		tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			let (reader, _writer) = stream.into_split();
			let mut lines = BufReader::new(reader).lines();
			while let Ok(Some(_)) = lines.next_line().await {}
		});
		let transport = socket::connect_socket(&socket_config(&path)).await.unwrap();
		let error = transport
			.request("x", json!({}), limit(Duration::from_millis(100)))
			.await
			.unwrap_err();
		assert!(error.to_string().contains("no answer"), "{error}");
		assert!(transport.waiters.lock().is_empty());
		assert!(transport.is_connected());
	}

	#[tokio::test]
	async fn nothing_listening_and_no_command_is_an_error_naming_the_path() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("absent.sock");
		let error = socket::connect_socket(&socket_config(&path))
			.await
			.err()
			.unwrap()
			.to_string();
		assert!(
			error.contains("nothing answers") && error.contains("absent.sock"),
			"{error}"
		);
	}

	#[tokio::test]
	async fn a_command_that_cannot_start_is_reported() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("absent.sock");
		let plugin = config(&format!(
			"type = \"socket\"\npath = \"{}\"\ncommand = \"/nonexistent/octomind-test-plugin\"\n",
			path.display()
		));
		let error = socket::connect_socket(&plugin).await.err().unwrap().to_string();
		assert!(error.contains("Failed to start plugin"), "{error}");
	}

	#[tokio::test]
	async fn a_command_with_a_missing_env_key_is_not_started() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("absent.sock");
		let plugin = config(&format!(
			"type = \"socket\"\npath = \"{}\"\ncommand = \"prog\"\nargs = [\"{{{{ENV:OCTOMIND_PLUGIN_TEST_UNSET_SOCKET_KEY}}}}\"]\n",
			path.display()
		));
		let error = socket::connect_socket(&plugin).await.err().unwrap().to_string();
		assert!(error.contains("OCTOMIND_PLUGIN_TEST_UNSET_SOCKET_KEY"), "{error}");
	}
}

// ---------------------------------------------------------------------------
// Standard input and output of a child process
// ---------------------------------------------------------------------------

/// Prints a line that is not a message, then answers every request with its own id.
#[cfg(unix)]
const ECHO_SCRIPT: &str = r##"
echo "plugin starting up"
while IFS= read -r line; do
	id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
	if [ -n "$id" ]; then
		printf '{"jsonrpc":"2.0","id":%s,"result":{"seen":true}}\n' "$id"
	fi
done
"##;

#[cfg(unix)]
#[tokio::test]
async fn a_child_process_answers_over_its_standard_streams() {
	let plugin = PluginConfig {
		args: vec!["-c".to_string(), ECHO_SCRIPT.to_string()],
		..config("type = \"stdio\"\ncommand = \"sh\"\n")
	};
	let transport = spawn_stdio(&plugin).unwrap();
	let first = transport.request("one", json!({}), LIMIT).await.unwrap();
	assert_eq!(first, json!({"seen": true}));
	transport.notify("note", json!({})).await.unwrap();
	let answers = futures::future::join_all((0..5).map(|n| transport.request("many", json!({"n": n}), LIMIT))).await;
	assert!(answers
		.iter()
		.all(|answer| answer.as_ref().is_ok_and(|value| value["seen"] == true)));
	assert!(transport.is_connected());
}

#[cfg(unix)]
#[tokio::test]
async fn a_child_that_exits_closes_the_connection() {
	let plugin = PluginConfig {
		args: vec!["-c".to_string(), "exit 0".to_string()],
		..config("type = \"stdio\"\ncommand = \"sh\"\n")
	};
	let transport = spawn_stdio(&plugin).unwrap();
	wait_until_closed(&transport).await;
	let error = transport.request("x", json!({}), LIMIT).await.unwrap_err();
	assert!(error.to_string().contains("closed the connection"), "{error}");
}

#[tokio::test]
async fn a_program_that_does_not_exist_is_reported_by_name() {
	let plugin = config("type = \"stdio\"\ncommand = \"/nonexistent/octomind-test-plugin\"\n");
	let error = spawn_stdio(&plugin).err().unwrap().to_string();
	assert!(error.contains("Failed to start plugin 'fake'"), "{error}");
}

#[tokio::test]
async fn an_unset_env_key_in_the_command_stops_the_start() {
	let plugin = config(
		"type = \"stdio\"\ncommand = \"prog\"\nargs = [\"--key={{ENV:OCTOMIND_PLUGIN_TEST_UNSET_STDIO_KEY}}\"]\n",
	);
	let error = spawn_stdio(&plugin).err().unwrap().to_string();
	assert!(error.contains("OCTOMIND_PLUGIN_TEST_UNSET_STDIO_KEY"), "{error}");
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/// What the test server saw of one request.
struct SeenRequest {
	head: String,
	body: String,
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> SeenRequest {
	let mut received = Vec::new();
	let mut chunk = [0u8; 1024];
	let (head_end, content_length) = loop {
		let read = stream.read(&mut chunk).await.unwrap();
		assert!(read > 0, "the client closed before sending a full request");
		received.extend_from_slice(&chunk[..read]);
		let text = String::from_utf8_lossy(&received).to_string();
		if let Some(end) = text.find("\r\n\r\n") {
			let length = text[..end]
				.lines()
				.find_map(|line| {
					line.to_ascii_lowercase()
						.strip_prefix("content-length:")
						.map(|v| v.trim().parse::<usize>().unwrap())
				})
				.unwrap_or(0);
			break (end + 4, length);
		}
	};
	while received.len() < head_end + content_length {
		let read = stream.read(&mut chunk).await.unwrap();
		received.extend_from_slice(&chunk[..read]);
	}
	SeenRequest {
		head: String::from_utf8_lossy(&received[..head_end]).to_string(),
		body: String::from_utf8_lossy(&received[head_end..head_end + content_length]).to_string(),
	}
}

/// Serve one scripted `(status, body)` per connection. `__ID__` in a body
/// becomes the id of the request it answers.
fn serve_http(listener: TcpListener, script: Vec<(u16, &'static str)>) -> tokio::task::JoinHandle<Vec<SeenRequest>> {
	tokio::spawn(async move {
		let mut seen = Vec::new();
		for (status, body) in script {
			let (mut stream, _) = listener.accept().await.unwrap();
			let request = read_http_request(&mut stream).await;
			let id = serde_json::from_str::<RpcRequest>(&request.body)
				.ok()
				.and_then(|request| request.id)
				.map_or_else(|| "null".to_string(), |id| id.to_string());
			let body = body.replace("__ID__", &id);
			let reply = format!(
				"HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
				body.len()
			);
			stream.write_all(reply.as_bytes()).await.unwrap();
			stream.shutdown().await.unwrap();
			seen.push(request);
		}
		seen
	})
}

async fn http_plugin(extra: &str) -> (TcpListener, PluginConfig) {
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}/rpc", listener.local_addr().unwrap());
	let plugin = config(&format!("type = \"http\"\nurl = \"{url}\"\n{extra}"));
	(listener, plugin)
}

#[tokio::test]
async fn http_posts_each_request_with_resolved_headers_and_reads_the_result() {
	const TOKEN_KEY: &str = "OCTOMIND_PLUGIN_TEST_HTTP_TOKEN";
	std::env::set_var(TOKEN_KEY, "abc123");
	let (listener, plugin) = http_plugin(&format!(
		"headers = {{ Authorization = \"Bearer {{{{ENV:{TOKEN_KEY}}}}}\" }}\n"
	))
	.await;
	let server = serve_http(
		listener,
		vec![(200, r#"{"jsonrpc":"2.0","id":__ID__,"result":{"ok":1}}"#)],
	);

	let transport = HttpTransport::new(&plugin).unwrap();
	let result = transport
		.request("initialize", json!({"protocol": 1}), LIMIT)
		.await
		.unwrap();
	assert_eq!(result, json!({"ok": 1}));

	let seen = server.await.unwrap();
	assert!(seen[0].head.starts_with("POST /rpc "), "{}", seen[0].head);
	assert!(
		seen[0]
			.head
			.to_ascii_lowercase()
			.contains("authorization: bearer abc123"),
		"{}",
		seen[0].head
	);
	let sent: RpcRequest = serde_json::from_str(&seen[0].body).unwrap();
	assert_eq!((sent.method.as_str(), sent.id), ("initialize", Some(1)));
	assert_eq!(sent.params["protocol"], 1);
}

#[tokio::test]
async fn http_notifications_carry_no_id_and_need_no_answer_body() {
	let (listener, plugin) = http_plugin("").await;
	let server = serve_http(listener, vec![(202, "")]);
	let transport = HttpTransport::new(&plugin).unwrap();
	transport
		.notify("session.end", json!({"reason": "exit"}))
		.await
		.unwrap();
	let seen = server.await.unwrap();
	let sent: RpcRequest = serde_json::from_str(&seen[0].body).unwrap();
	assert_eq!(sent.id, None);
	assert_eq!(sent.method, "session.end");
}

#[tokio::test]
async fn http_error_status_and_error_objects_are_errors() {
	let (listener, plugin) = http_plugin("").await;
	let server = serve_http(
		listener,
		vec![
			(500, "boom"),
			(
				200,
				r#"{"jsonrpc":"2.0","id":__ID__,"error":{"code":-32000,"message":"refused"}}"#,
			),
			(200, r#"{"jsonrpc":"2.0","id":4242,"result":{}}"#),
			(200, "not json"),
		],
	);
	let transport = HttpTransport::new(&plugin).unwrap();
	let status = transport.request("x", json!({}), LIMIT).await.unwrap_err().to_string();
	assert!(status.contains("500"), "{status}");
	let refused = transport.request("x", json!({}), LIMIT).await.unwrap_err().to_string();
	assert!(refused.contains("-32000") && refused.contains("refused"), "{refused}");
	let wrong_id = transport.request("x", json!({}), LIMIT).await.unwrap_err().to_string();
	assert!(wrong_id.contains("4242"), "{wrong_id}");
	let unreadable = transport.request("x", json!({}), LIMIT).await.unwrap_err().to_string();
	assert!(unreadable.contains("unreadable"), "{unreadable}");
	server.await.unwrap();
}

#[tokio::test]
async fn http_with_an_unset_env_key_in_a_header_is_refused() {
	let (_listener, plugin) =
		http_plugin("headers = { Authorization = \"Bearer {{ENV:OCTOMIND_PLUGIN_TEST_UNSET_HTTP_KEY}}\" }\n").await;
	let error = HttpTransport::new(&plugin).err().unwrap().to_string();
	assert!(error.contains("OCTOMIND_PLUGIN_TEST_UNSET_HTTP_KEY"), "{error}");
}

#[tokio::test]
async fn http_with_a_bad_header_name_is_refused() {
	let (_listener, plugin) = http_plugin("headers = { \"bad name\" = \"x\" }\n").await;
	assert!(HttpTransport::new(&plugin).is_err());
}

#[tokio::test]
async fn the_process_connector_builds_each_kind() {
	let (_listener, http) = http_plugin("").await;
	assert!(ProcessConnector.connect(&http).await.unwrap().is_connected());
	let missing = config("type = \"stdio\"\ncommand = \"/nonexistent/octomind-test-plugin\"\n");
	assert!(ProcessConnector.connect(&missing).await.is_err());
}

// ---------------------------------------------------------------------------
// Cancel
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod cancel_over_socket {
	use super::*;
	use crate::plugins::socket;
	use tokio::net::UnixListener;

	/// A peer that never answers: it reports the first request and the line after it.
	fn silent_peer(listener: UnixListener) -> tokio::task::JoinHandle<(RpcRequest, RpcRequest)> {
		tokio::spawn(async move {
			let (stream, _) = listener.accept().await.unwrap();
			let (reader, _writer) = stream.into_split();
			let mut lines = BufReader::new(reader).lines();
			let first = lines.next_line().await.unwrap().unwrap();
			let second = lines.next_line().await.unwrap().unwrap();
			(
				serde_json::from_str(&first).unwrap(),
				serde_json::from_str(&second).unwrap(),
			)
		})
	}

	async fn connect(dir: &std::path::Path) -> (LineTransport, tokio::task::JoinHandle<(RpcRequest, RpcRequest)>) {
		let path = dir.join("plugin.sock");
		let peer = silent_peer(UnixListener::bind(&path).unwrap());
		let plugin = config(&format!("type = \"socket\"\npath = \"{}\"\n", path.display()));
		(socket::connect_socket(&plugin).await.unwrap(), peer)
	}

	fn assert_cancel_of(request: &RpcRequest, cancel: &RpcRequest) {
		assert_eq!(cancel.method, METHOD_CANCEL);
		assert_eq!(cancel.id, None, "cancel is a notification");
		assert_eq!(cancel.params, json!({"id": request.id}));
		assert!(request.id.is_some());
	}

	#[tokio::test]
	async fn a_request_that_times_out_sends_cancel_with_its_id() {
		let dir = tempfile::tempdir().unwrap();
		let (transport, peer) = connect(dir.path()).await;
		assert!(transport
			.request("slow", json!({}), limit(Duration::from_millis(100)))
			.await
			.is_err());
		let (request, cancel) = tokio::time::timeout(WAIT, peer).await.unwrap().unwrap();
		assert_eq!(request.method, "slow");
		assert_cancel_of(&request, &cancel);
	}

	#[tokio::test]
	async fn a_dropped_request_sends_cancel_with_its_id() {
		let dir = tempfile::tempdir().unwrap();
		let (transport, peer) = connect(dir.path()).await;
		// The caller gives up (Ctrl+C): its future is dropped long before the limit.
		let abandoned =
			tokio::time::timeout(Duration::from_millis(100), transport.request("slow", json!({}), LIMIT)).await;
		assert!(abandoned.is_err());
		let (request, cancel) = tokio::time::timeout(WAIT, peer).await.unwrap().unwrap();
		assert_cancel_of(&request, &cancel);
		assert!(transport.waiters.lock().is_empty());
	}
}

/// Accepts two connections: the first request is never answered, the second is
/// the cancel and gets a 202.
fn cancel_watching_server(listener: TcpListener) -> tokio::task::JoinHandle<(SeenRequest, SeenRequest)> {
	tokio::spawn(async move {
		let (mut first_stream, _) = listener.accept().await.unwrap();
		let first = read_http_request(&mut first_stream).await;
		let (mut second_stream, _) = listener.accept().await.unwrap();
		let second = read_http_request(&mut second_stream).await;
		second_stream
			.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
			.await
			.unwrap();
		(first, second)
	})
}

fn assert_http_cancel(first: &SeenRequest, second: &SeenRequest) {
	let request: RpcRequest = serde_json::from_str(&first.body).unwrap();
	let cancel: RpcRequest = serde_json::from_str(&second.body).unwrap();
	assert_eq!(cancel.method, METHOD_CANCEL);
	assert_eq!(cancel.id, None);
	assert_eq!(cancel.params, json!({"id": request.id}));
}

#[tokio::test]
async fn http_sends_one_cancel_post_when_a_request_times_out() {
	let (listener, plugin) = http_plugin("").await;
	let server = cancel_watching_server(listener);
	let transport = HttpTransport::new(&plugin).unwrap();
	assert!(transport
		.request("slow", json!({}), limit(Duration::from_millis(200)))
		.await
		.is_err());
	let (first, second) = tokio::time::timeout(WAIT, server).await.unwrap().unwrap();
	assert_http_cancel(&first, &second);
}

#[tokio::test]
async fn http_sends_one_cancel_post_when_a_request_is_dropped() {
	let (listener, plugin) = http_plugin("").await;
	let server = cancel_watching_server(listener);
	let transport = HttpTransport::new(&plugin).unwrap();
	let abandoned = tokio::time::timeout(Duration::from_millis(200), transport.request("slow", json!({}), LIMIT)).await;
	assert!(abandoned.is_err());
	let (first, second) = tokio::time::timeout(WAIT, server).await.unwrap().unwrap();
	assert_http_cancel(&first, &second);
}

#[tokio::test]
async fn a_request_that_times_out_mid_write_still_puts_a_whole_line_on_the_pipe() {
	// The pipe holds 16 bytes, so the write blocks until the peer reads.
	const PIPE_CAPACITY: usize = 16;
	let (reader, _reader_peer) = tokio::io::duplex(PIPE_CAPACITY);
	let (writer, writer_peer) = tokio::io::duplex(PIPE_CAPACITY);
	let transport = LineTransport::new("slow", reader, writer, None);

	let padding = "x".repeat(200);
	let error = transport
		.request("first", json!({"padding": padding}), limit(Duration::from_millis(100)))
		.await
		.unwrap_err();
	assert!(error.to_string().contains("no answer"), "{error}");

	let mut lines = BufReader::new(writer_peer).lines();
	let whole = tokio::time::timeout(WAIT, lines.next_line())
		.await
		.expect("the line is finished once the peer reads")
		.unwrap()
		.expect("a line");
	let request: RpcRequest = serde_json::from_str(&whole).expect("the first line is a whole request");
	assert_eq!(request.method, "first");
	let cancel = tokio::time::timeout(WAIT, lines.next_line())
		.await
		.expect("the cancel follows")
		.unwrap()
		.expect("a line");
	let cancel: RpcRequest = serde_json::from_str(&cancel).expect("the cancel is a line of its own");
	assert_eq!(cancel.method, METHOD_CANCEL);
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

const PIPE_ROOM: usize = 64 * 1024;
/// Shorter than the silence limit of the tests below, so one gap alone never fails a request.
const PROGRESS_GAP: Duration = Duration::from_millis(300);

type SentLines = tokio::io::Lines<BufReader<tokio::io::DuplexStream>>;

/// A transport with a peer the test drives: the lines the transport wrote and
/// a way to write to it.
fn piped() -> (LineTransport, SentLines, tokio::io::DuplexStream) {
	let (reader, to_transport) = tokio::io::duplex(PIPE_ROOM);
	let (writer, from_transport) = tokio::io::duplex(PIPE_ROOM);
	let transport = LineTransport::new("piped", reader, writer, None);
	(transport, BufReader::new(from_transport).lines(), to_transport)
}

fn progress_line(id: u64) -> String {
	format!(
		"{}\n",
		json!({"jsonrpc": "2.0", "method": "progress", "params": {"id": id}})
	)
}

async fn next_request_id(sent: &mut SentLines) -> u64 {
	let line = sent.next_line().await.unwrap().expect("a request line");
	let request: RpcRequest = serde_json::from_str(&line).expect("a request");
	request.id.expect("a request has an id")
}

/// A waiter for request 1, as `request` registers it.
fn waiter_of_request_one() -> (Waiters, oneshot::Receiver<Result<Value>>, Arc<Notify>) {
	let waiters = Waiters::default();
	let (answer, receiver) = oneshot::channel();
	let progress = Arc::new(Notify::new());
	waiters.lock().insert(
		1,
		Waiter {
			answer,
			progress: progress.clone(),
		},
	);
	(waiters, receiver, progress)
}

#[tokio::test]
async fn an_answer_after_the_silence_limit_is_accepted_when_progress_came_in_between() {
	let (transport, mut sent, mut to_transport) = piped();
	let peer = tokio::spawn(async move {
		let id = next_request_id(&mut sent).await;
		for _ in 0..2 {
			tokio::time::sleep(PROGRESS_GAP).await;
			to_transport.write_all(progress_line(id).as_bytes()).await.unwrap();
		}
		tokio::time::sleep(PROGRESS_GAP).await;
		let answer = response_line(Some(id), json!({"done": true}));
		to_transport.write_all(answer.as_bytes()).await.unwrap();
	});
	let limit = WaitLimit {
		silence: Duration::from_millis(500),
		total: WAIT,
	};

	let answer = transport.request("slow", json!({}), limit).await.unwrap();

	assert_eq!(answer, json!({"done": true}));
	peer.await.unwrap();
}

#[tokio::test]
async fn a_plugin_that_sends_nothing_fails_the_request_at_the_silence_limit() {
	let (transport, mut sent, to_transport) = piped();
	let _peer = tokio::spawn(async move {
		let _held_open = to_transport;
		while let Ok(Some(_)) = sent.next_line().await {}
	});
	let limit = WaitLimit {
		silence: Duration::from_secs(1),
		total: WAIT,
	};

	let error = transport.request("slow", json!({}), limit).await.unwrap_err();

	assert!(error.to_string().contains("no answer within 1s"), "{error}");
	assert!(transport.waiters.lock().is_empty());
}

#[tokio::test]
async fn progress_cannot_carry_a_request_past_the_total_limit() {
	let (transport, mut sent, mut to_transport) = piped();
	let _peer = tokio::spawn(async move {
		let id = next_request_id(&mut sent).await;
		loop {
			tokio::time::sleep(PROGRESS_GAP).await;
			if to_transport.write_all(progress_line(id).as_bytes()).await.is_err() {
				break;
			}
		}
	});
	let limit = WaitLimit {
		silence: Duration::from_secs(1),
		total: Duration::from_secs(2),
	};

	let error = transport.request("slow", json!({}), limit).await.unwrap_err();

	assert!(error.to_string().contains("no answer within 2s"), "{error}");
}

#[tokio::test]
async fn a_progress_line_for_an_unknown_request_changes_nothing() {
	let (waiters, mut receiver, progress) = waiter_of_request_one();

	route_line("piped", &progress_line(999), &waiters);

	assert!(waiters.lock().contains_key(&1));
	assert!(matches!(receiver.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
	let woken = tokio::time::timeout(Duration::from_millis(50), progress.notified()).await;
	assert!(woken.is_err(), "the other request was not told about it");
}

#[tokio::test]
async fn a_progress_line_wakes_its_request_and_is_not_taken_for_the_answer() {
	let (waiters, mut receiver, progress) = waiter_of_request_one();

	route_line("piped", &progress_line(1), &waiters);

	assert!(waiters.lock().contains_key(&1), "the request is still waiting");
	assert!(matches!(receiver.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
	tokio::time::timeout(WAIT, progress.notified())
		.await
		.expect("the request was woken");
}
