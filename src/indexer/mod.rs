// Indexer module for OctoDev
// Handles code indexing, embedding, and search functionality

mod embed; // Embedding generation - moving from content.rs
mod search; // Search functionality
mod languages; // Language-specific processors
pub mod graphrag; // GraphRAG generation for code relationships
pub mod graph_optimization; // Task-focused graph extraction and optimization

pub use embed::*;
pub use search::*;
pub use graphrag::*;
pub use graph_optimization::*;

use crate::state::SharedState;
use crate::state;
use crate::store::{Store, CodeBlock, TextBlock};
use crate::config::Config;
use std::fs;
// We're using ignore::WalkBuilder instead of walkdir::WalkDir
use tree_sitter::{Parser, Node};
use anyhow::Result;
use ignore;
use std::path::PathBuf;
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct FileSignature {
	pub path: String,
	pub language: String,
	pub file_comment: Option<String>,
	pub signatures: Vec<SignatureItem>,
}

#[derive(Debug, Serialize, Clone)]
pub struct SignatureItem {
	pub kind: String,           // e.g., "function", "struct", "class", etc.
	pub name: String,           // Name of the item
	pub signature: String,      // Full signature
	pub description: Option<String>,  // Comment if available
	pub start_line: usize,      // Start line number
	pub end_line: usize,        // End line number
}

// Detect language based on file extension
pub fn detect_language(path: &std::path::Path) -> Option<&str> {
	match path.extension()?.to_str()? {
		"rs" => Some("rust"),
		"php" => Some("php"),
		"py" => Some("python"),
		"js" => Some("javascript"),
		"ts" => Some("typescript"),
		"jsx" | "tsx" => Some("typescript"),
		"json" => Some("json"),
		"go" => Some("go"),
		"cpp" | "cc" | "cxx" | "c++" | "hpp" | "h" => Some("cpp"),
		"sh" | "bash" => Some("bash"),
		"rb" => Some("ruby"),
		_ => None,
	}
}

/// Function to extract file signatures
pub fn extract_file_signatures(files: &[PathBuf]) -> Result<Vec<FileSignature>> {
	let mut all_signatures = Vec::new();
	let mut parser = Parser::new();

	for file_path in files {
		if let Some(language) = detect_language(file_path) {
			// Get the language implementation
			let lang_impl = match languages::get_language(language) {
				Some(impl_) => impl_,
				None => continue,  // Skip unsupported languages
			};

			// Set the parser language
			parser.set_language(&lang_impl.get_ts_language())?;

			// Read file contents
			if let Ok(contents) = fs::read_to_string(file_path) {
				// Create a relative path for display
				let current_dir = std::env::current_dir()?;
				let display_path = file_path.strip_prefix(&current_dir)
					.unwrap_or(file_path)
					.to_string_lossy()
					.to_string();

				// Parse the file
				let tree = parser.parse(&contents, None)
					.unwrap_or_else(|| parser.parse("", None).unwrap());

				// Extract signatures from the file
				let signatures = extract_signatures(tree.root_node(), &contents, &lang_impl);

				// Extract file-level comment if present
				let file_comment = extract_file_comment(tree.root_node(), &contents);

				// Add to our results
				all_signatures.push(FileSignature {
					path: display_path,
					language: lang_impl.name().to_string(),
					file_comment,
					signatures,
				});
			}
		}
	}

	Ok(all_signatures)
}

/// Extract signatures from a parsed file
fn extract_signatures(node: Node, contents: &str, lang_impl: &Box<dyn languages::Language>) -> Vec<SignatureItem> {
	let mut signatures = Vec::new();
	let meaningful_kinds = lang_impl.get_meaningful_kinds();

	// Create a visitor function to traverse the tree
	fn visit_node(
		node: Node,
		contents: &str,
		lang_impl: &Box<dyn languages::Language>,
		meaningful_kinds: &[&str],
		signatures: &mut Vec<SignatureItem>
	) {
		let node_kind = node.kind();

		// Check if this node is a meaningful declaration
		if meaningful_kinds.contains(&node_kind) {
			// Get the line numbers
			let start_line = node.start_position().row;
			let end_line = node.end_position().row;

			// Extract the name of the item (function name, struct name, etc.)
			let name = extract_name(node, contents, lang_impl);

			// Extract the preceding comment if available
			let description = extract_preceding_comment(node, contents);

			if let Some(name) = name {
				// Get the full signature text
				let sig_text = node_text(node, contents);

				// Map tree-sitter node kinds to our simplified kinds
				let kind = map_node_kind_to_simple(node_kind);

				signatures.push(SignatureItem {
					kind,
					name,
					signature: sig_text,
					description,
					start_line,
					end_line,
				});
			}
		}

		// Recursively process children
		let mut cursor = node.walk();
		if cursor.goto_first_child() {
			loop {
				visit_node(cursor.node(), contents, lang_impl, meaningful_kinds, signatures);
				if !cursor.goto_next_sibling() { break; }
			}
		}
	}

	// Start traversal from the root
	visit_node(node, contents, lang_impl, &meaningful_kinds, &mut signatures);

	// Sort by line number for a consistent order
	signatures.sort_by_key(|sig| sig.start_line);

	signatures
}

/// Extract the name of a declaration node (function, class, etc.)
fn extract_name(node: Node, contents: &str, lang_impl: &Box<dyn languages::Language>) -> Option<String> {
	// Look for identifier nodes
	for child in node.children(&mut node.walk()) {
		if child.kind() == "identifier" ||
		child.kind().contains("name") ||
		child.kind().contains("function_name") {
			if let Ok(name) = child.utf8_text(contents.as_bytes()) {
				if !name.is_empty() {
					return Some(name.to_string());
				}
			}
		}
	}

	// Fall back to using language-specific symbol extraction
	let symbols = lang_impl.extract_symbols(node, contents);
	symbols.into_iter().next()
}

/// Extract a preceding comment if available
fn extract_preceding_comment(node: Node, contents: &str) -> Option<String> {
	if let Some(parent) = node.parent() {
		let mut siblings = Vec::new();
		let mut cursor = parent.walk();

		if cursor.goto_first_child() {
			loop {
				let current = cursor.node();
				if current.id() == node.id() {
					break;
				}
				siblings.push(current);
				if !cursor.goto_next_sibling() {
					break;
				}
			}
		}

		// Check the last sibling before our node
		if let Some(last) = siblings.last() {
			if last.kind().contains("comment") {
				if let Ok(comment) = last.utf8_text(contents.as_bytes()) {
					// Clean up comment markers
					let comment = comment.trim()
						.trim_start_matches("/")
						.trim_start_matches("*")
						.trim_start_matches("/")
						.trim_end_matches("*/")
						.trim();
					return Some(comment.to_string());
				}
			}
		}
	}
	None
}

/// Extract a file-level comment (usually at the top of the file)
fn extract_file_comment(root: Node, contents: &str) -> Option<String> {
	let mut cursor = root.walk();
	if cursor.goto_first_child() {
		// Check if the first node is a comment
		let first = cursor.node();
		if first.kind().contains("comment") {
			if let Ok(comment) = first.utf8_text(contents.as_bytes()) {
				// Clean up comment markers
				let comment = comment.trim()
					.trim_start_matches("/")
					.trim_start_matches("*")
					.trim_start_matches("/")
					.trim_end_matches("*/")
					.trim();
				return Some(comment.to_string());
			}
		}
	}
	None
}

/// Get the full text of a node
fn node_text(node: Node, contents: &str) -> String {
	if let Ok(text) = node.utf8_text(contents.as_bytes()) {
		text.to_string()
	} else {
		// Fall back to byte range if UTF-8 conversion fails
		if node.start_byte() < node.end_byte() && node.end_byte() <= contents.len() {
			contents[node.start_byte()..node.end_byte()].to_string()
		} else {
			String::new()
		}
	}
}

/// Map tree-sitter node kinds to simpler, unified kinds for display
fn map_node_kind_to_simple(kind: &str) -> String {
	match kind {
		k if k.contains("function") => "function".to_string(),
		k if k.contains("method") => "method".to_string(),
		k if k.contains("class") => "class".to_string(),
		k if k.contains("struct") => "struct".to_string(),
		k if k.contains("enum") => "enum".to_string(),
		k if k.contains("interface") => "interface".to_string(),
		k if k.contains("trait") => "trait".to_string(),
		k if k.contains("mod") || k.contains("module") => "module".to_string(),
		k if k.contains("const") => "constant".to_string(),
		k if k.contains("macro") => "macro".to_string(),
		k if k.contains("type") => "type".to_string(),
		_ => kind.to_string(), // Fall back to the original kind
	}
}

/// Render signatures and search results as markdown output (more efficient for AI tools)
pub fn render_to_markdown<T: std::fmt::Display>(_title: &str, content: T) -> String {
	format!("{}", content)
}

/// Render signatures as markdown string
pub fn signatures_to_markdown(signatures: &[FileSignature]) -> String {
	let mut markdown = String::new();

	if signatures.is_empty() {
		markdown.push_str("No signatures found.");
		return markdown;
	}

	markdown.push_str(&format!("# Found signatures in {} files\n\n", signatures.len()));

	for file in signatures {
		markdown.push_str(&format!("## File: {}\n", file.path));
		markdown.push_str(&format!("**Language:** {}\n\n", file.language));

		// Show file comment if available
		if let Some(comment) = &file.file_comment {
			markdown.push_str("### File description\n");
			markdown.push_str(&format!("> {}\n\n", comment.replace("\n", "\n> ")));
		}

		if file.signatures.is_empty() {
			markdown.push_str("*No signatures found in this file.*\n\n");
		} else {
			for signature in &file.signatures {
				// Display line range if it spans multiple lines, otherwise just the start line
				let line_display = if signature.start_line == signature.end_line {
					format!("{}", signature.start_line + 1)
				} else {
					format!("{}-{}", signature.start_line + 1, signature.end_line + 1)
				};

				markdown.push_str(&format!("### {} `{}` (line {})\n", signature.kind, signature.name, line_display));

				// Show description if available
				if let Some(desc) = &signature.description {
					markdown.push_str(&format!("> {}\n\n", desc.replace("\n", "\n> ")));
				}

				// Format the signature for display
				markdown.push_str("```");

				// Add language identifier for syntax highlighting when possible
				if !file.language.is_empty() && file.language != "text" {
					markdown.push_str(&file.language);
				}
				markdown.push_str("\n");

				let lines = signature.signature.lines().collect::<Vec<_>>();
				if lines.len() > 5 {
					// Show first 5 lines only to conserve tokens
					for line in lines.iter().take(5) {
						markdown.push_str(&format!("{}", line));
						markdown.push_str("\n");
					}
					// If signature is too long, note how many lines are omitted
					markdown.push_str(&format!("// ... {} more lines\n", lines.len() - 5));
				} else {
					for line in &lines {
						markdown.push_str(&format!("{}", line));
						markdown.push_str("\n");
					}
				}
				markdown.push_str("```\n\n");
			}
		}

		// Add spacing between files
		markdown.push_str("---\n\n");
	}

	markdown
}

/// Render code blocks (search results) as markdown string
pub fn code_blocks_to_markdown(blocks: &[CodeBlock]) -> String {
	let mut markdown = String::new();

	if blocks.is_empty() {
		markdown.push_str("No code blocks found for the query.");
		return markdown;
	}

	markdown.push_str(&format!("# Found {} code blocks\n\n", blocks.len()));

	// Group blocks by file path for better organization
	let mut blocks_by_file: std::collections::HashMap<String, Vec<&CodeBlock>> = std::collections::HashMap::new();

	for block in blocks {
		blocks_by_file
			.entry(block.path.clone())
			.or_insert_with(|| Vec::new())
			.push(block);
	}

	// Print results organized by file
	for (file_path, file_blocks) in blocks_by_file.iter() {
		markdown.push_str(&format!("## File: {}\n\n", file_path));

		for (idx, block) in file_blocks.iter().enumerate() {
			markdown.push_str(&format!("### Block {} of {}\n", idx + 1, file_blocks.len()));
			markdown.push_str(&format!("**Language:** {}  ", block.language));
			markdown.push_str(&format!("**Lines:** {}-{}  ", block.start_line, block.end_line));

			// Show relevance score if available
			if let Some(distance) = block.distance {
				markdown.push_str(&format!("**Relevance:** {:.4}  ", distance));
			}
			markdown.push_str("\n");

			if !block.symbols.is_empty() {
				markdown.push_str("**Symbols:**  \n");
				// Deduplicate symbols in display
				let mut display_symbols = block.symbols.clone();
				display_symbols.sort();
				display_symbols.dedup();

				for symbol in display_symbols {
					// Only show non-type symbols to users
					if !symbol.contains("_") {
						markdown.push_str(&format!("- `{}`  \n", symbol));
					}
				}
			}

			markdown.push_str("```");
			// Add language for syntax highlighting
			if !block.language.is_empty() && block.language != "text" {
				markdown.push_str(&block.language);
			}
			markdown.push_str("\n");

			// Get the lines and determine if we need to truncate
			let lines: Vec<&str> = block.content.lines().collect();
			if lines.len() > 15 {
				// Show first 10 lines
				for line in lines.iter().take(10) {
					markdown.push_str(&format!("{}", line));
					markdown.push_str("\n");
				}
				// Note how many lines are omitted
				markdown.push_str(&format!("// ... {} more lines omitted\n", lines.len() - 15));
				// Show last 5 lines
				for line in lines.iter().skip(lines.len() - 5) {
					markdown.push_str(&format!("{}", line));
					markdown.push_str("\n");
				}
			} else {
				// If not too long, show all lines
				for line in lines {
					markdown.push_str(&format!("{}", line));
					markdown.push_str("\n");
				}
			}
			markdown.push_str("```\n\n");
		}

		markdown.push_str("---\n\n");
	}

	markdown
}

/// Render signatures as text output
pub fn render_signatures_text(signatures: &[FileSignature]) {
	if signatures.is_empty() {
		println!("No signatures found.");
		return;
	}

	println!("Found signatures in {} files:\n", signatures.len());

	for file in signatures {
		println!("╔══════════════════ File: {} ══════════════════", file.path);
		println!("║ Language: {}", file.language);

		// Show file comment if available
		if let Some(comment) = &file.file_comment {
			println!("║");
			println!("║ File description:");
			for line in comment.lines() {
				println!("║   {}", line);
			}
		}

		if file.signatures.is_empty() {
			println!("║");
			println!("║ No signatures found in this file.");
		} else {
			for signature in &file.signatures {
				println!("║");

				// Display line range if it spans multiple lines, otherwise just the start line
				let line_display = if signature.start_line == signature.end_line {
					format!("{}", signature.start_line + 1)
				} else {
					format!("{}-{}", signature.start_line + 1, signature.end_line + 1)
				};

				println!("║ {} `{}` (line {})", signature.kind, signature.name, line_display);

				// Show description if available
				if let Some(desc) = &signature.description {
					println!("║ Description:");
					for line in desc.lines() {
						println!("║   {}", line);
					}
				}

				// Format the signature for display
				println!("║ Signature:");
				let lines = signature.signature.lines().collect::<Vec<_>>();
				if lines.len() > 1 {
					println!("║ ┌────────────────────────────────────");
					for line in lines.iter().take(5) {
						println!("║ │ {}", line);
					}
					// If signature is too long, truncate it
					if lines.len() > 5 {
						println!("║ │ ... ({} more lines)", lines.len() - 5);
					}
					println!("║ └────────────────────────────────────");
				} else if !lines.is_empty() {
					println!("║   {}", lines[0]);
				}
			}
		}

		println!("╚════════════════════════════════════════\n");
	}
}

/// Render signatures as JSON
pub fn render_signatures_json(signatures: &[FileSignature]) -> Result<()> {
	let json = serde_json::to_string_pretty(signatures)?;
	println!("{}", json);
	Ok(())
}

// Main function to index files
pub async fn index_files(store: &Store, state: SharedState, config: &Config) -> Result<()> {
	let current_dir = state.read().current_directory.clone();
	let mut code_blocks_batch = Vec::new();
	let mut text_blocks_batch = Vec::new();
	let mut all_code_blocks = Vec::new(); // Store all code blocks for GraphRAG

	const BATCH_SIZE: usize = 10;
	let mut embedding_calls = 0;

	// Initialize GraphRAG state if enabled
	{
		let mut state_guard = state.write();
		state_guard.graphrag_enabled = config.graphrag.enabled;
		state_guard.graphrag_blocks = 0;
		state_guard.graphrag_files.clear();
	}

	// Use the ignore crate to respect .gitignore files
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
		if let Some(language) = detect_language(entry.path()) {
			if let Ok(contents) = fs::read_to_string(entry.path()) {
				let file_path = entry.path().to_string_lossy().to_string();
				process_file(
					&store,
					&contents,
					&file_path,
					language,
					&mut code_blocks_batch,
					&mut text_blocks_batch,
					&mut all_code_blocks,
					config,
					state.clone()
				).await?;

				state.write().indexed_files += 1;
				if code_blocks_batch.len() >= BATCH_SIZE {
					embedding_calls += code_blocks_batch.len();
					process_code_blocks_batch(&store, &code_blocks_batch, config).await?;
					code_blocks_batch.clear();
				}
				if text_blocks_batch.len() >= BATCH_SIZE {
					embedding_calls += text_blocks_batch.len();
					process_text_blocks_batch(&store, &text_blocks_batch, config).await?;
					text_blocks_batch.clear();
				}
			}
		}
	}

	if !code_blocks_batch.is_empty() {
		process_code_blocks_batch(&store, &code_blocks_batch, config).await?;
		embedding_calls += code_blocks_batch.len();
	}
	if !text_blocks_batch.is_empty() {
		process_text_blocks_batch(&store, &text_blocks_batch, config).await?;
		embedding_calls += text_blocks_batch.len();
	}

	// Build GraphRAG from all collected code blocks if enabled and if we found any blocks
	if config.graphrag.enabled && !all_code_blocks.is_empty() {
		let mut state_guard = state.write();
		state_guard.status_message = "Building GraphRAG knowledge graph...".to_string();
		drop(state_guard);

		// Initialize GraphBuilder
		let graph_builder = graphrag::GraphBuilder::new(config.clone()).await?;
		
		// Process code blocks to build the graph
		graph_builder.process_code_blocks(&all_code_blocks, Some(state.clone())).await?;
		
		// Update final state
		let mut state_guard = state.write();
		state_guard.status_message = "".to_string();
	}

	let mut state_guard = state.write();
	state_guard.indexing_complete = true;
	state_guard.embedding_calls = embedding_calls;

	Ok(())
}

// Function to handle file changes (for watch mode)
pub async fn handle_file_change(store: &Store, file_path: &str, config: &Config) -> Result<()> {
	// Create a state for tracking changes
	let state = state::create_shared_state();
	{
		let mut state_guard = state.write();
		state_guard.graphrag_enabled = config.graphrag.enabled;
		state_guard.graphrag_blocks = 0;
		state_guard.graphrag_files.clear();
	}

	// First, let's remove any existing code blocks for this file path
	store.remove_blocks_by_path(file_path).await?;

	// Now, if the file still exists, check if it should be indexed based on .gitignore rules
	let path = std::path::Path::new(file_path);
	if path.exists() {
		// Create a matcher that respects .gitignore rules
		let mut builder = ignore::gitignore::GitignoreBuilder::new(path.parent().unwrap_or_else(|| std::path::Path::new(".")));

		// Try to add .gitignore files from the project root up to the file's directory
		let parent_path = path.parent().unwrap_or_else(|| std::path::Path::new("."));
		let gitignore_path = parent_path.join(".gitignore");
		if gitignore_path.exists() {
			let _ = builder.add(&gitignore_path);
		}

		// Build the matcher
		if let Ok(matcher) = builder.build() {
			// Check if the file should be ignored
			if matcher.matched(path, path.is_dir()).is_ignore() {
				// File is in .gitignore, so don't index it
				return Ok(());
			}
		}

		// File is not ignored, so proceed with indexing
		if let Some(language) = detect_language(path) {
			if let Ok(contents) = fs::read_to_string(path) {
				let mut code_blocks_batch = Vec::new();
				let mut text_blocks_batch = Vec::new();
				let mut all_code_blocks = Vec::new(); // For GraphRAG

				process_file(
					store,
					&contents,
					file_path,
					language,
					&mut code_blocks_batch,
					&mut text_blocks_batch,
					&mut all_code_blocks,
					config,
					state.clone()
				).await?;

				if !code_blocks_batch.is_empty() {
					process_code_blocks_batch(store, &code_blocks_batch, config).await?;
				}
				if !text_blocks_batch.is_empty() {
					process_text_blocks_batch(store, &text_blocks_batch, config).await?;
				}
				
				// Update GraphRAG if enabled and we have new blocks
				if config.graphrag.enabled && !all_code_blocks.is_empty() {
					let graph_builder = graphrag::GraphBuilder::new(config.clone()).await?;
					graph_builder.process_code_blocks(&all_code_blocks, Some(state.clone())).await?;
				}
			}
		}
	}

	Ok(())
}

// Processes a single file, extracting code blocks and adding them to the batch
async fn process_file(
	store: &Store,
	contents: &str,
	file_path: &str,
	language: &str,
	code_blocks_batch: &mut Vec<CodeBlock>,
	text_blocks_batch: &mut Vec<TextBlock>,
	all_code_blocks: &mut Vec<CodeBlock>,
	config: &Config,
	state: SharedState,
) -> Result<()> {
	let mut parser = Parser::new();
	
	// Get force_reindex flag from state
	let force_reindex = state::create_shared_state().read().force_reindex;

	// Get the language implementation
	let lang_impl = match languages::get_language(language) {
		Some(impl_) => impl_,
		None => return Ok(()),  // Skip unsupported languages
	};

	// Set the parser language
	parser.set_language(&lang_impl.get_ts_language())?;

	let tree = parser.parse(contents, None).unwrap_or_else(|| parser.parse("", None).unwrap());
	let mut code_regions = Vec::new();

	extract_meaningful_regions(tree.root_node(), contents, &lang_impl, &mut code_regions);

	// Track the number of blocks we added to all_code_blocks for GraphRAG
	let mut graphrag_blocks_added = 0;

	for region in code_regions {
		// Use a hash that's unique to both content and path
		let content_hash = calculate_unique_content_hash(&region.content, file_path);
		
		// Skip the check if force_reindex is true
		let exists = !force_reindex && store.content_exists(&content_hash, "code_blocks").await?;
		if !exists {
			let code_block = CodeBlock {
				path: file_path.to_string(),
				hash: content_hash,
				language: lang_impl.name().to_string(),
				content: region.content.clone(),
				symbols: region.symbols.clone(),
				start_line: region.start_line,
				end_line: region.end_line,
				distance: None,  // No relevance score when indexing
			};
			
			// Add to batch for embedding
			code_blocks_batch.push(code_block.clone());
			
			// Add to all code blocks for GraphRAG
			if config.graphrag.enabled {
				all_code_blocks.push(code_block);
				graphrag_blocks_added += 1;
			}
		} else if config.graphrag.enabled {
			// If skipping because block exists, but we need for GraphRAG, fetch from store
			if let Ok(existing_block) = store.get_code_block_by_hash(&content_hash).await {
				// Add the existing block to the GraphRAG collection
				all_code_blocks.push(existing_block);
				graphrag_blocks_added += 1;
			}
		}
	}

	let content_hash = calculate_unique_content_hash(contents, file_path);
	let text_exists = !force_reindex && store.content_exists(&content_hash, "text_blocks").await?;
	if !text_exists {
		text_blocks_batch.push(TextBlock {
			path: file_path.to_string(),
			language: lang_impl.name().to_string(),
			hash: content_hash,
			content: contents.to_string(),
			start_line: 0,
			end_line: contents.lines().count(),
		});
	}

	// Update GraphRAG state if enabled and blocks were added
	if config.graphrag.enabled && graphrag_blocks_added > 0 {
		let mut state_guard = state.write();
		state_guard.graphrag_blocks += graphrag_blocks_added;
		state_guard.graphrag_files.insert(file_path.to_string());
	}

	Ok(())
}

/// Represents a meaningful code block/region.
struct CodeRegion {
	content: String,
	symbols: Vec<String>,
	start_line: usize,
	end_line: usize,
}

/// Recursively extracts meaningful regions based on node kinds.
fn extract_meaningful_regions(
	node: Node,
	contents: &str,
	lang_impl: &Box<dyn languages::Language>,
	regions: &mut Vec<CodeRegion>,
) {
	let meaningful_kinds = lang_impl.get_meaningful_kinds();
	let node_kind = node.kind();

	if meaningful_kinds.contains(&node_kind) {
		let (combined_content, start_line) = combine_with_preceding_comments(node, contents);
		let end_line = node.end_position().row;
		let symbols = lang_impl.extract_symbols(node, contents);
		
		// Only create a region if we have meaningful content
		if !combined_content.trim().is_empty() {
			// Ensure we have at least one symbol by using the node kind if necessary
			let mut final_symbols = symbols;
			if final_symbols.is_empty() {
				// Create a default symbol from the node kind
				final_symbols.push(format!("{}_{}", node_kind, start_line));
			}
			
			regions.push(CodeRegion { 
				content: combined_content, 
				symbols: final_symbols, 
				start_line, 
				end_line 
			});
		}
		return;
	}

	let mut cursor = node.walk();
	if cursor.goto_first_child() {
		loop {
			extract_meaningful_regions(cursor.node(), contents, lang_impl, regions);
			if !cursor.goto_next_sibling() { break; }
		}
	}
}

/// Combines preceding comment or attribute nodes with a declaration node.
fn combine_with_preceding_comments(node: Node, contents: &str) -> (String, usize) {
	let mut combined_start = node.start_position().row;
	let mut snippet = String::new();
	if let Some(parent) = node.parent() {
		let mut cursor = parent.walk();
		let mut preceding = Vec::new();
		for child in parent.children(&mut cursor) {
			if child.id() == node.id() { break; } else { preceding.push(child); }
		}
		if let Some(last) = preceding.last() {
			let kind = last.kind();
			if kind.contains("comment") || kind.contains("attribute") {
				combined_start = last.start_position().row;
				snippet.push_str(&contents[last.start_byte()..last.end_byte()]);
				snippet.push('\n');
			}
		}
	}
	snippet.push_str(&contents[node.start_byte()..node.end_byte()]);
	(snippet, combined_start)
}

async fn process_code_blocks_batch(store: &Store, blocks: &[CodeBlock], config: &Config) -> Result<()> {
	let contents: Vec<String> = blocks.iter().map(|b| b.content.clone()).collect();
	let embeddings = generate_embeddings_batch(contents, true, config).await?;
	store.store_code_blocks(blocks, embeddings).await?;
	Ok(())
}

async fn process_text_blocks_batch(store: &Store, blocks: &[TextBlock], config: &Config) -> Result<()> {
	let contents: Vec<String> = blocks.iter().map(|b| b.content.clone()).collect();
	let embeddings = generate_embeddings_batch(contents, false, config).await?;
	store.store_text_blocks(blocks, embeddings).await?;
	Ok(())
}
