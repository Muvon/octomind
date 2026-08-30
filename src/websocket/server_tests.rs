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

fn handshake(origin: Option<&str>, allow_origins: &[&str]) -> Result<Response, Box<ErrorResponse>> {
	let mut req = Request::builder().uri("/");
	if let Some(origin) = origin {
		req = req.header(ORIGIN, origin);
	}
	let allowlist = allow_origins.iter().map(|o| (*o).to_string()).collect();
	OriginAllowlist(Arc::new(allowlist))
		.on_request(&req.body(()).unwrap(), Response::new(()))
		.map_err(Box::new)
}

#[test]
fn native_clients_send_no_origin_and_are_allowed() {
	assert!(handshake(None, &[]).is_ok());
}

#[test]
fn listed_origin_is_allowed() {
	assert!(handshake(Some("http://localhost:3000"), &["http://localhost:3000"]).is_ok());
}

#[test]
fn unlisted_origin_is_refused() {
	// A different port is a different origin — this is the drive-by browser case.
	let err = handshake(Some("http://localhost:3001"), &["http://localhost:3000"]).unwrap_err();
	assert_eq!(err.status(), StatusCode::FORBIDDEN);
}

#[test]
fn empty_allowlist_refuses_every_browser() {
	let err = handshake(Some("https://evil.example.com"), &[]).unwrap_err();
	assert_eq!(err.status(), StatusCode::FORBIDDEN);
}

fn image_attachment() -> Attachment {
	Attachment {
		id: "AbCdEf0123456789GhIjKlMn".to_string(),
		kind: AttachmentKind::Image,
		media_type: "image/png".to_string(),
		name: "screenshot.png".to_string(),
		size: 1234,
	}
}

#[test]
fn known_non_vision_model_refuses_websocket_image_before_file_access() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openai:gpt-3.5-turbo".to_string();
	let missing_root = Path::new("/definitely/missing/media/root");

	let error = load_message_attachments(&session, &[image_attachment()], missing_root)
		.expect_err("known text-only model must refuse image before resolving the file");
	assert!(error.to_string().contains("openai:gpt-3.5-turbo"));
	assert!(error.to_string().contains("does not support vision"));
	assert!(!error.to_string().contains("missing or unreadable"));
}

#[test]
fn prefixed_websocket_image_is_attached_to_empty_user_turn() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let attachment = image_attachment();
	// The writer stores media as `<id>.<ext>`; resolve_path locates it by
	// prefix, so the fixture must be laid out the same way.
	let media_path = tmp.path().join(format!("{}.png", attachment.id));
	image::RgbImage::new(4, 4)
		.save(&media_path)
		.expect("save test image");

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/unknown-vision-model".to_string();
	let loaded = load_message_attachments(&session, &[attachment], tmp.path())
		.expect("load websocket attachment");
	session
		.add_user_message_with_attachments("", loaded.images, loaded.videos)
		.expect("add attachment-only user turn");

	let message = session.session.messages.last().expect("user message");
	assert_eq!(message.content, "");
	assert_eq!(message.images.as_ref().map(Vec::len), Some(1));
	assert!(message.videos.is_none());
}

#[test]
fn attachment_with_no_matching_file_is_reported_as_not_found() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let attachment = image_attachment();

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/unknown-vision-model".to_string();
	let error = load_message_attachments(&session, std::slice::from_ref(&attachment), tmp.path())
		.expect_err("no file on disk must be reported as not found");
	assert!(error.to_string().contains("not found"));
	assert!(error.to_string().contains(&attachment.id));
}
