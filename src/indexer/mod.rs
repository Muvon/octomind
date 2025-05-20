// Indexer module for OctoDev
// Handles code indexing, embedding, and search functionality

mod embed; // Embedding generation - moving from content.rs
mod search; // Search functionality
mod languages; // Language-specific processors

pub use embed::*;
pub use search::*;

use crate::state::SharedState;
use crate::store::{Store, CodeBlock, TextBlock};
use crate::config::Config;
use std::fs;
// We're using ignore::WalkBuilder instead of walkdir::WalkDir
use tree_sitter::{Parser, Node};
use anyhow::Result;
use ignore;

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

// Main function to index files
pub async fn index_files(store: &Store, state: SharedState, config: &Config) -> Result<()> {
	let current_dir = state.read().current_directory.clone();
	let mut code_blocks_batch = Vec::new();
	let mut text_blocks_batch = Vec::new();

	const BATCH_SIZE: usize = 10;
	let mut embedding_calls = 0;

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

	let mut state_guard = state.write();
	state_guard.indexing_complete = true;
	state_guard.embedding_calls = embedding_calls;

	Ok(())
}

// Function to handle file changes (for watch mode)
pub async fn handle_file_change(store: &Store, file_path: &str, config: &Config) -> Result<()> {
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

				process_file(
					store,
					&contents,
					file_path,
					language,
					&mut code_blocks_batch,
					&mut text_blocks_batch,
				).await?;

				if !code_blocks_batch.is_empty() {
					process_code_blocks_batch(store, &code_blocks_batch, config).await?;
				}
				if !text_blocks_batch.is_empty() {
					process_text_blocks_batch(store, &text_blocks_batch, config).await?;
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
) -> Result<()> {
	let mut parser = Parser::new();

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

	for region in code_regions {
		// Use a hash that's unique to both content and path
		let content_hash = calculate_unique_content_hash(&region.content, file_path);
		if !store.content_exists(&content_hash, "code_blocks").await? {
			code_blocks_batch.push(CodeBlock {
				path: file_path.to_string(),
				hash: content_hash,
				language: lang_impl.name().to_string(),
				content: region.content,
				symbols: region.symbols,
				start_line: region.start_line,
				end_line: region.end_line,
				distance: None,  // No relevance score when indexing
			});
		}
	}

	let content_hash = calculate_unique_content_hash(contents, file_path);
	if !store.content_exists(&content_hash, "text_blocks").await? {
		text_blocks_batch.push(TextBlock {
			path: file_path.to_string(),
			language: lang_impl.name().to_string(),
			hash: content_hash,
			content: contents.to_string(),
			start_line: 0,
			end_line: contents.lines().count(),
		});
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
		regions.push(CodeRegion { content: combined_content, symbols, start_line, end_line });
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
