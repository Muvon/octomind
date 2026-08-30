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
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn test_supported_extensions() {
	let extensions = VideoProcessor::supported_extensions();
	assert!(extensions.contains(&"mp4"));
	assert!(extensions.contains(&"mov"));
	assert!(extensions.contains(&"webm"));
}

#[test]
fn test_is_supported_video() {
	assert!(VideoProcessor::is_supported_video(Path::new("test.mp4")));
	assert!(VideoProcessor::is_supported_video(Path::new("test.MOV")));
	assert!(!VideoProcessor::is_supported_video(Path::new("test.txt")));
	assert!(!VideoProcessor::is_supported_video(Path::new("test.jpg")));
}

#[test]
fn test_is_url() {
	assert!(VideoProcessor::is_url("https://example.com/video.mp4"));
	assert!(VideoProcessor::is_url("http://example.com/video.mp4"));
	assert!(!VideoProcessor::is_url("/path/to/video.mp4"));
	assert!(!VideoProcessor::is_url("video.mp4"));
}

#[test]
fn media_type_helpers_cover_every_supported_extension() {
	for (name, media) in [
		("a.mp4", "video/mp4"),
		("a.m4v", "video/mp4"),
		("a.mov", "video/quicktime"),
		("a.avi", "video/x-msvideo"),
		("a.webm", "video/webm"),
		("a.mkv", "video/x-matroska"),
		("a.3gp", "video/3gpp"),
	] {
		assert_eq!(
			VideoProcessor::get_media_type(Path::new(name)).unwrap(),
			media
		);
		assert!(VideoProcessor::is_supported_video_by_name(name));
	}
	assert!(VideoProcessor::get_media_type(Path::new("a.bin")).is_err());
	assert!(VideoProcessor::get_media_type(Path::new("no-extension")).is_err());
	assert_eq!(
		VideoProcessor::guess_media_type_from_url("https://x/a.MKV").as_deref(),
		Some("video/x-matroska")
	);
	assert!(VideoProcessor::guess_media_type_from_url("https://x/a.bin").is_none());
}

#[test]
fn file_loading_encodes_bytes_and_rejects_bad_media_types() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("clip.mp4");
	std::fs::write(&path, b"fake-video-data").unwrap();
	let attachment = VideoProcessor::load_from_path(&path).unwrap();
	assert_eq!(attachment.media_type, "video/mp4");
	assert_eq!(attachment.size_bytes, Some(15));
	match attachment.data {
		VideoData::Base64(data) => {
			assert_eq!(
				general_purpose::STANDARD.decode(data).unwrap(),
				b"fake-video-data"
			)
		}
		VideoData::Url(_) => panic!("file load must encode data"),
	}
	assert!(VideoProcessor::load_from_path_with_media_type(&path, "video/unknown").is_err());
	assert!(VideoProcessor::load_from_path(&dir.path().join("missing.mp4")).is_err());
	assert!(VideoProcessor::load_from_path(&dir.path().join("clip.txt")).is_err());
}

async fn video_server(status: &str, content_type: &str, body: Vec<u8>) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	let status = status.to_string();
	let content_type = content_type.to_string();
	tokio::spawn(async move {
		let (mut socket, _) = listener.accept().await.unwrap();
		let mut request = [0_u8; 2048];
		let _ = socket.read(&mut request).await;
		let headers = format!(
				"HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
				body.len()
			);
		socket.write_all(headers.as_bytes()).await.unwrap();
		socket.write_all(&body).await.unwrap();
	});
	format!("http://{addr}/clip.mp4")
}

#[tokio::test]
async fn url_loading_covers_success_and_rejections() {
	let url = video_server("200 OK", "video/mp4", b"video".to_vec()).await;
	let attachment = VideoProcessor::load_from_url(&url).await.unwrap();
	assert_eq!(attachment.media_type, "video/mp4");
	assert_eq!(attachment.size_bytes, Some(5));
	assert!(matches!(attachment.source_type, SourceType::Url));

	let url = video_server("404 Not Found", "video/mp4", Vec::new()).await;
	assert!(VideoProcessor::load_from_url(&url).await.is_err());
	assert!(VideoProcessor::load_from_url("not a url").await.is_err());
	assert!(
		VideoProcessor::load_from_url("https://example.com/file.txt")
			.await
			.is_err()
	);
}

#[test]
fn preview_without_a_file_is_metadata_only() {
	let attachment = VideoAttachment {
		data: VideoData::Url("https://example.com/clip.mp4".into()),
		media_type: "video/mp4".into(),
		source_type: SourceType::Url,
		dimensions: Some((1920, 1080)),
		size_bytes: Some(2048),
		duration_secs: Some(65.0),
	};
	VideoProcessor::show_preview(&attachment).unwrap();
}

#[test]
fn oversized_file_is_rejected_before_reading() {
	let tmp = tempfile::tempdir().unwrap();
	let big = tmp.path().join("big.mp4");
	std::fs::write(&big, vec![0u8; 100 * 1024 * 1024 + 1]).unwrap();
	let err = VideoProcessor::load_from_path(&big).unwrap_err();
	assert!(err.to_string().contains("too large"));
}

#[test]
fn junk_video_file_loads_and_previews_without_ffprobe() {
	// ffprobe/ffmpeg may be absent: dimensions/duration degrade to None and
	// the frame preview is skipped — loading itself must still succeed.
	let tmp = tempfile::tempdir().unwrap();
	let clip = tmp.path().join("clip.mp4");
	let bytes = b"not really a video";
	std::fs::write(&clip, bytes).unwrap();
	let attachment = VideoProcessor::load_from_path(&clip).unwrap();
	assert_eq!(attachment.media_type, "video/mp4");
	assert_eq!(attachment.size_bytes, Some(bytes.len() as u64));
	VideoProcessor::show_preview(&attachment).unwrap();
}

#[test]
fn predicate_edge_cases() {
	assert!(!VideoProcessor::is_supported_video(std::path::Path::new(
		"noext"
	)));
	assert!(!VideoProcessor::is_supported_video_by_name("noext"));
	assert!(VideoProcessor::guess_media_type_from_url("no-dot").is_none());
}
