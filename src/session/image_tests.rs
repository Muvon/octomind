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
	assert!(!ImageProcessor::is_supported_image_by_name("notes.md"));

	assert!(!ImageProcessor::supported_extensions().is_empty());

	assert!(ImageProcessor::is_url("https://example.com/x.png"));
	assert!(ImageProcessor::is_url("http://example.com/x.png"));
	assert!(!ImageProcessor::is_url("./local/x.png"));
}
