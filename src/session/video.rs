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

// Video processing and attachment utilities

use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Video attachment for messages
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VideoAttachment {
	pub data: VideoData,
	pub media_type: String,
	pub source_type: SourceType,
	pub dimensions: Option<(u32, u32)>,
	pub size_bytes: Option<u64>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub duration_secs: Option<f64>,
}

/// Video data storage format
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum VideoData {
	Base64(String),
	Url(String),
}

/// Source of the video
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SourceType {
	File(PathBuf),
	Clipboard,
	Url,
}

/// Video processing utilities
pub struct VideoProcessor;

impl VideoProcessor {
	/// Maximum file size for video uploads (100MB - kimi and other providers typically allow larger videos)
	const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100MB

	/// Load video from file path
	pub fn load_from_path(path: &Path) -> Result<VideoAttachment> {
		let media_type = Self::get_media_type(path)?;
		Self::load_from_path_with_media_type(path, &media_type)
	}

	/// Load video from an opaque extensionless media path using the transport's
	/// declared media type. The same size, encoding, and metadata pipeline as
	/// normal path loading is retained.
	pub fn load_from_path_with_media_type(
		path: &Path,
		media_type: &str,
	) -> Result<VideoAttachment> {
		if !matches!(
			media_type,
			"video/mp4"
				| "video/quicktime"
				| "video/x-msvideo"
				| "video/webm"
				| "video/x-matroska"
				| "video/3gpp"
		) {
			return Err(anyhow::anyhow!(
				"Unsupported video media type: {}",
				media_type
			));
		}

		// Check file exists and size
		let metadata = std::fs::metadata(path)?;
		if metadata.len() > Self::MAX_FILE_SIZE {
			return Err(anyhow::anyhow!(
				"Video file too large: {}MB (max 100MB)",
				metadata.len() / 1024 / 1024
			));
		}

		// Read file and encode to base64
		let video_bytes = std::fs::read(path)?;
		let base64_data = general_purpose::STANDARD.encode(&video_bytes);

		// Try to get video dimensions using ffprobe if available
		let dimensions = Self::get_video_dimensions(path).ok();

		Ok(VideoAttachment {
			data: VideoData::Base64(base64_data),
			media_type: media_type.to_string(),
			source_type: SourceType::File(path.to_path_buf()),
			dimensions,
			size_bytes: Some(metadata.len()),
			duration_secs: Self::get_video_duration(path).ok(),
		})
	}

	/// Load video from URL
	pub async fn load_from_url(url: &str) -> Result<VideoAttachment> {
		use reqwest::Client;

		// Validate URL format
		let parsed_url = url::Url::parse(url).map_err(|_| anyhow::anyhow!("Invalid URL format"))?;

		// Check if URL looks like a video
		if let Some(mut path) = parsed_url.path_segments() {
			if let Some(filename) = path.next_back() {
				if !Self::is_supported_video_by_name(filename) {
					return Err(anyhow::anyhow!(
						"URL does not appear to point to a supported video format: {}",
						filename
					));
				}
			}
		}

		// Download the video
		let client = Client::new();
		let response = client
			.get(url)
			.header("User-Agent", "Octomind/1.0")
			.send()
			.await?;

		if !response.status().is_success() {
			return Err(anyhow::anyhow!(
				"Failed to download video: HTTP {}",
				response.status()
			));
		}

		// Check content type
		let content_type = response
			.headers()
			.get("content-type")
			.and_then(|h| h.to_str().ok())
			.unwrap_or("")
			.to_string();

		// Download video data
		let video_bytes = response.bytes().await?;

		if video_bytes.len() > Self::MAX_FILE_SIZE as usize {
			return Err(anyhow::anyhow!(
				"Video too large: {}MB (max 100MB)",
				video_bytes.len() / 1024 / 1024
			));
		}

		// Determine media type
		let media_type = if content_type.starts_with("video/") {
			content_type.to_string()
		} else {
			// Fallback to URL extension
			Self::guess_media_type_from_url(url).unwrap_or_else(|| "video/mp4".to_string())
		};

		let base64_data = general_purpose::STANDARD.encode(&video_bytes);

		Ok(VideoAttachment {
			data: VideoData::Base64(base64_data),
			media_type,
			source_type: SourceType::Url,
			dimensions: None, // Would need ffprobe on downloaded file
			size_bytes: Some(video_bytes.len() as u64),
			duration_secs: None,
		})
	}

	/// Show video preview in terminal (shows metadata + first frame if possible)
	pub fn show_preview(attachment: &VideoAttachment) -> Result<()> {
		// Show metadata
		if let Some((width, height)) = attachment.dimensions {
			crate::log_info!("🎬 Video: {}x{} ({})", width, height, attachment.media_type);
		} else {
			crate::log_info!("🎬 Video: {}", attachment.media_type);
		}

		if let Some(size) = attachment.size_bytes {
			let size_mb = size as f64 / (1024.0 * 1024.0);
			if size_mb >= 1.0 {
				crate::log_info!("📏 Size: {:.1}MB", size_mb);
			} else {
				crate::log_info!("📏 Size: {:.1}KB", size as f64 / 1024.0);
			}
		}

		if let Some(duration) = attachment.duration_secs {
			let mins = (duration as u64) / 60;
			let secs = (duration as u64) % 60;
			if mins > 0 {
				crate::log_info!("⏱️  Duration: {}:{:02}", mins, secs);
			} else {
				crate::log_info!("⏱️  Duration: {}s", secs);
			}
		}

		// Try to show a frame preview if the video is from a file
		if let SourceType::File(path) = &attachment.source_type {
			if let Err(e) = Self::show_frame_preview(path) {
				crate::log_debug!("⚠️  Video preview not available: {}", e);
			}
		}

		Ok(())
	}

	/// Try to extract and show a frame preview using ffmpeg
	fn show_frame_preview(video_path: &Path) -> Result<()> {
		// Try to use ffmpeg to extract first frame
		let output = std::process::Command::new("ffmpeg")
			.args([
				"-i",
				video_path.to_str().unwrap_or(""),
				"-ss",
				"00:00:00",
				"-vframes",
				"1",
				"-f",
				"image2pipe",
				"-vcodec",
				"png",
				"-",
			])
			.output()?;

		if !output.status.success() {
			return Err(anyhow::anyhow!("ffmpeg failed to extract frame"));
		}

		// Load the image from memory
		let img = image::load_from_memory(&output.stdout)?;

		// Display using viuer
		let config = viuer::Config {
			width: Some(40),
			height: Some(20),
			absolute_offset: false,
			..Default::default()
		};

		viuer::print(&img, &config)?;

		Ok(())
	}

	/// Try to get video dimensions using ffprobe
	fn get_video_dimensions(path: &Path) -> Result<(u32, u32)> {
		let output = std::process::Command::new("ffprobe")
			.args([
				"-v",
				"error",
				"-select_streams",
				"v:0",
				"-show_entries",
				"stream=width,height",
				"-of",
				"csv=p=0",
				path.to_str().unwrap_or(""),
			])
			.output()?;

		if !output.status.success() {
			return Err(anyhow::anyhow!("ffprobe failed"));
		}

		let output_str = String::from_utf8(output.stdout)?;
		let parts: Vec<&str> = output_str.trim().split(',').collect();

		if parts.len() == 2 {
			let width = parts[0].parse::<u32>()?;
			let height = parts[1].parse::<u32>()?;
			Ok((width, height))
		} else {
			Err(anyhow::anyhow!("Invalid ffprobe output"))
		}
	}

	/// Try to get video duration using ffprobe
	fn get_video_duration(path: &Path) -> Result<f64> {
		let output = std::process::Command::new("ffprobe")
			.args([
				"-v",
				"error",
				"-show_entries",
				"format=duration",
				"-of",
				"default=noprint_wrappers=1:nokey=1",
				path.to_str().unwrap_or(""),
			])
			.output()?;

		if !output.status.success() {
			return Err(anyhow::anyhow!("ffprobe failed"));
		}

		let output_str = String::from_utf8(output.stdout)?;
		let duration = output_str.trim().parse::<f64>()?;
		Ok(duration)
	}

	/// Check if file is a supported video format
	pub fn is_supported_video(path: &Path) -> bool {
		if let Some(extension) = path.extension() {
			if let Some(ext_str) = extension.to_str() {
				Self::is_supported_extension(ext_str)
			} else {
				false
			}
		} else {
			false
		}
	}

	/// Check if filename has supported video extension
	pub fn is_supported_video_by_name(filename: &str) -> bool {
		if let Some(ext) = filename.split('.').next_back() {
			Self::is_supported_extension(ext)
		} else {
			false
		}
	}

	/// Check if extension is supported
	fn is_supported_extension(ext: &str) -> bool {
		matches!(
			ext.to_lowercase().as_str(),
			"mp4" | "mov" | "avi" | "webm" | "mkv" | "m4v" | "3gp"
		)
	}

	/// Guess media type from URL
	fn guess_media_type_from_url(url: &str) -> Option<String> {
		if let Some(ext) = url.split('.').next_back() {
			match ext.to_lowercase().as_str() {
				"mp4" => Some("video/mp4".to_string()),
				"mov" => Some("video/quicktime".to_string()),
				"avi" => Some("video/x-msvideo".to_string()),
				"webm" => Some("video/webm".to_string()),
				"mkv" => Some("video/x-matroska".to_string()),
				"m4v" => Some("video/mp4".to_string()),
				"3gp" => Some("video/3gpp".to_string()),
				_ => None,
			}
		} else {
			None
		}
	}

	/// Get media type from file path
	fn get_media_type(path: &Path) -> Result<String> {
		if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
			match ext.to_lowercase().as_str() {
				"mp4" | "m4v" => Ok("video/mp4".to_string()),
				"mov" => Ok("video/quicktime".to_string()),
				"avi" => Ok("video/x-msvideo".to_string()),
				"webm" => Ok("video/webm".to_string()),
				"mkv" => Ok("video/x-matroska".to_string()),
				"3gp" => Ok("video/3gpp".to_string()),
				_ => Err(anyhow::anyhow!("Unsupported video format")),
			}
		} else {
			Err(anyhow::anyhow!("Could not determine video format"))
		}
	}

	/// Get supported video extensions for autocomplete
	pub fn supported_extensions() -> &'static [&'static str] {
		&["mp4", "mov", "avi", "webm", "mkv", "m4v", "3gp"]
	}

	/// Check if input string is a URL
	pub fn is_url(input: &str) -> bool {
		input.starts_with("http://") || input.starts_with("https://")
	}
}

#[cfg(test)]
mod tests {
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
		std::fs::write(&clip, b"not really a video").unwrap();
		let attachment = VideoProcessor::load_from_path(&clip).unwrap();
		assert_eq!(attachment.media_type, "video/mp4");
		assert_eq!(attachment.size_bytes, Some(16));
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
}
