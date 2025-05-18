// Module for search functionality

use crate::store::{Store, CodeBlock};
use anyhow::Result;
use std::collections::HashSet;

// Render code blocks in a user-friendly format
pub fn render_code_blocks(blocks: &[CodeBlock]) {
	if blocks.is_empty() {
		println!("No code blocks found for the query.");
		return;
	}

	println!("Found {} code blocks:\n", blocks.len());

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
		println!("╔══════════════════ File: {} ══════════════════", file_path);

		for (idx, block) in file_blocks.iter().enumerate() {
			println!("║");
			println!("║ Block {} of {} in file", idx + 1, file_blocks.len());
			println!("║ Language: {}", block.language);
			println!("║ Lines: {}-{}", block.start_line, block.end_line);

			if !block.symbols.is_empty() {
				println!("║ Symbols:");
				for symbol in &block.symbols {
					// Only show non-type symbols to users
					if !symbol.contains("_") {
						println!("║   • {}", symbol);
					}
				}
			}

			println!("║ Content:");
			println!("║ ┌────────────────────────────────────");
			for line in block.content.lines() {
				println!("║ │ {}", line);
			}
			println!("║ └────────────────────────────────────");
		}

		println!("╚════════════════════════════════════════\n");
	}
}

// Render search results as JSON
pub fn render_results_json(results: &[CodeBlock]) -> Result<(), anyhow::Error> {
	let json = serde_json::to_string_pretty(results)?;
	println!("{}", json);
	Ok(())
}

// Expand symbols in code blocks to include related code
pub async fn expand_symbols(store: &Store, code_blocks: Vec<CodeBlock>) -> Result<Vec<CodeBlock>, anyhow::Error> {
    let mut expanded_blocks = code_blocks.clone();
    let mut symbol_refs = Vec::new();

    // Collect all symbols from the code blocks
    for block in &code_blocks {
        for symbol in &block.symbols {
            // Skip the type symbols (like "function_definition") and only include actual named symbols
            if !symbol.contains("_") && symbol.chars().next().map_or(false, |c| c.is_alphabetic()) {
                symbol_refs.push(symbol.clone());
            }
        }
    }

    // Track files we've already visited to avoid duplication
    let mut visited_files = HashSet::new();
    for block in &expanded_blocks {
        visited_files.insert(block.path.clone());
    }

    // Deduplicate symbols
    symbol_refs.sort();
    symbol_refs.dedup();

    println!("Found {} symbols to expand", symbol_refs.len());

    // For each symbol, find code blocks that contain it
    for symbol in symbol_refs {
        if let Some(block) = store.get_code_block_by_symbol(&symbol).await? {
            // Check if we already have this block by its hash
            if !expanded_blocks.iter().any(|b| b.hash == block.hash) {
                // Add dependencies we haven't seen before
                expanded_blocks.push(block);
            }
        }
    }

    // Sort blocks by file path and line number
    expanded_blocks.sort_by(|a, b| {
        let path_cmp = a.path.cmp(&b.path);
        if path_cmp == std::cmp::Ordering::Equal {
            a.start_line.cmp(&b.start_line)
        } else {
            path_cmp
        }
    });

    Ok(expanded_blocks)
}