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

// Session parameter extraction utilities

use crate::config::Config;

// Type alias for extracted session parameters
pub type SessionParams = (
	Option<String>, // name
	Option<String>, // resume
	bool,           // resume_recent
	Option<String>, // model
	Option<u32>,    // max_tokens
	Option<f32>,    // temperature (None = use role config)
	String,         // role
	Option<u32>,    // max_retries (None = use role config)
	String,         // output_mode (plain or jsonl)
);

// Extract session parameters from Debug format with proper fallbacks
pub fn extract_session_params<T: std::fmt::Debug>(args: &T, _config: &Config) -> SessionParams {
	let args_str = format!("{:?}", args);

	// Get model
	let model = if args_str.contains("model: Some(\"") {
		let start = args_str.find("model: Some(\"").unwrap() + 13;
		let end = args_str[start..].find('"').unwrap() + start;
		Some(args_str[start..end].to_string())
	} else {
		None
	};

	// Get name
	let name = if args_str.contains("name: Some(\"") {
		let start = args_str.find("name: Some(\"").unwrap() + 12;
		let end = args_str[start..].find('"').unwrap() + start;
		Some(args_str[start..end].to_string())
	} else {
		None
	};

	// Get resume
	let resume = if args_str.contains("resume: Some(\"") {
		let start = args_str.find("resume: Some(\"").unwrap() + 14;
		let end = args_str[start..].find('"').unwrap() + start;
		Some(args_str[start..end].to_string())
	} else {
		None
	};

	// Get resume_recent
	let resume_recent = args_str.contains("resume_recent: true");

	// Get role
	let role = if args_str.contains("role: \"") {
		let start = args_str.find("role: \"").unwrap() + 7;
		let end = args_str[start..].find('"').unwrap() + start;
		args_str[start..end].to_string()
	} else {
		"developer".to_string() // Default role
	};

	// Get temperature - check if explicitly provided via CLI (now Optional)
	let temperature = if args_str.contains("temperature: Some(") {
		let start = args_str.find("temperature: Some(").unwrap() + 18;
		let end = args_str[start..].find(')').unwrap() + start;
		args_str[start..end].trim().parse::<f32>().ok()
	} else {
		None // No temperature specified, use role config
	};

	// Get max_tokens
	let max_tokens = if args_str.contains("max_tokens: Some(") {
		let start = args_str.find("max_tokens: Some(").unwrap() + 17;
		let end = args_str[start..].find(')').unwrap() + start;
		args_str[start..end].trim().parse::<u32>().ok()
	} else {
		None // No max_tokens specified
	};

	// Get max_retries - check if explicitly provided via CLI (now Optional)
	let max_retries = if args_str.contains("max_retries: Some(") {
		let start = args_str.find("max_retries: Some(").unwrap() + 18;
		let end = args_str[start..].find(')').unwrap() + start;
		args_str[start..end].trim().parse::<u32>().ok()
	} else {
		None // No max_retries specified, use role config
	};

	// Get output_mode - default to plain
	let output_mode = if args_str.contains("mode: \"") {
		let start = args_str.find("mode: \"").unwrap() + 7;
		let end = args_str[start..].find('"').unwrap() + start;
		let mode = args_str[start..end].to_string();
		if mode == "jsonl" {
			"jsonl".to_string()
		} else {
			"plain".to_string()
		}
	} else {
		"plain".to_string()
	};

	(
		name,
		resume,
		resume_recent,
		model,
		max_tokens,
		temperature,
		role,
		max_retries,
		output_mode,
	)
}
