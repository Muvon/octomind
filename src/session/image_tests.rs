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

//! Attachment-loading tests against real image files generated on the fly:
//! format detection, base64 payload production, and the rejection arms
//! (missing file, unsupported format).

use super::*;

fn write_test_image(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
	let path = dir.join(name);
	let img = image::RgbImage::from_fn(4, 4, |x, y| {
		image::Rgb([(x * 60) as u8, (y * 60) as u8, 128])
	});
	img.save(&path).expect("save test image");
	path
}

#[test]
fn test_load_from_path_png_and_jpeg() {
	let tmp = tempfile::tempdir().expect("tempdir");

	let png = write_test_image(tmp.path(), "tiny.png");
	let attachment = ImageProcessor::load_from_path(&png).expect("load png");
	assert_eq!(attachment.media_type, "image/png");
	assert_eq!(attachment.dimensions, Some((4, 4)));
	match &attachment.data {
		ImageData::Base64(b64) => assert!(!b64.is_empty()),
		ImageData::Url(u) => panic!("file load must produce base64, got url {u}"),
	}

	let jpg = write_test_image(tmp.path(), "tiny.jpg");
	let attachment = ImageProcessor::load_from_path(&jpg).expect("load jpeg");
	assert_eq!(attachment.media_type, "image/jpeg");
}

#[test]
fn test_load_from_extensionless_media_id() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let png = write_test_image(tmp.path(), "source.png");
	let media_id = tmp.path().join("AbCdEf0123456789GhIjKlMn");
	std::fs::rename(png, &media_id).expect("rename to opaque media id");

	let attachment = ImageProcessor::load_from_path(&media_id).expect("load opaque media id");
	assert_eq!(attachment.media_type, "image/png");
	assert_eq!(attachment.dimensions, Some((4, 4)));
}

#[test]
fn test_load_rejections() {
	let tmp = tempfile::tempdir().expect("tempdir");

	// Missing file
	assert!(ImageProcessor::load_from_path(&tmp.path().join("absent.png")).is_err());

	// Present but not an image
	let not_image = tmp.path().join("notes.png");
	std::fs::write(&not_image, "this is text, not pixels").expect("write");
	assert!(ImageProcessor::load_from_path(&not_image).is_err());
}

#[test]
fn test_support_predicates() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let png = write_test_image(tmp.path(), "ok.png");
	assert!(ImageProcessor::is_supported_image(&png));
	assert!(!ImageProcessor::is_supported_image(std::path::Path::new(
		"/tmp/whatever.txt"
	)));

	assert!(ImageProcessor::is_supported_image_by_name("shot.PNG"));
	assert!(ImageProcessor::is_supported_image_by_name("photo.webp"));
	assert!(!ImageProcessor::is_supported_image_by_name("legacy.bmp"));
	assert!(!ImageProcessor::is_supported_image_by_name("notes.md"));

	assert!(!ImageProcessor::supported_extensions().contains(&"bmp"));

	assert!(ImageProcessor::is_url("https://example.com/x.png"));
	assert!(ImageProcessor::is_url("http://example.com/x.png"));
	assert!(!ImageProcessor::is_url("./local/x.png"));
}

#[tokio::test]
async fn test_known_non_vision_model_refuses_image_attach_with_model_name() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let png = write_test_image(tmp.path(), "known-text-only.png");
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.model = "openai:gpt-3.5-turbo".to_string();

	let error = session
		.attach_image_from_path(png.to_str().expect("utf-8 path"))
		.await
		.expect_err("known text-only model must refuse image");
	assert!(error.to_string().contains("openai:gpt-3.5-turbo"));
	assert!(error.to_string().contains("does not support vision"));
	assert!(!session.has_pending_image());
}

#[tokio::test]
async fn test_unknown_proxy_model_still_attaches_image() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let png = write_test_image(tmp.path(), "unknown-model.png");
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/totally-unknown-model-xyz".to_string();

	session
		.attach_image_from_path(png.to_str().expect("utf-8 path"))
		.await
		.expect("unknown proxy model must remain permissive");
	assert!(session.has_pending_image());
}

#[test]
fn test_known_non_video_model_refuses_video_attach_with_model_name() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.model = "openai:gpt-3.5-turbo".to_string();

	let error = session
		.ensure_model_supports_video()
		.expect_err("known non-video model must refuse video");
	assert!(error.to_string().contains("openai:gpt-3.5-turbo"));
	assert!(error.to_string().contains("does not support video"));
}

#[test]
fn test_unknown_proxy_model_remains_permissive_for_video() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/totally-unknown-model-xyz".to_string();

	session
		.ensure_model_supports_video()
		.expect("unknown proxy model must remain permissive for video");
}

// --- URL loading against a local HTTP server ---

async fn serve_image(
	status: &'static str,
	content_type: &'static str,
	path: &'static str,
	body: Vec<u8>,
) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind");
	let addr = listener.local_addr().expect("addr");
	tokio::spawn(async move {
		use tokio::io::{AsyncReadExt, AsyncWriteExt};
		let (mut socket, _) = listener.accept().await.expect("accept");
		let mut buf = [0u8; 2048];
		let _ = socket.read(&mut buf).await;
		let header = format!(
			"HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
			body.len()
		);
		socket
			.write_all(header.as_bytes())
			.await
			.expect("write head");
		socket.write_all(&body).await.expect("write body");
	});
	format!("http://{addr}{path}")
}

fn png_bytes() -> Vec<u8> {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = write_test_image(tmp.path(), "payload.png");
	std::fs::read(path).expect("read png")
}

#[tokio::test]
async fn test_load_from_url_success() {
	let url = serve_image("200 OK", "image/png", "/pic.png", png_bytes()).await;
	let attachment = ImageProcessor::load_from_url(&url)
		.await
		.expect("load from url");
	assert_eq!(attachment.media_type, "image/png");
	assert!(matches!(attachment.source_type, SourceType::Url));
	assert_eq!(attachment.dimensions, Some((4, 4)));
	assert!(matches!(attachment.data, ImageData::Base64(_)));
}

#[tokio::test]
async fn test_load_from_url_rejections() {
	// Invalid URL
	let err = ImageProcessor::load_from_url("not a url")
		.await
		.expect_err("invalid url");
	assert!(err.to_string().contains("Invalid URL"));

	// Filename does not look like an image
	let url = serve_image("200 OK", "image/png", "/file.txt", png_bytes()).await;
	let err = ImageProcessor::load_from_url(&url)
		.await
		.expect_err("non-image name");
	assert!(err.to_string().contains("does not appear to point"));

	// HTTP error status
	let url = serve_image("404 Not Found", "image/png", "/pic.png", Vec::new()).await;
	let err = ImageProcessor::load_from_url(&url)
		.await
		.expect_err("http error");
	assert!(err.to_string().contains("HTTP 404"));

	// Non-image content type
	let url = serve_image("200 OK", "text/plain", "/pic.png", b"hello".to_vec()).await;
	let err = ImageProcessor::load_from_url(&url)
		.await
		.expect_err("wrong content type");
	assert!(err.to_string().contains("does not return an image"));

	// Oversized download (checked before decoding)
	let url = serve_image(
		"200 OK",
		"image/png",
		"/pic.png",
		vec![0u8; 6 * 1024 * 1024],
	)
	.await;
	let err = ImageProcessor::load_from_url(&url)
		.await
		.expect_err("too large");
	assert!(err.to_string().contains("too large"));
}

#[test]
fn test_oversized_file_rejected_before_decode() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let big = tmp.path().join("big.png");
	std::fs::write(&big, vec![0u8; 5 * 1024 * 1024 + 1]).expect("write big");
	let err = ImageProcessor::load_from_path(&big).expect_err("must reject >5MB");
	assert!(err.to_string().contains("too large"));
}

#[test]
fn test_resize_if_needed_shrinks_oversized_dimensions() {
	let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(2000, 100, |_, _| {
		image::Rgb([1, 2, 3])
	}));
	let resized = ImageProcessor::resize_if_needed(img);
	assert!(resized.width() <= ImageProcessor::MAX_WIDTH);
	assert!(resized.height() <= ImageProcessor::MAX_HEIGHT);
	assert_eq!(resized.height(), 78);
}

#[test]
fn test_format_to_media_type_rejects_unsupported() {
	assert!(ImageProcessor::format_to_media_type(ImageFormat::Bmp).is_err());
	assert!(ImageProcessor::media_type_to_format("image/bmp").is_err());
	assert!(ImageProcessor::media_type_to_format("image/gif").is_ok());
}

#[test]
fn test_convert_clipboard_image_encodes_png() {
	// 2x2 RGBA clipboard buffer
	let rgba: Vec<u8> = vec![
		255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
	];
	let img_data = arboard::ImageData {
		width: 2,
		height: 2,
		bytes: std::borrow::Cow::from(rgba),
	};
	let attachment = ImageProcessor::convert_clipboard_image(img_data).expect("convert");
	assert_eq!(attachment.media_type, "image/png");
	assert!(matches!(attachment.source_type, SourceType::Clipboard));
	assert_eq!(attachment.dimensions, Some((2, 2)));
	assert!(matches!(attachment.data, ImageData::Base64(_)));
}

#[test]
fn test_load_from_clipboard_without_image_is_not_an_error() {
	// Headless environments cannot open the clipboard (Err); graphical ones
	// with an empty clipboard yield Ok(None). Both are acceptable.
	let result = ImageProcessor::load_from_clipboard();
	assert!(result.is_err() || result.as_ref().unwrap().is_none());
}

#[test]
fn test_show_preview_prints_metadata_without_failing() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let png = write_test_image(tmp.path(), "preview.png");
	let attachment = ImageProcessor::load_from_path(&png).expect("load");
	ImageProcessor::show_preview(&attachment).expect("preview must not fail");
}

#[test]
#[serial_test::serial]
fn test_render_inline_escape_selects_protocol_by_terminal() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let png = write_test_image(tmp.path(), "inline.png");
	let attachment = ImageProcessor::load_from_path(&png).expect("load");

	// No inline-graphics terminal → None
	std::env::remove_var("KITTY_WINDOW_ID");
	std::env::remove_var("TERM");
	std::env::remove_var("TERM_PROGRAM");
	assert!(ImageProcessor::render_inline_escape(&attachment).is_none());

	// Kitty graphics via TERM_PROGRAM=ghostty
	std::env::set_var("TERM_PROGRAM", "ghostty");
	let (escape, rows) = ImageProcessor::render_inline_escape(&attachment).expect("kitty");
	assert!(escape.starts_with("\x1b_Ga=T,f=100,q=2,c=40,r="));
	assert!(escape.contains("m=0;"));
	assert!((1..=30).contains(&rows));

	// iTerm2 OSC 1337 via TERM_PROGRAM=vscode
	std::env::set_var("TERM_PROGRAM", "vscode");
	let (escape, rows) = ImageProcessor::render_inline_escape(&attachment).expect("iterm2");
	assert!(escape.starts_with("\x1b]1337;File=inline=1;width=40;height="));
	assert!(escape.ends_with("\x07"));
	assert!((1..=30).contains(&rows));

	// Kitty via TERM containing "kitty"
	std::env::remove_var("TERM_PROGRAM");
	std::env::set_var("TERM", "xterm-kitty");
	let (escape, _) = ImageProcessor::render_inline_escape(&attachment).expect("kitty via TERM");
	assert!(escape.starts_with("\x1b_G"));

	std::env::remove_var("TERM");
}

#[test]
fn test_build_kitty_escape_splits_long_payloads_into_chunks() {
	let escape = ImageProcessor::build_kitty_escape(&"A".repeat(5000), 40, 10);
	// 5000 bytes → two escapes: the first carries metadata + m=1, the last m=0
	assert_eq!(escape.matches("\x1b_G").count(), 2);
	assert!(escape.contains("m=1;"));
	assert!(escape.contains("m=0;"));
}

#[test]
fn test_shrink_for_preview_resizes_large_images_only() {
	let small_b64 = {
		let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(100, 50, |_, _| {
			image::Rgb([9, 9, 9])
		}));
		let mut buf = Vec::new();
		img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)
			.expect("encode");
		base64::engine::general_purpose::STANDARD.encode(&buf)
	};
	let shrunk = ImageProcessor::shrink_for_preview(&small_b64).expect("shrink small");
	assert!(!shrunk.is_empty());

	let big_b64 = {
		let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(400, 400, |_, _| {
			image::Rgb([7, 7, 7])
		}));
		let mut buf = Vec::new();
		img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)
			.expect("encode");
		base64::engine::general_purpose::STANDARD.encode(&buf)
	};
	let shrunk = ImageProcessor::shrink_for_preview(&big_b64).expect("shrink big");
	let bytes = base64::engine::general_purpose::STANDARD
		.decode(shrunk)
		.expect("valid b64");
	let img = image::load_from_memory(&bytes).expect("valid image");
	assert!(img.width() <= 320 && img.height() <= 320);
}

#[test]
fn test_support_predicates_edge_cases() {
	// Non-UTF-8 extension, missing extension, extensionless names
	assert!(!ImageProcessor::is_supported_image(std::path::Path::new(
		"/tmp/pic.\u{FF}"
	)));
	assert!(!ImageProcessor::is_supported_image(std::path::Path::new(
		"/tmp/README"
	)));
	assert!(!ImageProcessor::is_supported_image_by_name("noext"));
	assert!(ImageProcessor::guess_media_type_from_url("no-dot").is_none());
	assert_eq!(
		ImageProcessor::guess_media_type_from_url("https://x/a.PNG"),
		Some("image/png".to_string())
	);
}
