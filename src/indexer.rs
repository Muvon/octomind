use crate::content;
use crate::state::SharedState;
use crate::store::{Store, CodeBlock, TextBlock};
use crate::config::Config;
use std::fs;
use walkdir::WalkDir;
use tree_sitter::{Parser, Node};
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
        "go" => tree_sitter_go::LANGUAGE,
        "cpp" => tree_sitter_cpp::LANGUAGE,
        "bash" => tree_sitter_bash::LANGUAGE,
        "ruby" => tree_sitter_ruby::LANGUAGE,
        _ => return Ok(()),
    };
    parser.set_language(&ts_lang.into())?;

    let tree = parser.parse(contents, None).unwrap_or_else(|| parser.parse("", None).unwrap());
    let mut code_regions = Vec::new();
    extract_meaningful_regions(tree.root_node(), contents, language, &mut code_regions);

    for region in code_regions {
        let content_hash = content::calculate_content_hash(&region.content);
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

/// Represents a meaningful code block/region.
struct CodeRegion {
    content: String,
    symbols: Vec<String>,
    start_line: usize,
    end_line: usize,
}

/// Returns Tree-sitter node kinds considered meaningful per language.
fn get_meaningful_kinds(language: &str) -> Vec<&'static str> {
    match language {
        "rust" => vec!["function_item", "struct_item", "enum_item", "impl_item"],
        "javascript" | "typescript" => vec![
            "function_declaration", "method_definition", "class_declaration", "arrow_function",
        ],
        "python" => vec!["function_definition", "class_definition"],
        "go" => vec!["function_declaration", "method_declaration"],
        "cpp" => vec!["function_definition", "class_specifier", "struct_specifier"],
        "php" => vec![
            "function_definition", "class_declaration", "method_declaration",
            "trait_declaration", "interface_declaration",
        ],
        "bash" => vec!["function_definition"],
        "ruby" => vec!["method", "class"],
        _ => Vec::new(),
    }
}

/// Recursively extracts meaningful regions based on node kinds.
fn extract_meaningful_regions(
    node: Node,
    contents: &str,
    language: &str,
    regions: &mut Vec<CodeRegion>,
) {
    let meaningful_kinds = get_meaningful_kinds(language);
    let node_kind = node.kind();
    if meaningful_kinds.contains(&node_kind) {
        let (combined_content, start_line) = combine_with_preceding_comments(node, contents);
        let end_line = node.end_position().row;
        let symbols = extract_symbols(node, contents);
        regions.push(CodeRegion { content: combined_content, symbols, start_line, end_line });
        return;
    }
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            extract_meaningful_regions(cursor.node(), contents, language, regions);
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

/// Extracts symbols for a code block.
fn extract_symbols(node: Node, contents: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    match node.kind() {
        "function_item" => {
            for child in node.children(&mut node.walk()) {
                if child.kind() == "identifier" {
                    if let Ok(n) = child.utf8_text(contents.as_bytes()) { symbols.push(n.to_string()); }
                    break;
                }
            }
        }
        "struct_item" | "enum_item" | "impl_item" => {
            for child in node.children(&mut node.walk()) {
                if child.kind() == "identifier" || child.kind().contains("name") {
                    if let Ok(n) = child.utf8_text(contents.as_bytes()) { symbols.push(n.to_string()); }
                    break;
                }
            }
        }
        _ => extract_identifiers(node, contents, &mut symbols),
    }
    symbols
}

/// Recursively collect identifier-like nodes.
fn extract_identifiers(node: Node, contents: &str, symbols: &mut Vec<String>) {
    let kind = node.kind();
    if (kind.contains("identifier") || kind.contains("name")) && kind != "property_identifier" {
        if let Ok(text) = node.utf8_text(contents.as_bytes()) {
            let t = text.trim(); if !t.is_empty() { symbols.push(t.to_string()); }
        }
    }
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            extract_identifiers(cursor.node(), contents, symbols);
            if !cursor.goto_next_sibling() { break; }
        }
    }
}

async fn process_code_blocks_batch(store: &Store, blocks: &[CodeBlock], config: &Config) -> Result<()> {
    let contents: Vec<String> = blocks.iter().map(|b| b.content.clone()).collect();
    let embeddings = content::generate_embeddings_batch(contents, true, config).await?;
    store.store_code_blocks(blocks, embeddings).await?;
    Ok(())
}

async fn process_text_blocks_batch(store: &Store, blocks: &[TextBlock], config: &Config) -> Result<()> {
    let contents: Vec<String> = blocks.iter().map(|b| b.content.clone()).collect();
    let embeddings = content::generate_embeddings_batch(contents, false, config).await?;
    store.store_text_blocks(blocks, embeddings).await?;
    Ok(())
}