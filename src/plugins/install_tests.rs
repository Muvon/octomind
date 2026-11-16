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
use serial_test::serial;

const MANIFEST: &[u8] = include_bytes!("install_fixtures/manifest.json");
const SIGNATURE: &[u8] = include_bytes!("install_fixtures/manifest.json.sig");
const PUBLIC_KEY: &str = include_str!("install_fixtures/public-key.b64");
/// A real key, but not the one that signed the fixture.
const OTHER_KEY: &str = "NP/qvbvD3tG3vqBlp+m6aMIzjM4U0p2QdWdpok15v9Y=";

fn fixture_key() -> [u8; INSTALL_KEY_BYTES] {
	decode_install_key(PUBLIC_KEY).expect("fixture key decodes")
}

fn config(body: &str) -> PluginConfig {
	toml::from_str(&format!("name = \"fake\"\ntype = \"stdio\"\n{body}")).expect("plugin entry parses")
}

// ---------------------------------------------------------------------------
// Signature
// ---------------------------------------------------------------------------

#[test]
fn the_fixture_manifest_verifies_with_its_key() {
	verify_manifest(MANIFEST, SIGNATURE, &fixture_key()).expect("valid signature");
}

#[test]
fn one_changed_byte_of_the_manifest_fails() {
	let mut changed = MANIFEST.to_vec();
	let position = changed.iter().position(|byte| *byte == b'0').unwrap();
	changed[position] = b'1';
	let error = verify_manifest(&changed, SIGNATURE, &fixture_key())
		.unwrap_err()
		.to_string();
	assert!(error.contains("does not match"), "{error}");
}

#[test]
fn a_trailing_byte_added_to_the_manifest_fails() {
	let mut longer = MANIFEST.to_vec();
	longer.push(b' ');
	assert!(verify_manifest(&longer, SIGNATURE, &fixture_key()).is_err());
}

#[test]
fn another_key_fails() {
	let other = decode_install_key(OTHER_KEY).unwrap();
	assert!(verify_manifest(MANIFEST, SIGNATURE, &other).is_err());
}

#[test]
fn a_damaged_signature_file_fails() {
	for signature in [&b"not base64 !!"[..], b"AAAA", b""] {
		assert!(verify_manifest(MANIFEST, signature, &fixture_key()).is_err());
	}
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

#[test]
fn each_target_of_the_fixture_gives_its_own_archive() {
	let download = plan_download(MANIFEST, "aarch64-apple-darwin").unwrap();
	assert_eq!(
		download,
		Download {
			version: "0.1.0".to_string(),
			url: "https://dl.besydr.com/0.1.0/besydr-0.1.0-aarch64-apple-darwin.tar.gz".to_string(),
			sha256: "6e84e516146e2a30a3dfe7ec44b776c4c6a3dbee844f7635a3ff66f368ff8114".to_string(),
		}
	);
	assert!(plan_download(MANIFEST, "x86_64-pc-windows-msvc")
		.unwrap()
		.url
		.ends_with(".zip"));
}

#[test]
fn this_machine_has_a_build_in_the_release() {
	let target = this_target().expect("a supported machine");
	assert!(plan_download(MANIFEST, target).is_ok(), "{target}");
	if cfg!(target_os = "linux") {
		assert!(target.ends_with("linux-musl"), "{target}");
	}
}

#[test]
fn a_missing_target_is_named_in_the_error() {
	let error = plan_download(MANIFEST, "riscv64-unknown-linux-musl")
		.unwrap_err()
		.to_string();
	assert!(error.contains("riscv64-unknown-linux-musl"), "{error}");
}

fn manifest_with(url: &str, sha256: &str) -> Vec<u8> {
	format!(r#"{{"version":"1","targets":{{"t":{{"url":"{url}","sha256":"{sha256}"}}}}}}"#).into_bytes()
}

#[test]
fn an_archive_url_must_be_https() {
	let sha256 = "ab".repeat(SHA256_BYTES);
	plan_download(&manifest_with("https://dl.example.com/a.tar.gz", &sha256), "t").expect("https is fine");
	for url in [
		"http://dl.example.com/a.tar.gz",
		"ftp://dl.example.com/a.tar.gz",
		"/a.tar.gz",
	] {
		assert!(plan_download(&manifest_with(url, &sha256), "t").is_err(), "{url}");
	}
}

#[test]
fn a_malformed_hash_or_manifest_is_refused() {
	let not_hex = "zz".repeat(SHA256_BYTES);
	for sha256 in ["", "abcd", not_hex.as_str()] {
		assert!(plan_download(&manifest_with("https://dl.example.com/a", sha256), "t").is_err());
	}
	assert!(plan_download(b"not json", "t").is_err());
}

#[test]
fn a_hash_mismatch_is_reported_and_case_does_not_matter() {
	check_sha256("abc123", "ABC123").expect("same digest");
	let error = check_sha256("abc123", "def456").unwrap_err().to_string();
	assert!(error.contains("abc123") && error.contains("def456"), "{error}");
}

// ---------------------------------------------------------------------------
// Unpack
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
mod unpack {
	use super::*;

	fn archive_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
		let mut builder = tar::Builder::new(Vec::new());
		for (name, data) in entries {
			let mut header = tar::Header::new_gnu();
			header.set_size(data.len() as u64);
			header.set_mode(0o644);
			header.set_cksum();
			builder.append_data(&mut header, name, *data).unwrap();
		}
		gzip(builder.into_inner().unwrap())
	}

	/// An entry named `name` as written, with no check of the path the way
	/// `tar::Builder` does, like a hostile archive would have it.
	fn archive_with_raw_name(name: &[u8], data: &[u8]) -> Vec<u8> {
		let mut header = tar::Header::new_gnu();
		header.as_old_mut().name[..name.len()].copy_from_slice(name);
		header.set_size(data.len() as u64);
		header.set_mode(0o644);
		header.set_entry_type(tar::EntryType::Regular);
		header.set_cksum();
		let mut builder = tar::Builder::new(Vec::new());
		builder.append(&header, data).unwrap();
		gzip(builder.into_inner().unwrap())
	}

	fn gzip(tar: Vec<u8>) -> Vec<u8> {
		let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
		encoder.write_all(&tar).unwrap();
		encoder.finish().unwrap()
	}

	fn unpack(archive: &[u8], command: &str) -> (tempfile::TempDir, Result<NamedTempFile>) {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("download.tar.gz");
		std::fs::write(&path, archive).unwrap();
		let result = unpack_program(&path, command, dir.path());
		(dir, result)
	}

	#[test]
	fn the_named_entry_is_taken_and_made_executable() {
		let (_dir, result) = unpack(
			&archive_of(&[("README", b"read me"), ("besydr", b"#!program")]),
			"besydr",
		);
		let program = result.unwrap();
		assert_eq!(std::fs::read(program.path()).unwrap(), b"#!program");
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			assert_eq!(
				program.as_file().metadata().unwrap().permissions().mode() & 0o777,
				0o755
			);
		}
	}

	#[test]
	fn an_entry_written_with_a_leading_dot_is_still_the_program() {
		let (_dir, result) = unpack(&archive_of(&[("./besydr", b"program")]), "besydr");
		assert_eq!(std::fs::read(result.unwrap().path()).unwrap(), b"program");
	}

	#[test]
	fn a_path_that_leaves_the_directory_fails_the_archive() {
		for name in [&b"../besydr"[..], b"/etc/besydr", b"a/../../besydr"] {
			let (_dir, result) = unpack(&archive_with_raw_name(name, b"x"), "besydr");
			let error = result.unwrap_err().to_string();
			assert!(error.contains("leaves the plugin directory"), "{error}");
		}
	}

	#[test]
	fn a_missing_entry_is_an_error_and_a_nested_one_does_not_count() {
		for archive in [archive_of(&[("other", b"x")]), archive_of(&[("bin/besydr", b"x")])] {
			let (_dir, result) = unpack(&archive, "besydr");
			let error = result.unwrap_err().to_string();
			assert!(error.contains("no entry named 'besydr'"), "{error}");
		}
	}

	#[test]
	fn a_directory_named_like_the_program_is_not_the_program() {
		let mut builder = tar::Builder::new(Vec::new());
		let mut header = tar::Header::new_gnu();
		header.set_entry_type(tar::EntryType::Directory);
		header.set_size(0);
		header.set_mode(0o755);
		header.set_cksum();
		builder.append_data(&mut header, "besydr", std::io::empty()).unwrap();
		let (_dir, result) = unpack(&gzip(builder.into_inner().unwrap()), "besydr");
		assert!(result.unwrap_err().to_string().contains("not a regular file"));
	}

	#[test]
	fn something_that_is_not_a_tar_gz_is_an_error() {
		let (_dir, result) = unpack(b"plain text", "besydr");
		assert!(result.is_err());
	}
}

// ---------------------------------------------------------------------------
// Names and lookup order
// ---------------------------------------------------------------------------

#[test]
fn only_a_single_ordinary_component_is_a_plain_file_name() {
	for name in ["besydr", "besydr.exe", "my plugin"] {
		plain_file_name(name).expect(name);
	}
	for name in ["", "..", ".", "a/b", "../a", "/a"] {
		assert!(plain_file_name(name).is_err(), "{name:?}");
	}
}

#[test]
fn the_program_file_carries_the_suffix_of_the_os_once() {
	assert_eq!(program_file_name("besydr", ""), "besydr");
	assert_eq!(program_file_name("besydr", ".exe"), "besydr.exe");
	assert_eq!(program_file_name("besydr.exe", ".exe"), "besydr.exe");
	assert_eq!(program_file_name("Besydr.EXE", ".exe"), "Besydr.EXE");
}

#[test]
fn a_command_with_a_path_is_used_as_it_is() {
	let dir = tempfile::tempdir().unwrap();
	let found = locate("./tools/besydr", dir.path(), None);
	assert_eq!(found, Some(PathBuf::from("./tools/besydr")));
}

#[cfg(unix)]
mod lookup_order {
	use super::*;
	use std::os::unix::fs::PermissionsExt;

	fn program_in(dir: &Path, name: &str) -> PathBuf {
		let path = dir.join(name);
		std::fs::write(&path, "#!/bin/sh\n").unwrap();
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
		path
	}

	#[test]
	fn path_wins_over_the_plugin_directory() {
		let on_path = tempfile::tempdir().unwrap();
		let plugin_dir = tempfile::tempdir().unwrap();
		let expected = program_in(on_path.path(), "besydr");
		program_in(plugin_dir.path(), "besydr");
		let found = locate("besydr", plugin_dir.path(), Some(on_path.path().into()));
		assert_eq!(found, Some(expected));
	}

	#[test]
	fn the_plugin_directory_is_used_when_path_has_no_hit() {
		let empty = tempfile::tempdir().unwrap();
		let plugin_dir = tempfile::tempdir().unwrap();
		let expected = program_in(plugin_dir.path(), "besydr");
		assert_eq!(
			locate("besydr", plugin_dir.path(), Some(empty.path().into())),
			Some(expected)
		);
	}

	#[test]
	fn nothing_found_is_none() {
		let empty = tempfile::tempdir().unwrap();
		assert_eq!(locate("besydr", empty.path(), Some(empty.path().into())), None);
	}
}

/// Point the data directory at a temporary one for the length of a test.
struct DataDir {
	_dir: tempfile::TempDir,
	path: PathBuf,
}

impl DataDir {
	fn new() -> Self {
		let dir = tempfile::tempdir().unwrap();
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			path: dir.path().to_path_buf(),
			_dir: dir,
		}
	}
}

impl Drop for DataDir {
	fn drop(&mut self) {
		std::env::remove_var("OCTOMIND_DATA_DIR");
	}
}

#[tokio::test]
#[serial]
async fn without_a_manifest_a_missing_program_names_the_places_looked_at() {
	let data = DataDir::new();
	let plugin = config("command = \"octomind-test-absent-program\"\n");
	let error = resolve_command(&plugin).await.unwrap_err().to_string();
	assert!(error.contains("octomind-test-absent-program"), "{error}");
	assert!(error.contains("PATH"), "{error}");
	assert!(
		error.contains(&data.path.join("plugins").join("fake").display().to_string()),
		"{error}"
	);
	assert!(error.contains("install_manifest"), "{error}");
}

#[tokio::test]
#[serial]
async fn an_installed_copy_is_found_without_any_download() {
	let data = DataDir::new();
	let plugin_dir = data.path.join("plugins").join("fake");
	std::fs::create_dir_all(&plugin_dir).unwrap();
	std::fs::write(plugin_dir.join("octomind-test-installed"), "program").unwrap();
	// The manifest is unreachable on purpose: a download would fail the test.
	let plugin = config(&format!(
		"command = \"octomind-test-installed\"\ninstall_manifest = \"https://127.0.0.1:1/latest.json\"\ninstall_key = \"{OTHER_KEY}\"\n"
	));
	let found = resolve_command(&plugin).await.unwrap();
	assert_eq!(found, plugin_dir.join("octomind-test-installed"));
	assert_eq!(find_installed("octomind-test-installed", "fake").unwrap(), Some(found));
}

#[tokio::test]
#[serial]
async fn a_plugin_name_cannot_point_out_of_the_plugins_directory() {
	let _data = DataDir::new();
	let mut plugin = config("command = \"octomind-test-absent-program\"\n");
	plugin.name = "../escape".to_string();
	assert!(resolve_command(&plugin).await.is_err());
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Answer one request with `body`, over plain http on a local port.
async fn serve_once(body: Vec<u8>) -> String {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}/file", listener.local_addr().unwrap());
	tokio::spawn(async move {
		let (mut stream, _) = listener.accept().await.unwrap();
		let mut request = [0u8; 1024];
		let _ = stream.read(&mut request).await;
		let head = format!(
			"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
			body.len()
		);
		let _ = stream.write_all(head.as_bytes()).await;
		let _ = stream.write_all(&body).await;
	});
	url
}

#[tokio::test]
async fn a_body_within_the_limit_is_delivered_whole() {
	let url = serve_once(b"0123456789".to_vec()).await;
	let body = fetch(&reqwest::Client::new(), &url, 10).await.unwrap();
	assert_eq!(body, b"0123456789");
}

#[tokio::test]
async fn a_body_over_the_limit_is_refused() {
	let url = serve_once(b"0123456789".to_vec()).await;
	let error = fetch(&reqwest::Client::new(), &url, 9).await.unwrap_err().to_string();
	assert!(error.contains("larger than 9 bytes"), "{error}");
}

#[tokio::test]
async fn the_download_client_refuses_plain_http() {
	let url = serve_once(b"x".to_vec()).await;
	assert!(fetch(&download_client().unwrap(), &url, 10).await.is_err());
}
