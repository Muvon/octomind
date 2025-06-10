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

// Image processing and attachment utilities

use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use image::{DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Image attachment for messages
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ImageAttachment {
	pub data: ImageData,
	pub media_type: String,
	pub source_type: SourceType,
	pub dimensions: Option<(u32, u32)>,
	pub size_bytes: Option<u64>,
}

/// Image data storage format
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ImageData {
	Base64(String),
	Url(String),
}

/// Source of the image
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SourceType {
	File(PathBuf),
	Clipboard,
	Url,
}

/// Image processing utilities
pub struct ImageProcessor;

impl ImageProcessor {
	/// Maximum dimensions for API transmission (Anthropic limits)
	const MAX_WIDTH: u32 = 1568;
	const MAX_HEIGHT: u32 = 1568;
	const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5MB

	/// Load image from file path
	pub fn load_from_path(path: &Path) -> Result<ImageAttachment> {
		// Check file exists and size
		let metadata = std::fs::metadata(path)?;
		if metadata.len() > Self::MAX_FILE_SIZE {
			return Err(anyhow::anyhow!(
				"Image file too large: {}MB (max 5MB)",
				metadata.len() / 1024 / 1024
			));
		}

		// Load and process image
		let img = image::open(path)?;
		let format = ImageFormat::from_path(path)
			.map_err(|_| anyhow::anyhow!("Unsupported image format"))?;

		let media_type = Self::format_to_media_type(format)?;

		// Resize if needed
		let processed_img = Self::resize_if_needed(img);
		let base64_data = Self::encode_to_base64(&processed_img, format)?;

		Ok(ImageAttachment {
			data: ImageData::Base64(base64_data),
			media_type,
			source_type: SourceType::File(path.to_path_buf()),
			dimensions: Some((processed_img.width(), processed_img.height())),
			size_bytes: Some(metadata.len()),
		})
	}

	/// Load image from URL
	pub async fn load_from_url(url: &str) -> Result<ImageAttachment> {
		use reqwest::Client;

		// Validate URL format
		let parsed_url = url::Url::parse(url).map_err(|_| anyhow::anyhow!("Invalid URL format"))?;

		// Check if URL looks like an image
		if let Some(mut path) = parsed_url.path_segments() {
			if let Some(filename) = path.next_back() {
				if !Self::is_supported_image_by_name(filename) {
					return Err(anyhow::anyhow!(
						"URL does not appear to point to a supported image format: {}",
						filename
					));
				}
			}
		}

		// Download the image
		let client = Client::new();
		let response = client
			.get(url)
			.header("User-Agent", "Octomind/1.0")
			.send()
			.await?;

		if !response.status().is_success() {
			return Err(anyhow::anyhow!(
				"Failed to download image: HTTP {}",
				response.status()
			));
		}

		// Check content type
		let content_type = response
			.headers()
			.get("content-type")
			.and_then(|h| h.to_str().ok())
			.unwrap_or("")
			.to_string(); // Convert to owned String

		if !content_type.starts_with("image/") {
			return Err(anyhow::anyhow!(
				"URL does not return an image (content-type: {})",
				content_type
			));
		}

		// Download image data
		let image_bytes = response.bytes().await?;

		if image_bytes.len() > Self::MAX_FILE_SIZE as usize {
			return Err(anyhow::anyhow!(
				"Image too large: {}MB (max 5MB)",
				image_bytes.len() / 1024 / 1024
			));
		}

		// Load and process image
		let img = image::load_from_memory(&image_bytes)?;

		// Determine format from content-type or URL
		let media_type = if content_type.starts_with("image/") {
			content_type.to_string()
		} else {
			// Fallback to URL extension
			Self::guess_media_type_from_url(url).unwrap_or_else(|| "image/png".to_string())
		};

		// Resize if needed
		let processed_img = Self::resize_if_needed(img);

		// Convert to appropriate format for encoding
		let format = Self::media_type_to_format(&media_type)?;
		let base64_data = Self::encode_to_base64(&processed_img, format)?;

		Ok(ImageAttachment {
			data: ImageData::Base64(base64_data),
			media_type,
			source_type: SourceType::Url,
			dimensions: Some((processed_img.width(), processed_img.height())),
			size_bytes: Some(image_bytes.len() as u64),
		})
	}

	/// Load image from clipboard
	pub fn load_from_clipboard() -> Result<Option<ImageAttachment>> {
		use arboard::Clipboard;

		let mut clipboard =
			Clipboard::new().map_err(|_| anyhow::anyhow!("Failed to access clipboard"))?;

		match clipboard.get_image() {
			Ok(img_data) => {
				let attachment = Self::convert_clipboard_image(img_data)?;
				Ok(Some(attachment))
			}
			Err(_) => Ok(None), // No image in clipboard
		}
	}

	/// Convert clipboard image data to attachment
	fn convert_clipboard_image(img_data: arboard::ImageData) -> Result<ImageAttachment> {
		// Convert RGBA bytes to DynamicImage
		let width = img_data.width;
		let height = img_data.height;
		let bytes = img_data.bytes;

		let img = image::RgbaImage::from_raw(width as u32, height as u32, bytes.into_owned())
			.ok_or_else(|| anyhow::anyhow!("Failed to create image from clipboard data"))?;

		let dynamic_img = DynamicImage::ImageRgba8(img);
		let processed_img = Self::resize_if_needed(dynamic_img);

		// Encode as PNG for clipboard images
		let base64_data = Self::encode_to_base64(&processed_img, ImageFormat::Png)?;

		Ok(ImageAttachment {
			data: ImageData::Base64(base64_data),
			media_type: "image/png".to_string(),
			source_type: SourceType::Clipboard,
			dimensions: Some((processed_img.width(), processed_img.height())),
			size_bytes: None, // Unknown for clipboard
		})
	}

	/// Resize image if it exceeds maximum dimensions
	fn resize_if_needed(img: DynamicImage) -> DynamicImage {
		let (width, height) = (img.width(), img.height());

		if width <= Self::MAX_WIDTH && height <= Self::MAX_HEIGHT {
			return img;
		}

		// Calculate new dimensions maintaining aspect ratio
		let ratio =
			(Self::MAX_WIDTH as f32 / width as f32).min(Self::MAX_HEIGHT as f32 / height as f32);

		let new_width = (width as f32 * ratio) as u32;
		let new_height = (height as f32 * ratio) as u32;

		img.resize(new_width, new_height, image::imageops::FilterType::Lanczos3)
	}

	/// Encode image to base64
	fn encode_to_base64(img: &DynamicImage, format: ImageFormat) -> Result<String> {
		let mut buffer = Vec::new();
		img.write_to(&mut std::io::Cursor::new(&mut buffer), format)?;
		Ok(general_purpose::STANDARD.encode(&buffer))
	}

	/// Convert ImageFormat to MIME type
	fn format_to_media_type(format: ImageFormat) -> Result<String> {
		let media_type = match format {
			ImageFormat::Png => "image/png",
			ImageFormat::Jpeg => "image/jpeg",
			ImageFormat::Gif => "image/gif",
			ImageFormat::WebP => "image/webp",
			ImageFormat::Bmp => "image/bmp",
			_ => return Err(anyhow::anyhow!("Unsupported image format for vision API")),
		};
		Ok(media_type.to_string())
	}

	/// Show image preview in terminal
	pub fn show_preview(attachment: &ImageAttachment) -> Result<()> {
		if let ImageData::Base64(ref data) = attachment.data {
			let img_bytes = general_purpose::STANDARD.decode(data)?;
			let img = image::load_from_memory(&img_bytes)?;

			// Show metadata
			if let Some((width, height)) = attachment.dimensions {
				println!("📸 Image: {}x{} ({})", width, height, attachment.media_type);
			}

			if let Some(size) = attachment.size_bytes {
				println!("📏 Size: {:.1}KB", size as f64 / 1024.0);
			}

			// Display small preview using viuer
			let config = viuer::Config {
				width: Some(40),
				height: Some(20),
				absolute_offset: false,
				..Default::default()
			};

			if let Err(e) = viuer::print(&img, &config) {
				println!("⚠️  Preview not available: {}", e);
			}
		}
		Ok(())
	}

	/// Check if file is a supported image format
	pub fn is_supported_image(path: &Path) -> bool {
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

	/// Check if filename has supported image extension
	pub fn is_supported_image_by_name(filename: &str) -> bool {
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
			"png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
		)
	}

	/// Guess media type from URL
	fn guess_media_type_from_url(url: &str) -> Option<String> {
		if let Some(ext) = url.split('.').next_back() {
			match ext.to_lowercase().as_str() {
				"png" => Some("image/png".to_string()),
				"jpg" | "jpeg" => Some("image/jpeg".to_string()),
				"gif" => Some("image/gif".to_string()),
				"webp" => Some("image/webp".to_string()),
				"bmp" => Some("image/bmp".to_string()),
				_ => None,
			}
		} else {
			None
		}
	}

	/// Convert media type to ImageFormat
	fn media_type_to_format(media_type: &str) -> Result<ImageFormat> {
		match media_type {
			"image/png" => Ok(ImageFormat::Png),
			"image/jpeg" => Ok(ImageFormat::Jpeg),
			"image/gif" => Ok(ImageFormat::Gif),
			"image/webp" => Ok(ImageFormat::WebP),
			"image/bmp" => Ok(ImageFormat::Bmp),
			_ => Ok(ImageFormat::Png), // Default to PNG
		}
	}

	/// Get supported image extensions for autocomplete
	pub fn supported_extensions() -> &'static [&'static str] {
		&["png", "jpg", "jpeg", "gif", "webp", "bmp"]
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
		let extensions = ImageProcessor::supported_extensions();
		assert!(extensions.contains(&"png"));
		assert!(extensions.contains(&"jpg"));
	}

	#[test]
	fn test_is_supported_image() {
		assert!(ImageProcessor::is_supported_image(Path::new("test.png")));
		assert!(ImageProcessor::is_supported_image(Path::new("test.JPG")));
		assert!(!ImageProcessor::is_supported_image(Path::new("test.txt")));
	}
}
