// Semantic code analysis functionality for the Developer MCP provider

use serde_json::{json, Value};
use anyhow::{Result, anyhow};
use super::super::{McpToolCall, McpToolResult, McpFunction};

// Define the semantic_code_function for signatures and search modes
pub fn get_semantic_code_function() -> McpFunction {
	McpFunction {
		name: "semantic_code".to_string(),
		description: "Analyze and search code in the repository using both structural and semantic methods by looking into code blocks and documentation available.

This tool can operate in multiple modes:

1. **signatures**: Extracts function/method signatures and other declarations from code files to understand APIs without looking at the entire implementation. Useful to understand file before read it full.
2. **search**: Searches across all content types (code, docs, and text) using semantic embeddings. Find something specific when not sure where to find it.
3. **codesearch**: Searches only within code blocks using semantic embeddings. When we are looking exactly for some code blocks that do something required.
4. **docsearch**: Searches only within documentation/markdown content.Results across .md files in project, mostly about docs.
5. **textsearch**: Searches only within text files and other readable content. Not very useful in development, but in case to search text content can be useful.

Use signatures mode when you want to understand what functions/methods are available in specific files.
Use the search mode or specific search mode when you need to find something in code blocks, documentation or just text files first, to undersntand what is going on and where to get something specific or try to catch up flow.

The tool returns results formatted in a clean, token-efficient Markdown output.".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["mode"],
			"properties": {
				"mode": {
					"type": "string",
					"enum": ["signatures", "search", "codesearch", "docsearch", "textsearch"],
					"description": "The mode to use: 'signatures' to view function signatures, 'search' for combined search across all content, 'codesearch' for code only, 'docsearch' for docs only, 'textsearch' for text files only"
				},
				"files": {
					"type": "array",
					"items": {
						"type": "string"
					},
					"description": "[For signatures mode] Files to analyze. Can include glob patterns (e.g., 'src/*.rs', 'src/**/*.py')."
				},
				"query": {
					"type": "string",
					"description": "[For search modes] Natural language query to search for"
				},
				"expand": {
					"type": "boolean",
					"description": "[For search modes] Whether to expand symbols in search results to include related code",
					"default": false
				}
			}
		}),
	}
}

// Define the code_search function for semantic code search only
pub fn get_code_search_function() -> McpFunction {
	McpFunction {
		name: "code_search".to_string(),
		description: "Perform semantic search specifically within code blocks using embeddings.

This searches only through semantically indexed code blocks, which include functions, methods, classes,
and other meaningful code structures. It uses tree-sitter parsing and semantic embeddings to find
code that matches your natural language query.

Use this when you want to find specific code functionality or implementations.".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["query"],
			"properties": {
				"query": {
					"type": "string",
					"description": "Natural language query to search for in code blocks of current project"
				},
				"expand": {
					"type": "boolean",
					"description": "Whether to expand symbols in search results to include related code",
					"default": false
				}
			}
		}),
	}
}

// Define the docs_search function for document search only
pub fn get_docs_search_function() -> McpFunction {
	McpFunction {
		name: "docs_search".to_string(),
		description: "Perform semantic search specifically within documentation content.

This searches only through markdown documentation files that have been indexed. Content is split
by headers and sections to provide meaningful, contextual search results from README files,
documentation, and other markdown content.

Use this when you want to find information in project documentation or README files.".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["query"],
			"properties": {
				"query": {
					"type": "string",
					"description": "Natural language query to search for in documentation"
				}
			}
		}),
	}
}

// Define the text_search function for text content search only
pub fn get_text_search_function() -> McpFunction {
	McpFunction {
		name: "text_search".to_string(),
		description: "Perform semantic search specifically within text files and other readable content.

This searches through chunked text content from various file types including configuration files,
logs, data files, and other text-based content that isn't code or markdown documentation.
Content is chunked into 2000-character segments with overlap for better search granularity.

Supported file types: txt, log, xml, html, css, sql, csv, yaml, toml, ini, conf, and others.

Use this when you want to find information in configuration files, logs, or other text content.".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["query"],
			"properties": {
				"query": {
					"type": "string",
					"description": "Natural language query to search for in text content"
				}
			}
		}),
	}
}

// Execute view_signatures command
pub async fn execute_view_signatures(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract files parameter
	let files = match call.parameters.get("files") {
		Some(Value::Array(files_array)) => {
			// Convert JSON array to Vec<String>
			files_array.iter()
				.filter_map(|v| v.as_str().map(|s| s.to_string()))
				.collect::<Vec<String>>()
		},
		_ => return Err(anyhow!("Missing or invalid 'files' parameter, expected an array of strings")),
	};

	if files.is_empty() {
		return Err(anyhow!("No files specified"));
	}

	// Get current directory for resolving paths
	let current_dir = std::env::current_dir()?;

	// Process file patterns and find matching files
	let mut matching_files = Vec::new();

	for pattern in &files {
		// Use glob pattern matching
		let glob_pattern = match globset::Glob::new(pattern) {
			Ok(g) => g.compile_matcher(),
			Err(e) => {
				return Err(anyhow!("Invalid glob pattern '{}': {}", pattern, e));
			}
		};

		// Use ignore crate to respect .gitignore files while finding files
		let walker = ignore::WalkBuilder::new(&current_dir)
			.hidden(false)  // Don't ignore hidden files (unless in .gitignore)
			.git_ignore(true)  // Respect .gitignore files
			.git_global(true) // Respect global git ignore files
			.git_exclude(true) // Respect .git/info/exclude files
			.build();

		for result in walker {
			let entry = match result {
				Ok(entry) => entry,
				Err(_) => continue,
			};

			// Skip directories, only process files
			if !entry.file_type().is_some_and(|ft| ft.is_file()) {
				continue;
			}

			// See if this file matches our pattern
			let relative_path = entry.path().strip_prefix(&current_dir).unwrap_or(entry.path());
			if glob_pattern.is_match(relative_path) {
				matching_files.push(entry.path().to_path_buf());
			}
		}
	}

	if matching_files.is_empty() {
		return Ok(McpToolResult {
			tool_name: "view_signatures".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"success": true,
				"output": "No matching files found.",
				"files": [],
				"count": 0,
				"parameters": {
					"files": files
				}
			}),
		});
	}

	// Extract signatures from matching files using the indexer module
	let signatures = match crate::indexer::extract_file_signatures(&matching_files) {
		Ok(sigs) => sigs,
		Err(e) => return Err(anyhow!("Failed to extract signatures: {}", e)),
	};

	// Format the results as text output
	let mut output = String::new();

	// Add header information
	if signatures.is_empty() {
		output.push_str("No signatures found.\n");
	} else {
		output.push_str(&format!("Found signatures in {} files:\n\n", signatures.len()));

		// Process each file
		for file in &signatures {
			output.push_str(&format!("╔══════════════════ File: {} ══════════════════\n", file.path));
			output.push_str(&format!("║ Language: {}\n", file.language));

			// Show file comment if available
			if let Some(comment) = &file.file_comment {
				output.push_str("║\n");
				output.push_str("║ File description:\n");
				for line in comment.lines() {
					output.push_str(&format!("║   {}\n", line));
				}
			}

			if file.signatures.is_empty() {
				output.push_str("║\n");
				output.push_str("║ No signatures found in this file.\n");
			} else {
				for signature in &file.signatures {
					output.push_str("║\n");

					// Display line range if it spans multiple lines, otherwise just the start line
					let line_display = if signature.start_line == signature.end_line {
						format!("{}", signature.start_line + 1)
					} else {
						format!("{}-{}", signature.start_line + 1, signature.end_line + 1)
					};

					output.push_str(&format!("║ {} `{}` (line {})\n", signature.kind, signature.name, line_display));

					// Show description if available
					if let Some(desc) = &signature.description {
						output.push_str("║ Description:\n");
						for line in desc.lines() {
							output.push_str(&format!("║   {}\n", line));
						}
					}

					// Format the signature for display
					output.push_str("║ Signature:\n");
					let lines = signature.signature.lines().collect::<Vec<_>>();
					if lines.len() > 1 {
						output.push_str("║ ┌────────────────────────────────────\n");
						for line in lines.iter().take(5) {
							output.push_str(&format!("║ │ {}\n", line));
						}
						// If signature is too long, truncate it
						if lines.len() > 5 {
							output.push_str(&format!("║ │ ... ({} more lines)\n", lines.len() - 5));
						}
						output.push_str("║ └────────────────────────────────────\n");
					} else if !lines.is_empty() {
						output.push_str(&format!("║   {}\n", lines[0]));
					}
				}
			}

			output.push_str("╚════════════════════════════════════════\n\n");
		}
	}

	// Return the result with both text and structured data
	Ok(McpToolResult {
		tool_name: "view_signatures".to_string(),

		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": output,
			"files_analyzed": matching_files.len(),
			"signatures_found": signatures.iter().fold(0, |acc, file| acc + file.signatures.len()),
			"parameters": {
				"files": files
			}
		}),
	})
}

// Execute semantic_code function (all modes)
pub async fn execute_semantic_code(call: &McpToolCall, store: &crate::store::Store, config: &crate::config::Config) -> Result<McpToolResult> {
	// Extract mode parameter
	let mode = match call.parameters.get("mode") {
		Some(Value::String(m)) => m.as_str(),
		_ => return Err(anyhow!("Missing or invalid 'mode' parameter. Must be 'signatures', 'search', 'codesearch', 'docsearch', or 'textsearch'")),
	};

	match mode {
		"signatures" => execute_signatures_mode(call).await,
		"search" => execute_search_mode(call, store, config).await,
		"codesearch" => execute_codesearch_mode(call, store, config).await,
		"docsearch" => execute_docsearch_mode(call, store, config).await,
		"textsearch" => execute_textsearch_mode(call, store, config).await,
		_ => Err(anyhow!("Invalid mode: {}. Must be 'signatures', 'search', 'codesearch', 'docsearch', or 'textsearch'", mode)),
	}
}

// Implementation of signatures mode
async fn execute_signatures_mode(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract files parameter
	let files = match call.parameters.get("files") {
		Some(Value::Array(files_array)) => {
			// Convert JSON array to Vec<String>
			files_array.iter()
				.filter_map(|v| v.as_str().map(|s| s.to_string()))
				.collect::<Vec<String>>()
		},
		_ => return Err(anyhow!("Missing or invalid 'files' parameter, expected an array of strings")),
	};

	if files.is_empty() {
		return Err(anyhow!("No files specified"));
	}

	// Get current directory for resolving paths
	let current_dir = std::env::current_dir()?;

	// Process file patterns and find matching files
	let mut matching_files = Vec::new();

	for pattern in &files {
		// Use glob pattern matching
		let glob_pattern = match globset::Glob::new(pattern) {
			Ok(g) => g.compile_matcher(),
			Err(e) => {
				return Err(anyhow!("Invalid glob pattern '{}': {}", pattern, e));
			}
		};

		// Use ignore crate to respect .gitignore files while finding files
		let walker = ignore::WalkBuilder::new(&current_dir)
			.hidden(false)  // Don't ignore hidden files (unless in .gitignore)
			.git_ignore(true)  // Respect .gitignore files
			.git_global(true) // Respect global git ignore files
			.git_exclude(true) // Respect .git/info/exclude files
			.build();

		for result in walker {
			let entry = match result {
				Ok(entry) => entry,
				Err(_) => continue,
			};

			// Skip directories, only process files
			if !entry.file_type().is_some_and(|ft| ft.is_file()) {
				continue;
			}

			// See if this file matches our pattern
			let relative_path = entry.path().strip_prefix(&current_dir).unwrap_or(entry.path());
			if glob_pattern.is_match(relative_path) {
				matching_files.push(entry.path().to_path_buf());
			}
		}
	}

	if matching_files.is_empty() {
		return Ok(McpToolResult {
			tool_name: "semantic_code".to_string(),

			tool_id: call.tool_id.clone(),
			result: json!({
				"success": true,
				"output": "No matching files found.",
				"files": [],
				"count": 0,
				"parameters": {
					"mode": "signatures",
					"files": files
				}
			}),
		});
	}

	// Extract signatures from matching files using the indexer module
	let signatures = match crate::indexer::extract_file_signatures(&matching_files) {
		Ok(sigs) => sigs,
		Err(e) => return Err(anyhow!("Failed to extract signatures: {}", e)),
	};

	// Format the results as markdown
	let markdown_output = crate::indexer::signatures_to_markdown(&signatures);

	// Return the result with both text and structured data
	Ok(McpToolResult {
		tool_name: "semantic_code".to_string(),

		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown_output,
			"files_analyzed": matching_files.len(),
			"signatures_found": signatures.iter().fold(0, |acc, file| acc + file.signatures.len()),
			"parameters": {
				"mode": "signatures",
				"files": files
			}
		}),
	})
}

// Implementation of search mode (combined search across all content types)
async fn execute_search_mode(call: &McpToolCall, store: &crate::store::Store, config: &crate::config::Config) -> Result<McpToolResult> {
	// Extract query parameter
	let query = match call.parameters.get("query") {
		Some(Value::String(q)) => q.clone(),
		_ => return Err(anyhow!("Missing or invalid 'query' parameter, expected a string")),
	};

	if query.trim().is_empty() {
		return Err(anyhow!("Query cannot be empty"));
	}

	// Extract optional expand parameter
	let expand = call.parameters.get("expand")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	// Get current directory for checking index
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let index_path = octodev_dir.join("storage");

	// Check if we have an index, which is required for search
	if !index_path.exists() {
		return Err(anyhow!("No index found. Please run 'octodev index' first before using search."));
	}

	// Generate embeddings for the query
	let embeddings = match crate::indexer::generate_embeddings(&query, true, config).await {
		Ok(emb) => emb,
		Err(e) => return Err(anyhow!("Failed to generate query embeddings: {}", e)),
	};

	// Search across all content types
	let code_results = match store.get_code_blocks(embeddings.clone()).await {
		Ok(res) => res,
		Err(e) => return Err(anyhow!("Failed to search for code blocks: {}", e)),
	};

	let doc_results = match store.get_document_blocks(embeddings.clone()).await {
		Ok(res) => res,
		Err(e) => return Err(anyhow!("Failed to search for document blocks: {}", e)),
	};

	let text_results = match store.get_text_blocks(embeddings).await {
		Ok(res) => res,
		Err(e) => return Err(anyhow!("Failed to search for text blocks: {}", e)),
	};

	// If expand flag is set, expand symbols in code results
	let mut final_code_results = code_results;
	if expand {
		final_code_results = match crate::indexer::expand_symbols(store, final_code_results).await {
			Ok(expanded) => expanded,
			Err(e) => return Err(anyhow!("Failed to expand symbols: {}", e)),
		};
	}

	// Create combined markdown output
	let mut combined_markdown = String::new();

	if !doc_results.is_empty() {
		combined_markdown.push_str("# Documentation Results\n\n");
		combined_markdown.push_str(&crate::indexer::document_blocks_to_markdown(&doc_results));
		combined_markdown.push('\n');
	}

	if !final_code_results.is_empty() {
		combined_markdown.push_str("# Code Results\n\n");
		combined_markdown.push_str(&crate::indexer::code_blocks_to_markdown(&final_code_results));
		combined_markdown.push('\n');
	}

	if !text_results.is_empty() {
		combined_markdown.push_str("# Text Results\n\n");
		combined_markdown.push_str(&crate::indexer::text_blocks_to_markdown(&text_results));
	}

	if combined_markdown.is_empty() {
		combined_markdown.push_str("No results found for the query.");
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "semantic_code".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": combined_markdown,
			"code_blocks_found": final_code_results.len(),
			"doc_blocks_found": doc_results.len(),
			"text_blocks_found": text_results.len(),
			"total_results": final_code_results.len() + doc_results.len() + text_results.len(),
			"parameters": {
				"mode": "search",
				"query": query,
				"expand": expand
			}
		}),
	})
}

// Implementation of codesearch mode (code blocks only)
async fn execute_codesearch_mode(call: &McpToolCall, store: &crate::store::Store, config: &crate::config::Config) -> Result<McpToolResult> {
	// Extract query parameter
	let query = match call.parameters.get("query") {
		Some(Value::String(q)) => q.clone(),
		_ => return Err(anyhow!("Missing or invalid 'query' parameter, expected a string")),
	};

	if query.trim().is_empty() {
		return Err(anyhow!("Query cannot be empty"));
	}

	// Extract optional expand parameter
	let expand = call.parameters.get("expand")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	// Check if we have an index
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let index_path = octodev_dir.join("storage");

	if !index_path.exists() {
		return Err(anyhow!("No index found. Please run 'octodev index' first before using search."));
	}

	// Generate embeddings for the query
	let embeddings = match crate::indexer::generate_embeddings(&query, true, config).await {
		Ok(emb) => emb,
		Err(e) => return Err(anyhow!("Failed to generate query embeddings: {}", e)),
	};

	// Search for matching code blocks only
	let mut results = match store.get_code_blocks(embeddings).await {
		Ok(res) => res,
		Err(e) => return Err(anyhow!("Failed to search for code blocks: {}", e)),
	};

	// If expand flag is set, expand symbols in the results
	if expand {
		results = match crate::indexer::expand_symbols(store, results).await {
			Ok(expanded) => expanded,
			Err(e) => return Err(anyhow!("Failed to expand symbols: {}", e)),
		};
	}

	// Format the results as markdown
	let markdown_output = crate::indexer::code_blocks_to_markdown(&results);

	// Return the result
	Ok(McpToolResult {
		tool_name: "semantic_code".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown_output,
			"blocks_found": results.len(),
			"parameters": {
				"mode": "codesearch",
				"query": query,
				"expand": expand
			}
		}),
	})
}

// Implementation of docsearch mode (documentation blocks only)
async fn execute_docsearch_mode(call: &McpToolCall, store: &crate::store::Store, config: &crate::config::Config) -> Result<McpToolResult> {
	// Extract query parameter
	let query = match call.parameters.get("query") {
		Some(Value::String(q)) => q.clone(),
		_ => return Err(anyhow!("Missing or invalid 'query' parameter, expected a string")),
	};

	if query.trim().is_empty() {
		return Err(anyhow!("Query cannot be empty"));
	}

	// Check if we have an index
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let index_path = octodev_dir.join("storage");

	if !index_path.exists() {
		return Err(anyhow!("No index found. Please run 'octodev index' first before using search."));
	}

	// Generate embeddings for the query
	let embeddings = match crate::indexer::generate_embeddings(&query, false, config).await {
		Ok(emb) => emb,
		Err(e) => return Err(anyhow!("Failed to generate query embeddings: {}", e)),
	};

	// Search for matching document blocks only
	let results = match store.get_document_blocks(embeddings).await {
		Ok(res) => res,
		Err(e) => return Err(anyhow!("Failed to search for document blocks: {}", e)),
	};

	// Format the results as markdown
	let markdown_output = crate::indexer::document_blocks_to_markdown(&results);

	// Return the result
	Ok(McpToolResult {
		tool_name: "semantic_code".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown_output,
			"blocks_found": results.len(),
			"parameters": {
				"mode": "docsearch",
				"query": query
			}
		}),
	})
}

// Implementation of textsearch mode (text blocks only)
async fn execute_textsearch_mode(call: &McpToolCall, store: &crate::store::Store, config: &crate::config::Config) -> Result<McpToolResult> {
	// Extract query parameter
	let query = match call.parameters.get("query") {
		Some(Value::String(q)) => q.clone(),
		_ => return Err(anyhow!("Missing or invalid 'query' parameter, expected a string")),
	};

	if query.trim().is_empty() {
		return Err(anyhow!("Query cannot be empty"));
	}

	// Check if we have an index
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let index_path = octodev_dir.join("storage");

	if !index_path.exists() {
		return Err(anyhow!("No index found. Please run 'octodev index' first before using search."));
	}

	// Generate embeddings for the query
	let embeddings = match crate::indexer::generate_embeddings(&query, false, config).await {
		Ok(emb) => emb,
		Err(e) => return Err(anyhow!("Failed to generate query embeddings: {}", e)),
	};

	// Search for matching text blocks only
	let results = match store.get_text_blocks(embeddings).await {
		Ok(res) => res,
		Err(e) => return Err(anyhow!("Failed to search for text blocks: {}", e)),
	};

	// Format the results as markdown
	let markdown_output = crate::indexer::text_blocks_to_markdown(&results);

	// Return the result
	Ok(McpToolResult {
		tool_name: "semantic_code".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown_output,
			"blocks_found": results.len(),
			"parameters": {
				"mode": "textsearch",
				"query": query
			}
		}),
	})
}