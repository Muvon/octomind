// Developer MCP provider with enhanced functionality
// Based on the reference implementation with additional developer tools

use std::process::Command;
use serde_json::{json, Value};
use anyhow::{Result, anyhow};
use super::{McpToolCall, McpToolResult, McpFunction};

// Define the shell function for the MCP protocol with enhanced description
pub fn get_shell_function() -> McpFunction {
	McpFunction {
		name: "shell".to_string(),
		description: format!("Execute a command in the shell.

This will return the output and error concatenated into a single string, as
you would see from running on the command line. There will also be an indication
of if the command succeeded or failed.

Avoid commands that produce a large amount of output, and consider piping those outputs to files.
If you need to run a long lived command, background it - e.g. `uvicorn main:app &` so that
this tool does not run indefinitely.

**Important**: Each shell command runs in its own process. Things like directory changes or
sourcing files do not persist between tool calls. So you may need to repeat them each time by
stringing together commands, e.g. `cd example && ls` or `source env/bin/activate && pip install numpy`

**Important**: Use ripgrep - `rg` - when you need to locate a file or a code reference, other solutions
may show ignored or hidden files. For example *do not* use `find` or `ls -r`
- List files by name: `rg --files | rg <filename>`
- List files that contain a regex: `rg '<regex>' -l`
"),
		parameters: json!({
			"type": "object",
			"properties": {
				"command": {
					"type": "string",
					"description": "The shell command to execute"
				}
			},
			"required": ["command"]
		}),
	}
}

// Define the semantic_code_function for signatures and search modes
pub fn get_semantic_code_function() -> McpFunction {
	McpFunction {
		name: "semantic_code".to_string(),
		description: "Analyze and search code in the repository using both structural and semantic methods.

This tool can operate in multiple modes:

1. **signatures**: Extracts function/method signatures and other declarations from code files to understand APIs without looking at the entire implementation.
2. **search**: Searches across all content types (code, docs, and text) using semantic embeddings.
3. **codesearch**: Searches only within code blocks using semantic embeddings.
4. **docsearch**: Searches only within documentation/markdown content.
5. **textsearch**: Searches only within text files and other readable content.

Use signatures mode when you want to understand what functions/methods are available in specific files.
Use the search modes when you want to find specific functionality across different content types.

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
					"description": "Natural language query to search for in code blocks"
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

// Execute a shell command
pub async fn execute_shell_command(call: &McpToolCall) -> Result<McpToolResult> {
	// Extract command parameter
	let command = match call.parameters.get("command") {
		Some(Value::String(cmd)) => cmd.clone(),
		_ => return Err(anyhow!("Missing or invalid 'command' parameter")),
	};

	// Execute the command
	let output = tokio::task::spawn_blocking(move || {
		let output = if cfg!(target_os = "windows") {
			Command::new("cmd")
				.args(["/C", &command])
				.output()
		} else {
			Command::new("sh")
				.args(["-c", &command])
				.output()
		};

		match output {
			Ok(output) => {
				let stdout = String::from_utf8_lossy(&output.stdout).to_string();
				let stderr = String::from_utf8_lossy(&output.stderr).to_string();

				// Format the output more clearly with error handling
				let combined = if stderr.is_empty() {
					stdout
				} else if stdout.is_empty() {
					stderr
				} else {
					format!("{}

Error: {}", stdout, stderr)
				};

				// Add detailed execution results including status code
				let status_code = output.status.code().unwrap_or(-1);
				let success = output.status.success();

				json!({
					"success": success,
					"output": combined,
					"code": status_code,
					"parameters": {
						"command": command
					},
					"message": if success {
						format!("Command executed successfully with exit code {}", status_code)
					} else {
					format!("Command failed with exit code {}", status_code)
			}
			})
			},
			Err(e) => json!({
				"success": false,
				"output": format!("Failed to execute command: {}", e),
				"code": -1,
				"parameters": {
					"command": command
				},
				"message": format!("Failed to execute command: {}", e)
			}),
		}
	}).await?;

	Ok(McpToolResult {
		tool_name: "shell".to_string(),
		tool_id: call.tool_id.clone(),
		result: output,
	})
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
			if !entry.file_type().map_or(false, |ft| ft.is_file()) {
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
			if !entry.file_type().map_or(false, |ft| ft.is_file()) {
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
		combined_markdown.push_str("\n");
	}

	if !final_code_results.is_empty() {
		combined_markdown.push_str("# Code Results\n\n");
		combined_markdown.push_str(&crate::indexer::code_blocks_to_markdown(&final_code_results));
		combined_markdown.push_str("\n");
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

// Get all available developer functions
pub fn get_all_functions() -> Vec<McpFunction> {
	let mut functions = vec![
		get_shell_function(),
		get_semantic_code_function(),
	];

	// Only add GraphRAG function if the feature is enabled in the config
	let config = crate::config::Config::load().unwrap_or_default();
	if config.graphrag.enabled {
		functions.push(get_graphrag_function());
	}

	functions
}

// Define the GraphRAG function
pub fn get_graphrag_function() -> McpFunction {
	McpFunction {
		name: "graphrag".to_string(),
		description: "Query and explore the code relationship graph (GraphRAG) built during indexing.

This tool allows you to explore the code knowledge graph that was built during indexing, which contains
code entities and their relationships. This semantic graph helps in understanding complex codebases and
finding connections between different parts of the code.

Operations:
- `search`: Find code nodes that match a semantic query
- Use with `task_focused: true` for an optimized, token-efficient view focused on your specific task
- `get_node`: Get detailed information about a specific node by ID
- `get_relationships`: Find relationships involving a specific node
- `find_path`: Find paths between two nodes in the graph
- `overview`: Get an overview of the entire graph structure

Use this tool to understand how different parts of the code are related and to explore the codebase
from a structural perspective.".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["operation"],
			"properties": {
				"operation": {
					"type": "string",
					"enum": ["search", "get_node", "get_relationships", "find_path", "overview"],
					"description": "The GraphRAG operation to perform"
				},
				"query": {
					"type": "string",
					"description": "[For search operation] The semantic query to search for"
				},
				"task_focused": {
					"type": "boolean",
					"description": "[For search operation] Whether to use task-focused optimization to provide a more concise, relevant view",
					"default": false
				},
				"node_id": {
					"type": "string",
					"description": "[For get_node/get_relationships operations] The ID of the node to get information about"
				},
				"source_id": {
					"type": "string",
					"description": "[For find_path operation] The ID of the source node"
				},
				"target_id": {
					"type": "string",
					"description": "[For find_path operation] The ID of the target node"
				},
				"max_depth": {
					"type": "integer",
					"description": "[For find_path operation] The maximum path length to consider",
					"default": 3
				}
			}
		}),
	}
}

// Execute GraphRAG operations
pub async fn execute_graphrag(call: &McpToolCall, config: &crate::config::Config) -> Result<McpToolResult> {
	// Extract operation parameter
	let operation = match call.parameters.get("operation") {
		Some(Value::String(op)) => op.as_str(),
		_ => return Err(anyhow!("Missing or invalid 'operation' parameter")),
	};

	// Check if GraphRAG is enabled
	if !config.graphrag.enabled {
		return Ok(McpToolResult {
			tool_name: "graphrag".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"success": false,
				"error": "GraphRAG is not enabled. Enable it in your configuration and run 'octodev index' to build the knowledge graph.",
				"message": "To enable GraphRAG, add the following to your .octodev/config.toml file:\n\n[graphrag]\nenabled = true\n\nThen run 'octodev index' to build the knowledge graph."
			}),
		});
	}

	// Initialize the GraphBuilder
	let graph_builder = match crate::indexer::GraphBuilder::new(config.clone()).await {
		Ok(builder) => builder,
		Err(e) => return Err(anyhow!("Failed to initialize GraphBuilder: {}", e)),
	};

	// Execute the requested operation
	match operation {
		"search" => execute_graphrag_search(call, &graph_builder).await,
		"get_node" => execute_graphrag_get_node(call, &graph_builder).await,
		"get_relationships" => execute_graphrag_get_relationships(call, &graph_builder).await,
		"find_path" => execute_graphrag_find_path(call, &graph_builder).await,
		"overview" => execute_graphrag_overview(call, &graph_builder).await,
		_ => Err(anyhow!("Invalid operation: {}", operation)),
	}
}

// Search for nodes in the graph
async fn execute_graphrag_search(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract query parameter
	let query = match call.parameters.get("query") {
		Some(Value::String(q)) => q.clone(),
		_ => return Err(anyhow!("Missing or invalid 'query' parameter for search operation")),
	};

	// Check for task-focused flag
	let task_focused = call.parameters.get("task_focused")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	if task_focused {
		// Use the graph optimizer for task-focused search
		let store = crate::store::Store::new().await?;
		let config = crate::config::Config::load().unwrap_or_default();

		// Get the full graph
		let full_graph = graph_builder.get_graph().await?;

		// Generate embeddings for the query
		let query_embedding = crate::indexer::generate_embeddings(&query, false, &config).await?;

		// Create optimizer with token budget
		let optimizer = crate::indexer::graph_optimization::GraphOptimizer::new(2000);

		// Get all code blocks
		let code_blocks = store.get_code_blocks(query_embedding.clone()).await?;

		// Generate a task-focused view
		let task_view = optimizer.generate_task_focused_view(
			&query,
			&query_embedding,
			&full_graph,
			&code_blocks
		).await?;

		// Return the optimized view
		return Ok(McpToolResult {
			tool_name: "graphrag".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"success": true,
				"output": task_view,
				"task_focused": true,
				"parameters": {
					"operation": "search",
					"query": query,
					"task_focused": true
				}
			}),
		});
	}

	// Traditional node search (without task focusing)
	let nodes = graph_builder.search_nodes(&query).await?;

	// Format the results as markdown using the official formatter
	let markdown = crate::indexer::graphrag::graphrag_nodes_to_markdown(&nodes);

	// Return the results
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"count": nodes.len(),
			"nodes": nodes,
			"parameters": {
				"operation": "search",
				"query": query
			}
		}),
	})
}

// Get details about a specific node
async fn execute_graphrag_get_node(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract node_id parameter
	let node_id = match call.parameters.get("node_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'node_id' parameter for get_node operation")),
	};

	// Get the graph
	let graph = graph_builder.get_graph().await?;

	// Check if the node exists
	let node = match graph.nodes.get(&node_id) {
		Some(node) => node.clone(),
		None => return Err(anyhow!("Node not found: {}", node_id)),
	};

	// Format the result as markdown
	let mut markdown = String::from(format!("# Node: {}\n\n", node.name));
	markdown.push_str(&format!("**ID**: {}\n", node.id));
	markdown.push_str(&format!("**Kind**: {}\n", node.kind));
	markdown.push_str(&format!("**Path**: {}\n", node.path));
	markdown.push_str(&format!("**Description**: {}\n\n", node.description));

	// Add symbols if any
	if !node.symbols.is_empty() {
		markdown.push_str("## Symbols\n\n");
		for symbol in &node.symbols {
			markdown.push_str(&format!("- `{}`\n", symbol));
		}
		markdown.push_str("\n");
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"node": node,
			"parameters": {
				"operation": "get_node",
				"node_id": node_id
			}
		}),
	})
}

// Get relationships involving a specific node
async fn execute_graphrag_get_relationships(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract node_id parameter
	let node_id = match call.parameters.get("node_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'node_id' parameter for get_relationships operation")),
	};

	// Get the graph
	let graph = graph_builder.get_graph().await?;

	// Check if the node exists
	if !graph.nodes.contains_key(&node_id) {
		return Err(anyhow!("Node not found: {}", node_id));
	}

	// Find relationships where this node is either source or target
	let relationships: Vec<_> = graph.relationships.iter()
		.filter(|rel| rel.source == node_id || rel.target == node_id)
		.cloned()
		.collect();

	// Format the result as markdown
	let mut markdown = format!("# Relationships for Node: {}\n\n", node_id);

	if relationships.is_empty() {
		markdown.push_str("No relationships found for this node.\n");
	} else {
		markdown.push_str(&format!("Found {} relationships:\n\n", relationships.len()));

		// Outgoing relationships
		let outgoing: Vec<_> = relationships.iter()
			.filter(|rel| rel.source == node_id)
			.collect();

		if !outgoing.is_empty() {
			markdown.push_str("## Outgoing Relationships\n\n");
			for rel in outgoing {
				let target_name = graph.nodes.get(&rel.target)
					.map(|n| n.name.clone())
					.unwrap_or_else(|| rel.target.clone());

				markdown.push_str(&format!("- **{}** → {} ({}): {}\n",
					rel.relation_type,
					target_name,
					rel.target,
					rel.description));
			}
			markdown.push_str("\n");
		}

		// Incoming relationships
		let incoming: Vec<_> = relationships.iter()
			.filter(|rel| rel.target == node_id)
			.collect();

		if !incoming.is_empty() {
			markdown.push_str("## Incoming Relationships\n\n");
			for rel in incoming {
				let source_name = graph.nodes.get(&rel.source)
					.map(|n| n.name.clone())
					.unwrap_or_else(|| rel.source.clone());

				markdown.push_str(&format!("- **{}** ← {} ({}): {}\n",
					rel.relation_type,
					source_name,
					rel.source,
					rel.description));
			}
			markdown.push_str("\n");
		}
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"relationships": relationships,
			"count": relationships.len(),
			"parameters": {
				"operation": "get_relationships",
				"node_id": node_id
			}
		}),
	})
}

// Find paths between two nodes
async fn execute_graphrag_find_path(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract parameters
	let source_id = match call.parameters.get("source_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'source_id' parameter for find_path operation")),
	};

	let target_id = match call.parameters.get("target_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'target_id' parameter for find_path operation")),
	};

	let max_depth = match call.parameters.get("max_depth") {
		Some(Value::Number(n)) => n.as_u64().unwrap_or(3) as usize,
		_ => 3, // Default to depth 3
	};

	// Find paths
	let paths = graph_builder.find_paths(&source_id, &target_id, max_depth).await?;

	// Get the graph for node name lookup
	let graph = graph_builder.get_graph().await?;

	// Format the result as markdown
	let mut markdown = format!("# Paths from '{}' to '{}'\n\n", source_id, target_id);

	if paths.is_empty() {
		markdown.push_str("No paths found between these nodes within the specified depth.\n");
	} else {
		markdown.push_str(&format!("Found {} paths with max depth {}:\n\n", paths.len(), max_depth));

		for (i, path) in paths.iter().enumerate() {
			markdown.push_str(&format!("## Path {}\n\n", i + 1));

			// Display each node in the path
			for (j, node_id) in path.iter().enumerate() {
				let node_name = graph.nodes.get(node_id)
					.map(|n| n.name.clone())
					.unwrap_or_else(|| node_id.clone());

				if j > 0 {
					// Look up the relationship
					let prev_id = &path[j-1];
					let rel = graph.relationships.iter()
						.find(|r| r.source == *prev_id && r.target == *node_id);

					if let Some(rel) = rel {
						markdown.push_str(&format!("→ **{}** → ", rel.relation_type));
					} else {
						markdown.push_str("→ ");
					}
				}

				markdown.push_str(&format!("`{}` ({})", node_name, node_id));
			}
			markdown.push_str("\n\n");
		}
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"paths": paths,
			"count": paths.len(),
			"parameters": {
				"operation": "find_path",
				"source_id": source_id,
				"target_id": target_id,
				"max_depth": max_depth
			}
		}),
	})
}

// Get an overview of the graph
async fn execute_graphrag_overview(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Get the graph
	let graph = graph_builder.get_graph().await?;

	// Get statistics
	let node_count = graph.nodes.len();
	let relationship_count = graph.relationships.len();

	// Count node types
	let mut node_types = std::collections::HashMap::new();
	for node in graph.nodes.values() {
		*node_types.entry(node.kind.clone()).or_insert(0) += 1;
	}

	// Count relationship types
	let mut rel_types = std::collections::HashMap::new();
	for rel in &graph.relationships {
		*rel_types.entry(rel.relation_type.clone()).or_insert(0) += 1;
	}

	// Format the result as markdown
	let mut markdown = String::from("# GraphRAG Knowledge Graph Overview\n\n");
	markdown.push_str(&format!("The knowledge graph contains {} nodes and {} relationships.\n\n", node_count, relationship_count));

	// Node type statistics
	markdown.push_str("## Node Types\n\n");
	for (kind, count) in node_types.iter() {
		markdown.push_str(&format!("- {}: {} nodes\n", kind, count));
	}
	markdown.push_str("\n");

	// Relationship type statistics
	markdown.push_str("## Relationship Types\n\n");
	for (rel_type, count) in rel_types.iter() {
		markdown.push_str(&format!("- {}: {} relationships\n", rel_type, count));
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"node_count": node_count,
			"relationship_count": relationship_count,
			"node_types": node_types,
			"relationship_types": rel_types,
			"parameters": {
				"operation": "overview"
			}
		}),
	})
}
