use clap::Args;

use octodev::config::Config;
use octodev::store::Store;
use octodev::indexer;

#[derive(Args, Debug)]
pub struct SearchArgs {
	/// Search query
	pub query: String,

	/// Expand all symbols in matching code blocks
	#[arg(long, short)]
	pub expand: bool,

	/// Output in JSON format
	#[arg(long)]
	pub json: bool,

	/// Output in Markdown format
	#[arg(long)]
	pub md: bool,

	/// Search mode: all (default), code, docs, or text
	#[arg(long, default_value = "all")]
	pub mode: String,
}

pub async fn execute(store: &Store, args: &SearchArgs, config: &Config) -> Result<(), anyhow::Error> {
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let index_path = octodev_dir.join("storage");

	// Check if we have an index already; if not, inform the user but don't auto-index
	if !index_path.exists() {
		return Err(anyhow::anyhow!("No index found. Please run 'octodev index' first to create an index."));
	}

	println!("Searching for: {}", args.query);
	println!("Using embedding provider: {:?}", config.embedding_provider);
	println!("Search mode: {}", args.mode);

	// Generate embeddings for the query
	let embeddings = indexer::generate_embeddings(&args.query, true, config).await?;

	// Validate search mode
	let search_mode = match args.mode.as_str() {
		"all" | "code" | "docs" | "text" => args.mode.as_str(),
		_ => {
			return Err(anyhow::anyhow!("Invalid search mode '{}'. Use 'all', 'code', 'docs', or 'text'.", args.mode));
		}
	};

	// Search based on mode
	match search_mode {
		"code" => {
			// Search only code blocks
			let mut results = store.get_code_blocks(embeddings).await?;

			// If expand flag is set, expand symbols in the results
			if args.expand {
				println!("Expanding symbols...");
				results = indexer::expand_symbols(store, results).await?;
			}

			// Output the results
			if args.json {
				indexer::render_results_json(&results)?
			} else if args.md {
				let markdown = indexer::code_blocks_to_markdown(&results);
				println!("{}", markdown);
			} else {
				indexer::render_code_blocks(&results);
			}
		},
		"docs" => {
			// Search only document blocks
			let results = store.get_document_blocks(embeddings).await?;

			// Output the results
			if args.json {
				let json = serde_json::to_string_pretty(&results)?;
				println!("{}", json);
			} else if args.md {
				let markdown = indexer::document_blocks_to_markdown(&results);
				println!("{}", markdown);
			} else {
				// Render documents in a readable format
				render_document_blocks(&results);
			}
		},
		"text" => {
			// Search only text blocks
			let results = store.get_text_blocks(embeddings).await?;

			// Output the results
			if args.json {
				let json = serde_json::to_string_pretty(&results)?;
				println!("{}", json);
			} else if args.md {
				let markdown = indexer::text_blocks_to_markdown(&results);
				println!("{}", markdown);
			} else {
				// Render text blocks in a readable format
				render_text_blocks(&results);
			}
		},
		"all" => {
			// Search code, documents, and text blocks
			let code_results = store.get_code_blocks(embeddings.clone()).await?;
			let doc_results = store.get_document_blocks(embeddings.clone()).await?;
			let text_results = store.get_text_blocks(embeddings).await?;

			// If expand flag is set, expand symbols in code results
			let mut final_code_results = code_results;
			if args.expand {
				println!("Expanding symbols...");
				final_code_results = indexer::expand_symbols(store, final_code_results).await?;
			}

			// Output combined results
			if args.json {
				// Create a combined JSON structure
				let combined = serde_json::json!({
					"code_blocks": final_code_results,
					"document_blocks": doc_results,
					"text_blocks": text_results
				});
				println!("{}", serde_json::to_string_pretty(&combined)?);
			} else if args.md {
				// Render all sections in markdown
				let mut combined_markdown = String::new();

				if !doc_results.is_empty() {
					combined_markdown.push_str("# Documentation Results\n\n");
					combined_markdown.push_str(&indexer::document_blocks_to_markdown(&doc_results));
					combined_markdown.push('\n');
				}

				if !final_code_results.is_empty() {
					combined_markdown.push_str("# Code Results\n\n");
					combined_markdown.push_str(&indexer::code_blocks_to_markdown(&final_code_results));
					combined_markdown.push('\n');
				}

				if !text_results.is_empty() {
					combined_markdown.push_str("# Text Results\n\n");
					combined_markdown.push_str(&indexer::text_blocks_to_markdown(&text_results));
				}

				if combined_markdown.is_empty() {
					combined_markdown.push_str("No results found for the query.");
				}

				println!("{}", combined_markdown);
			} else {
				// Render all sections in text format
				if !doc_results.is_empty() {
					println!("=== DOCUMENTATION RESULTS ===\n");
					render_document_blocks(&doc_results);
					println!("\n");
				}

				if !final_code_results.is_empty() {
					println!("=== CODE RESULTS ===\n");
					indexer::render_code_blocks(&final_code_results);
					println!("\n");
				}

				if !text_results.is_empty() {
					println!("=== TEXT RESULTS ===\n");
					render_text_blocks(&text_results);
				}

				if doc_results.is_empty() && final_code_results.is_empty() && text_results.is_empty() {
					println!("No results found for the query.");
				}
			}
		},
		_ => unreachable!(),
	}

	Ok(())
}

fn render_text_blocks(blocks: &[octodev::store::TextBlock]) {
	if blocks.is_empty() {
		println!("No text blocks found.");
		return;
	}

	println!("Found {} text blocks:\n", blocks.len());

	// Group blocks by file path for better organization
	let mut blocks_by_file: std::collections::HashMap<String, Vec<&octodev::store::TextBlock>> = std::collections::HashMap::new();

	for block in blocks {
		blocks_by_file
			.entry(block.path.clone())
			.or_default()
			.push(block);
	}

	// Print results organized by file
	for (file_path, file_blocks) in blocks_by_file.iter() {
		println!("╔══════════════════ File: {} ══════════════════", file_path);

		for (idx, block) in file_blocks.iter().enumerate() {
			println!("║");
			println!("║ Block {} of {}: {}", idx + 1, file_blocks.len(), block.language);
			println!("║ Lines: {}-{}", block.start_line, block.end_line);

			// Show relevance score if available
			if let Some(distance) = block.distance {
				println!("║ Relevance: {:.4}", distance);
			}

			println!("║");
			println!("║ Content:");

			// Display content with proper indentation
			let lines: Vec<&str> = block.content.lines().collect();
			if lines.len() > 15 {
				// Show first 10 lines
				for line in lines.iter().take(10) {
					println!("║   {}", line);
				}
				println!("║   ... ({} more lines omitted)", lines.len() - 15);
				// Show last 5 lines
				for line in lines.iter().skip(lines.len() - 5) {
					println!("║   {}", line);
				}
			} else {
				// If not too long, show all lines
				for line in lines {
					println!("║   {}", line);
				}
			}
		}

		println!("╚════════════════════════════════════════\n");
	}
}

fn render_document_blocks(blocks: &[octodev::store::DocumentBlock]) {
	if blocks.is_empty() {
		println!("No documentation blocks found.");
		return;
	}

	println!("Found {} documentation sections:\n", blocks.len());

	// Group blocks by file path for better organization
	let mut blocks_by_file: std::collections::HashMap<String, Vec<&octodev::store::DocumentBlock>> = std::collections::HashMap::new();

	for block in blocks {
		blocks_by_file
			.entry(block.path.clone())
			.or_default()
			.push(block);
	}

	// Print results organized by file
	for (file_path, file_blocks) in blocks_by_file.iter() {
		println!("╔══════════════════ File: {} ══════════════════", file_path);

		for (idx, block) in file_blocks.iter().enumerate() {
			println!("║");
			println!("║ Section {} of {}: {}", idx + 1, file_blocks.len(), block.title);
			println!("║ Level: {}  Lines: {}-{}", block.level, block.start_line, block.end_line);

			// Show relevance score if available
			if let Some(distance) = block.distance {
				println!("║ Relevance: {:.4}", distance);
			}

			println!("║");
			println!("║ Content:");

			// Display content with proper indentation
			let lines: Vec<&str> = block.content.lines().collect();
			if lines.len() > 10 {
				// Show first 7 lines
				for line in lines.iter().take(7) {
					println!("║   {}", line);
				}
				println!("║   ... ({} more lines omitted)", lines.len() - 10);
				// Show last 3 lines
				for line in lines.iter().skip(lines.len() - 3) {
					println!("║   {}", line);
				}
			} else {
				// If not too long, show all lines
				for line in lines {
					println!("║   {}", line);
				}
			}
		}

		println!("╚════════════════════════════════════════\n");
	}
}
