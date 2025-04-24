use crate::content;
use crate::state::SharedState;
use crate::store::{Store, CodeBlock, TextBlock};
use crate::config::Config;
use std::fs;
use walkdir::WalkDir;
use tree_sitter::Parser;
use anyhow::Result;

fn detect_language(path: &std::path::Path) -> Option<&str> {
    match path.extension()?.to_str()? {
        "rs" => Some("rust"),
        "php" => Some("php"),
        "py" => Some("python"),
        "js" => Some("javascript"),
        "ts" => Some("typescript"),
        "jsx" | "tsx" => Some("typescript"),
        "json" => Some("json"),
        // Skipping markdown due to type conversion issues
        // "md" => Some("markdown"),
        "go" => Some("go"),
        "cpp" | "cc" | "cxx" | "c++" | "hpp" | "h" => Some("cpp"),
        "sh" | "bash" => Some("bash"),
        "rb" => Some("ruby"),
        _ => None,
    }
}

pub async fn index_files(store: &Store, state: SharedState, config: &Config) -> Result<()> {
	let current_dir = state.read().current_directory.clone();
	let mut code_blocks_batch = Vec::new();
	let mut text_blocks_batch = Vec::new();

	const BATCH_SIZE: usize = 10;
	let mut embedding_calls = 0;

	for entry in WalkDir::new(current_dir)
		.into_iter()
		.filter_map(|e| e.ok())
		.filter(|e| e.file_type().is_file())
	{
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
				// Process batches when they reach the size limit
				let code_blocks_len = code_blocks_batch.len();
				if code_blocks_len >= BATCH_SIZE {
					embedding_calls += code_blocks_len;
					process_code_blocks_batch(&store, &code_blocks_batch, config).await?;
					code_blocks_batch.clear();
				}
				let text_blocks_len = text_blocks_batch.len();
				if text_blocks_len >= BATCH_SIZE {
					embedding_calls += text_blocks_len;
					process_text_blocks_batch(&store, &text_blocks_batch, config).await?;
					text_blocks_batch.clear();
				}
			}
		}
	}

	// Process remaining items
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

async fn process_file(
    store: &Store,
    contents: &str,
    file_path: &str,
    language: &str,
    code_blocks_batch: &mut Vec<CodeBlock>,
    text_blocks_batch: &mut Vec<TextBlock>,
) -> Result<()> {
    let mut parser = Parser::new();
    let ts_lang = match language {
        "rust" => tree_sitter_rust::LANGUAGE,
        "php" => tree_sitter_php::LANGUAGE_PHP,
        "python" => tree_sitter_python::LANGUAGE,
        "javascript" => tree_sitter_javascript::LANGUAGE,
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        "json" => tree_sitter_json::LANGUAGE,
        // Skipping markdown due to type conversion issues
        // "markdown" => tree_sitter_markdown::language().into(),
        "go" => tree_sitter_go::LANGUAGE,
        "cpp" => tree_sitter_cpp::LANGUAGE,
        "bash" => tree_sitter_bash::LANGUAGE,
        "ruby" => tree_sitter_ruby::LANGUAGE,
        _ => return Ok(()),
    };
    parser.set_language(&ts_lang.into())?;

    let tree = parser.parse(contents, None).unwrap_or_else(|| {
        // If parsing fails, just return an empty tree
        parser.parse("", None).unwrap()
    });

    let mut cursor = tree.walk();
    // let mut has_traversed = false;

    // Try to go to first child, if not then the file is empty or unparsable
    if cursor.goto_first_child() {
        // has_traversed = true;
        // Process each top-level node
        loop {
            let node = cursor.node();

            // Extract meaningful code blocks based on the node type
            let kind = node.kind();

            // Skip tiny nodes or ones that don't represent meaningful code blocks
            if node.end_byte() - node.start_byte() < 10 ||
               kind.contains("comment") ||
               kind == "string" ||
               kind == "string_literal" {
                if !cursor.goto_next_sibling() {
                    break;
                }
                continue;
            }

            let content = contents[node.start_byte()..node.end_byte()].to_string();
            let content_hash = content::calculate_content_hash(&content);

            // Extract symbols from the node
            // Replace the existing symbols extraction with:
            let symbols = extract_symbols(node, contents);

            if !store.content_exists(&content_hash, "code_blocks").await? {
                code_blocks_batch.push(CodeBlock {
                    path: file_path.to_string(),
                    hash: content_hash,
                    language: language.to_string(),
                    content,
                    symbols,
                    start_line: node.start_position().row,
                    end_line: node.end_position().row,
                });
            }

            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }

    // Always store the full file content for context searches
    let content_hash = content::calculate_content_hash(contents);
    if !store.content_exists(&content_hash, "text_blocks").await? {
        text_blocks_batch.push(TextBlock {
            path: file_path.to_string(),
            hash: content_hash,
            content: contents.to_string(),
            start_line: 0,
            end_line: contents.lines().count(),
        });
    }

    Ok(())
}

async fn process_code_blocks_batch(store: &Store, blocks: &[CodeBlock], config: &Config) -> Result<()> {
	let contents: Vec<String> = blocks.iter().map(|block| block.content.clone()).collect();
	let embeddings = content::generate_embeddings_batch(contents, true, config).await?;
	store.store_code_blocks(blocks, embeddings).await?;
	Ok(())
}

async fn process_text_blocks_batch(store: &Store, blocks: &[TextBlock], config: &Config) -> Result<()> {
	let contents: Vec<String> = blocks.iter().map(|block| block.content.clone()).collect();
	let embeddings = content::generate_embeddings_batch(contents, false, config).await?;
	store.store_text_blocks(blocks, embeddings).await?;
	Ok(())
}

// Extract symbols from the node using a recursive approach
fn extract_symbols(node: tree_sitter::Node, contents: &str) -> Vec<String> {
    let mut symbols = Vec::new();

    // Add the node kind as a symbol type
    symbols.push(node.kind().to_string());

    // Extract identifiers recursively
    extract_identifiers(node, contents, &mut symbols);

    symbols
}

fn extract_identifiers(node: tree_sitter::Node, contents: &str, symbols: &mut Vec<String>) {
    // Check if this node is an identifier
    let kind = node.kind();
    if kind.contains("identifier") ||
       kind.contains("name") ||
       kind == "identifier" {
        if let Some(symbol_text) = node.utf8_text(contents.as_bytes()).ok() {
            // Only add non-empty identifiers
            if !symbol_text.trim().is_empty() {
                symbols.push(symbol_text.to_string());
            }
        }
    }

    // Recursively check children
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            extract_identifiers(cursor.node(), contents, symbols);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}
