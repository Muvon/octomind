// Copyright 2025 Muvon Un Limited
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
		// Check file exists and size
		let metadata = std::fs::metadata(path)?;
		if metadata.len() > Self::MAX_FILE_SIZE {
			return Err(anyhow::anyhow!(
				"Video file too large: {}MB (max 100MB)",
				metadata.len() / 1024 / 1024
			));
		}

		// Check if it's a supported video format
		if !Self::is_supported_video(path) {
			return Err(anyhow::anyhow!(
				"Unsupported video format. Supported: {}",
				Self::supported_extensions().join(", ")
			));
		}

		// Read file and encode to base64
		let video_bytes = std::fs::read(path)?;
		let base64_data = general_purpose::STANDARD.encode(&video_bytes);

		// Determine media type from extension
		let media_type = Self::get_media_type(path)?;

		// Try to get video dimensions using ffprobe if available
		let dimensions = Self::get_video_dimensions(path).ok();

		Ok(VideoAttachment {
			data: VideoData::Base64(base64_data),
			media_type,
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
}
