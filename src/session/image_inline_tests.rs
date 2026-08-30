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

#[test]
fn resize_encoding_and_format_helpers_cover_boundaries() {
	let original = DynamicImage::new_rgb8(2000, 1000);
	let resized = ImageProcessor::resize_if_needed(original);
	assert_eq!((resized.width(), resized.height()), (1568, 784));

	let small = DynamicImage::new_rgb8(8, 4);
	let encoded = ImageProcessor::encode_to_base64(&small, ImageFormat::Png).unwrap();
	let shrunk = ImageProcessor::shrink_for_preview(&encoded).unwrap();
	assert!(!shrunk.is_empty());
	assert!(ImageProcessor::shrink_for_preview("not-base64").is_err());

	for (format, media) in [
		(ImageFormat::Png, "image/png"),
		(ImageFormat::Jpeg, "image/jpeg"),
		(ImageFormat::Gif, "image/gif"),
		(ImageFormat::WebP, "image/webp"),
	] {
		assert_eq!(ImageProcessor::format_to_media_type(format).unwrap(), media);
		assert_eq!(ImageProcessor::media_type_to_format(media).unwrap(), format);
	}
	assert!(ImageProcessor::format_to_media_type(ImageFormat::Bmp).is_err());
	assert!(ImageProcessor::media_type_to_format("image/bmp").is_err());
	assert_eq!(
		ImageProcessor::guess_media_type_from_url("https://x/photo.JPEG").as_deref(),
		Some("image/jpeg")
	);
	assert!(ImageProcessor::guess_media_type_from_url("https://x/file.bin").is_none());
}

#[test]
fn inline_escape_builders_chunk_and_size_the_preview() {
	let payload = "a".repeat(9000);
	let kitty = ImageProcessor::build_kitty_escape(&payload, 40, 10);
	assert!(kitty.starts_with("\u{1b}_Ga=T,f=100,q=2,c=40,r=10,m=1;"));
	assert!(kitty.contains("\u{1b}_Gm=0;"));
	let iterm = ImageProcessor::build_iterm2_escape("YWJj", 40, 7);
	assert!(iterm.contains("width=40;height=7"));

	let url_attachment = ImageAttachment {
		data: ImageData::Url("https://example.com/a.png".into()),
		media_type: "image/png".into(),
		source_type: SourceType::Url,
		dimensions: None,
		size_bytes: None,
	};
	assert!(ImageProcessor::render_inline_escape(&url_attachment).is_none());
}

async fn image_server(status: &str, content_type: &str, body: Vec<u8>) -> String {
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
	format!("http://{addr}/tiny.png")
}

#[tokio::test]
async fn load_from_url_covers_success_status_and_content_type_errors() {
	let mut png = Vec::new();
	DynamicImage::new_rgb8(3, 2)
		.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
		.unwrap();
	let url = image_server("200 OK", "image/png", png).await;
	let attachment = ImageProcessor::load_from_url(&url).await.unwrap();
	assert_eq!(attachment.dimensions, Some((3, 2)));
	assert_eq!(attachment.media_type, "image/png");

	let url = image_server("404 Not Found", "image/png", Vec::new()).await;
	assert!(ImageProcessor::load_from_url(&url).await.is_err());
	let url = image_server("200 OK", "text/plain", b"no".to_vec()).await;
	assert!(ImageProcessor::load_from_url(&url).await.is_err());
	assert!(ImageProcessor::load_from_url("not a url").await.is_err());
	assert!(
		ImageProcessor::load_from_url("https://example.com/file.txt")
			.await
			.is_err()
	);
}
