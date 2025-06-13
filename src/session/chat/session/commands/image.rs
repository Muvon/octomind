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

// Image command handler

use super::super::core::ChatSession;
use anyhow::Result;
use colored::Colorize;

pub async fn handle_image(session: &mut ChatSession, params: &[&str]) -> Result<bool> {
	// Handle /image command for attaching images
	if params.is_empty() {
		println!("{}", "Usage: /image <path_to_image_or_url>".bright_yellow());
		println!("{}", "Examples:".bright_blue());
		println!("{}", "  /image screenshot.png".bright_white());
		println!("{}", "  /image /path/to/image.jpg".bright_white());
		println!(
			"{}",
			"  /image https://example.com/image.png".bright_white()
		);
		println!(
			"{}",
			"Supported formats: PNG, JPEG, GIF, WebP, BMP".bright_blue()
		);

		// Check if current model supports vision
		let (provider, model_name) =
			match crate::session::providers::ProviderFactory::get_provider_for_model(&session.model)
			{
				Ok((provider, model)) => (provider, model),
				Err(_) => {
					println!(
						"{}",
						"Unable to check vision support for current model".bright_red()
					);
					return Ok(false);
				}
			};

		if provider.supports_vision(&model_name) {
			println!("{}", "✅ Current model supports vision".bright_green());
		} else {
			println!(
				"{}",
				"⚠️  Current model does not support vision".bright_yellow()
			);
		}

		// Check clipboard for images
		if let Ok(true) = session.try_attach_from_clipboard().await {
			// Image was found and attached from clipboard
			return Ok(false);
		} else {
			println!(
				"{}",
				"💡 Tip: Copy an image to clipboard and run /image to auto-attach it".bright_blue()
			);
		}

		return Ok(false);
	}

	let image_path = params.join(" ");
	match session.attach_image_from_path(&image_path).await {
		Ok(_) => {
			println!("{}", "✅ Image attached successfully!".bright_green());
			println!(
				"{}",
				"Your next message will include this image.".bright_cyan()
			);
		}
		Err(e) => {
			println!("{}: {}", "❌ Failed to attach image".bright_red(), e);
		}
	}
	Ok(false)
}
