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

			// Show relevance score if available
			if let Some(distance) = block.distance {
				println!("║ Relevance: {:.4}", distance);
			}

			if !block.symbols.is_empty() {
				println!("║ Symbols:");
				// Deduplicate symbols in display
				let mut display_symbols = block.symbols.clone();
				display_symbols.sort();
				display_symbols.dedup();

				for symbol in display_symbols {
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

// Expand symbols in code blocks to include related code while maintaining relevance order
pub async fn expand_symbols(store: &Store, code_blocks: Vec<CodeBlock>) -> Result<Vec<CodeBlock>, anyhow::Error> {
	// We'll keep original blocks at the top with their original order
	let mut expanded_blocks = Vec::new();
	let mut original_hashes = HashSet::new();

	// Add original blocks and keep track of their hashes
	for block in &code_blocks {
		expanded_blocks.push(block.clone());
		original_hashes.insert(block.hash.clone());
	}

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

	// Deduplicate symbols
	symbol_refs.sort();
	symbol_refs.dedup();

	println!("Found {} unique symbols to expand", symbol_refs.len());

	// Store expanded blocks to sort them by symbol count later
	let mut additional_blocks = Vec::new();

	// For each symbol, find code blocks that contain it
	for symbol in &symbol_refs {  // Use a reference to avoid moving symbol_refs
		if let Some(block) = store.get_code_block_by_symbol(symbol).await? {
			// Check if we already have this block (avoid duplicates)
			if !original_hashes.contains(&block.hash) &&
			!additional_blocks.iter().any(|b: &CodeBlock| b.hash == block.hash) {
				// Add dependencies we haven't seen before
				additional_blocks.push(block);
			}
		}
	}

	// Sort additional blocks by symbol count (more symbols = more relevant)
	// This is a heuristic to put more complex/relevant blocks first
	additional_blocks.sort_by(|a, b| {
		// First try to sort by number of matching symbols (more matches = more relevant)
		let a_matches = a.symbols.iter()
			.filter(|s| symbol_refs.contains(s))
			.count();
		let b_matches = b.symbols.iter()
			.filter(|s| symbol_refs.contains(s))
			.count();

		// Primary sort by symbol match count (descending)
		let match_cmp = b_matches.cmp(&a_matches);

		if match_cmp == std::cmp::Ordering::Equal {
			// Secondary sort by file path and line number when match counts are equal
			let path_cmp = a.path.cmp(&b.path);
			if path_cmp == std::cmp::Ordering::Equal {
				a.start_line.cmp(&b.start_line)
			} else {
				path_cmp
			}
		} else {
			match_cmp
		}
	});

	// Add the sorted additional blocks to our results
	expanded_blocks.extend(additional_blocks);

	Ok(expanded_blocks)
}
