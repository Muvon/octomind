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

// HTML to Markdown converter module

use super::super::{McpToolCall, McpToolResult};
use anyhow::{anyhow, Result};
use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use reqwest;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs as tokio_fs;
use url::Url;

// Execute HTML to Markdown conversion
pub async fn execute_html2md(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract sources parameter
	let sources_value = match call.parameters.get("sources") {
		Some(value) => value,
		_ => return Err(anyhow!("Missing 'sources' parameter")),
	};

	// Support either a single source string or an array of sources
	match sources_value {
		Value::String(source) => {
			// Single source conversion
			convert_single_html_to_md(call, source).await
		}
		Value::Array(sources) => {
			// Multiple sources conversion
			let source_strings: Result<Vec<String>, _> = sources
				.iter()
				.map(|s| s.as_str().ok_or_else(|| anyhow!("Invalid source in array")))
				.map(|r| r.map(|s| s.to_string()))
				.collect();

			match source_strings {
				Ok(source_strs) => convert_multiple_html_to_md(call, &source_strs).await,
				Err(e) => Err(e),
			}
		}
		_ => Err(anyhow!(
			"'sources' parameter must be a string or array of strings"
		)),
	}
}

// Convert a single HTML source to Markdown
async fn convert_single_html_to_md(call: &McpToolCall, source: &str) -> Result<McpToolResult> {
	let (html_content, source_type) = fetch_html_content(source).await?;
	let markdown = html_to_markdown(&html_content)?;

	Ok(McpToolResult {
		tool_name: "html2md".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
				"success": true,
				"conversions": [{
				"source": source,
				"type": source_type,
				"markdown": markdown,
				"size": markdown.len()
			}],
			"count": 1
		}),
	})
}

// Convert multiple HTML sources to Markdown
async fn convert_multiple_html_to_md(
	call: &McpToolCall,
	sources: &[String],
) -> Result<McpToolResult> {
	let mut conversions = Vec::with_capacity(sources.len());
	let mut failures = Vec::new();

	for source in sources {
		match fetch_html_content(source).await {
			Ok((html_content, source_type)) => match html_to_markdown(&html_content) {
				Ok(markdown) => {
					conversions.push(json!({
						"source": source,
						"type": source_type,
						"markdown": markdown,
						"size": markdown.len()
					}));
				}
				Err(e) => {
					failures.push(format!("Failed to convert {} to markdown: {}", source, e));
				}
			},
			Err(e) => {
				failures.push(format!("Failed to fetch {}: {}", source, e));
			}
		}
	}

	Ok(McpToolResult {
		tool_name: "html2md".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": !conversions.is_empty(),
			"conversions": conversions,
			"count": conversions.len(),
			"failed": failures
		}),
	})
}

// Fetch HTML content from URL or local file
async fn fetch_html_content(source: &str) -> Result<(String, &'static str)> {
	// Check if source is a URL or file path
	if let Ok(url) = Url::parse(source) {
		if url.scheme() == "http" || url.scheme() == "https" {
			// Fetch from URL
			let response = reqwest::get(source).await?;
			if !response.status().is_success() {
				return Err(anyhow!("HTTP error {}: {}", response.status(), source));
			}
			let html = response.text().await?;
			Ok((html, "url"))
		} else if url.scheme() == "file" {
			// Handle file:// URLs
			let path = url
				.to_file_path()
				.map_err(|_| anyhow!("Invalid file URL: {}", source))?;
			let html = tokio_fs::read_to_string(&path).await?;
			Ok((html, "file"))
		} else {
			Err(anyhow!("Unsupported URL scheme: {}", url.scheme()))
		}
	} else {
		// Treat as file path
		let path = Path::new(source);
		if !path.exists() {
			return Err(anyhow!("File does not exist: {}", source));
		}
		if !path.is_file() {
			return Err(anyhow!("Path is not a file: {}", source));
		}
		let html = tokio_fs::read_to_string(path).await?;
		Ok((html, "file"))
	}
}

// Convert HTML to Markdown using html5ever parser
fn html_to_markdown(html: &str) -> Result<String> {
	let dom = parse_document(RcDom::default(), Default::default())
		.from_utf8()
		.read_from(&mut html.as_bytes())?;

	let mut markdown = String::new();
	walk_node(&dom.document, &mut markdown, 0)?;

	// Clean up the markdown
	let cleaned = clean_markdown(&markdown);
	Ok(cleaned)
}

// Recursively walk the DOM tree and convert to Markdown
fn walk_node(handle: &Handle, markdown: &mut String, depth: usize) -> Result<()> {
	let node = handle;
	match &node.data {
		NodeData::Document => {
			// Process children
			for child in node.children.borrow().iter() {
				walk_node(child, markdown, depth)?;
			}
		}
		NodeData::Element { name, attrs, .. } => {
			let tag_name = &name.local;
			let attrs = attrs.borrow();

			match tag_name.as_ref() {
				"h1" => {
					markdown.push_str("\n# ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				}
				"h2" => {
					markdown.push_str("\n## ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				}
				"h3" => {
					markdown.push_str("\n### ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				}
				"h4" => {
					markdown.push_str("\n#### ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				}
				"h5" => {
					markdown.push_str("\n##### ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				}
				"h6" => {
					markdown.push_str("\n###### ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				}
				"p" => {
					markdown.push('\n');
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				}
				"strong" | "b" => {
					markdown.push_str("**");
					process_children(node, markdown, depth)?;
					markdown.push_str("**");
				}
				"em" | "i" => {
					markdown.push('*');
					process_children(node, markdown, depth)?;
					markdown.push('*');
				}
				"code" => {
					markdown.push('`');
					process_children(node, markdown, depth)?;
					markdown.push('`');
				}
				"pre" => {
					markdown.push_str("\n```\n");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n```\n\n");
				}
				"a" => {
					// Find href attribute
					let href = attrs
						.iter()
						.find(|attr| &*attr.name.local == "href")
						.map(|attr| attr.value.to_string());

					if let Some(url) = href {
						markdown.push('[');
						process_children(node, markdown, depth)?;
						markdown.push_str(&format!("]({})", url));
					} else {
						process_children(node, markdown, depth)?;
					}
				}
				"ul" => {
					markdown.push('\n');
					process_children(node, markdown, depth)?;
					markdown.push('\n');
				}
				"ol" => {
					markdown.push('\n');
					process_children(node, markdown, depth)?;
					markdown.push('\n');
				}
				"li" => {
					if depth > 0 {
						for _ in 0..(depth - 1) {
							markdown.push_str("  ");
						}
					}
					markdown.push_str("- ");
					process_children(node, markdown, depth + 1)?;
					markdown.push('\n');
				}
				"blockquote" => {
					markdown.push_str("\n> ");
					process_children(node, markdown, depth)?;
					markdown.push_str("\n\n");
				}
				"br" => {
					markdown.push_str("  \n");
				}
				"hr" => {
					markdown.push_str("\n---\n\n");
				}
				"img" => {
					// Find src and alt attributes
					let src = attrs
						.iter()
						.find(|attr| &*attr.name.local == "src")
						.map(|attr| attr.value.to_string());
					let alt = attrs
						.iter()
						.find(|attr| &*attr.name.local == "alt")
						.map(|attr| attr.value.to_string())
						.unwrap_or_else(|| "".to_string());

					if let Some(url) = src {
						markdown.push_str(&format!("![{}]({})", alt, url));
					}
				}
				// Skip common non-content elements
				"script" | "style" | "head" | "meta" | "link" | "title" => {
					// Don't process children of these elements
				}
				// For all other elements, just process children
				_ => {
					process_children(node, markdown, depth)?;
				}
			}
		}
		NodeData::Text { contents } => {
			let text = contents.borrow().to_string();
			// Clean up whitespace in text nodes
			let cleaned_text = text.trim();
			if !cleaned_text.is_empty() {
				markdown.push_str(cleaned_text);
			}
		}
		_ => {
			// For other node types (comments, etc.), process children
			for child in node.children.borrow().iter() {
				walk_node(child, markdown, depth)?;
			}
		}
	}
	Ok(())
}

// Helper function to process children of a node
fn process_children(node: &Handle, markdown: &mut String, depth: usize) -> Result<()> {
	for child in node.children.borrow().iter() {
		walk_node(child, markdown, depth)?;
	}
	Ok(())
}

// Clean up the generated Markdown
fn clean_markdown(markdown: &str) -> String {
	let mut lines: Vec<&str> = markdown.lines().collect();

	// Remove leading and trailing empty lines
	while let Some(&first) = lines.first() {
		if first.trim().is_empty() {
			lines.remove(0);
		} else {
			break;
		}
	}

	while let Some(&last) = lines.last() {
		if last.trim().is_empty() {
			lines.pop();
		} else {
			break;
		}
	}

	// Collapse multiple consecutive empty lines into at most two
	let mut result = Vec::new();
	let mut empty_count = 0;

	for line in lines {
		if line.trim().is_empty() {
			empty_count += 1;
			if empty_count <= 2 {
				result.push(line);
			}
		} else {
			empty_count = 0;
			result.push(line);
		}
	}

	result.join("\n")
}
