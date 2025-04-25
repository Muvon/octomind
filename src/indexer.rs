use crate::content;
use crate::state::SharedState;
use crate::store::{Store, CodeBlock, TextBlock};
use crate::config::Config;
use std::fs;
use walkdir::WalkDir;
use tree_sitter::{Parser, Node, TreeCursor};
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

/// Improved process_file that uses recursive traversal for grouping meaningful blocks.
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

    // Instead of iterating only immediate children,
    // recursively traverse the tree to capture meaningful items.
    let mut code_regions = Vec::new();
    extract_meaningful_regions(tree.root_node(), contents, language, &mut code_regions);

    for region in code_regions {
        // region.content has the combined code (e.g. doc comments plus function body)
        let content_hash = content::calculate_content_hash(&region.content);

        // Skip if already stored
        if !store.content_exists(&content_hash, "code_blocks").await? {
            code_blocks_batch.push(CodeBlock {
                path: file_path.to_string(),
                hash: content_hash,
                language: language.to_string(),
                content: region.content,
                symbols: region.symbols,
                start_line: region.start_line,
                end_line: region.end_line,
            });
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

/// A helper struct that represents a meaningful code block/region.
struct CodeRegion {
    content: String,
    symbols: Vec<String>,
    start_line: usize,
    end_line: usize,
}

/// Recursively extracts meaningful regions. Adjust the heuristics as needed per language.
fn extract_meaningful_regions(
    node: Node,
    contents: &str,
    language: &str,
    regions: &mut Vec<CodeRegion>,
) {
    // This example is tuned for Rust – you might do similar things for other languages.
    // For Rust, consider a region meaningful if it is a function, struct, enum, impl, etc.
    // You can add more kinds as needed.
    let meaningful_kinds = match language {
        "rust" => vec![
            "function_item",
            "struct_item",
            "enum_item",
            "impl_item",
            // You can add "macro_rules" or other items here if needed.
        ],
        // For other languages, list the kind names as appropriate.
        _ => Vec::new(),
    };

    let node_kind = node.kind();
    if meaningful_kinds.contains(&node_kind) {
        // Optionally: check for comments or attribute nodes preceding this declaration.
        // For example, in Rust, attributes (like #[derive(...)]) might be children of the declaration,
        // OR they might appear as siblings. Here we check preceding siblings for comments/attributes.
        let (combined_content, start_line) = combine_with_preceding_comments(node, contents);

        let end_line = node.end_position().row;
        let symbols = extract_symbols(node, contents);
        regions.push(CodeRegion {
            content: combined_content,
            symbols,
            start_line,
            end_line,
        });
        // Do not recurse further into these nodes if they already constitute a region.
        return;
    }

    // Otherwise, recursively process children.
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            extract_meaningful_regions(cursor.node(), contents, language, regions);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Combines preceding comment or attribute nodes with the given declaration node.
/// Returns the combined content and the starting line number.
fn combine_with_preceding_comments(node: Node, contents: &str) -> (String, usize) {
    // Start with the current node.
    let mut combined_start = node.start_position().row;
    let mut snippet = String::new();
    let mut current = node;

    // Iterate over siblings backwards.
    if let Some(parent) = node.parent() {
        let mut cursor = parent.walk();
        // Go through all children and pick nodes that are immediately before `node`
        let mut preceding_nodes = Vec::new();
        let mut found = false;
        for child in parent.children(&mut cursor) {
            if child.id() == node.id() {
                found = true;
                break;
            } else {
                preceding_nodes.push(child);
            }
        }
        if found {
            // If the last preceding node is a comment or attribute, include it.
            if let Some(last) = preceding_nodes.last() {
                let kind = last.kind();
                if kind.contains("comment") || kind.contains("attribute") {
                    combined_start = last.start_position().row;
                    snippet.push_str(&contents[last.start_byte()..last.end_byte()]);
                    snippet.push('\n');
                }
            }
        }
    }
    // Append the original node content.
    snippet.push_str(&contents[node.start_byte()..node.end_byte()]);
    (snippet, combined_start)
}

/// Improved symbol extraction:
///
/// Instead of simply recursing and recording any identifier, you can adjust this
/// to extract only symbols you deem “meaningful”. You might even use Tree-sitter’s Query API here.
fn extract_symbols(node: Node, contents: &str) -> Vec<String> {
    let mut symbols = Vec::new();

    // Try to extract symbol based on the type of declaration
    match node.kind() {
        "function_item" => {
            // In Rust, the function name is usually a child called "identifier".
            for child in node.children(&mut node.walk()) {
                if child.kind() == "identifier" {
                    if let Ok(name) = child.utf8_text(contents.as_bytes()) {
                        symbols.push(name.to_string());
                    }
                    break;
                }
            }
        }
        "struct_item" | "enum_item" | "impl_item" => {
            // Look for the name in these declarations.
            for child in node.children(&mut node.walk()) {
                if child.kind() == "identifier" || child.kind().contains("name") {
                    if let Ok(name) = child.utf8_text(contents.as_bytes()) {
                        symbols.push(name.to_string());
                    }
                    break;
                }
            }
        }
        _ => {
            // For other nodes, fallback to extracting any identifier-like node
            extract_identifiers(node, contents, &mut symbols);
        }
    }

    symbols
}

/// A more selective recursive extraction that only targets identifier-like nodes.
fn extract_identifiers(node: Node, contents: &str, symbols: &mut Vec<String>) {
    let kind = node.kind();
    if (kind.contains("identifier") || kind.contains("name")) && kind != "property_identifier" {
        if let Ok(text) = node.utf8_text(contents.as_bytes()) {
            let text = text.trim();
            if !text.is_empty() {
                symbols.push(text.to_string());
            }
        }
    }

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
