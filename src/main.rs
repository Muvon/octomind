mod content;
mod indexer;
mod store;
mod prompt;
mod state;

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;
use state::create_shared_state;
use clap::{Parser, Subcommand, Args};

use crate::store::Store;

#[derive(Parser)]
#[command(name = "octodev")]
#[command(version = "0.1.0")]
#[command(about = "OctoDev is smart developer assistant based on your codebase")]
struct OctodevArgs {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Index the current directory's codebase
    Index(IndexArgs),
    
    /// Search the codebase with a natural language query
    Search(SearchArgs),
    
    /// Watch for changes in the codebase and reindex automatically
    Watch(IndexArgs),
}

#[derive(Args)]
struct IndexArgs {
    /// Path to the directory to index
    #[arg(default_value = ".")]
    directory: PathBuf,
}

#[derive(Args)]
struct SearchArgs {
    /// Search query
    query: String,
    
    /// Expand all symbols in matching code blocks
    #[arg(long, short)]
    expand: bool,
    
    /// Output in JSON format
    #[arg(long)]
    json: bool,
    
    /// Path to the directory to search
    #[arg(default_value = ".")]
    directory: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args = OctodevArgs::parse();
    
    // Initialize the store
    let store = Store::new()?;
    store.initialize_collections().await?;

    // Execute the appropriate command
    match &args.command {
        Commands::Index(index_args) => {
            index_codebase(&store, &index_args.directory).await?
        },
        Commands::Search(search_args) => {
            search_codebase(&store, search_args).await?
        },
        Commands::Watch(watch_args) => {
            watch_codebase(&store, &watch_args.directory).await?
        },
    }
    
    Ok(())
}

async fn index_codebase(store: &Store, directory: &PathBuf) -> Result<(), anyhow::Error> {
	println!("Indexing directory: {}", directory.display());
	
	let state = create_shared_state();
	state.write().current_directory = directory.clone();
	
	// Spawn the progress display task
	let progress_handle = tokio::spawn(display_indexing_progress(state.clone()));
	
	// Start indexing
	indexer::index_files(store, state.clone()).await?;
	
	// Wait for the progress display to finish
	let _ = progress_handle.await;
	
	println!("✓ Indexing complete!");
	Ok(())
}

async fn search_codebase(store: &Store, args: &SearchArgs) -> Result<(), anyhow::Error> {
    let current_dir = std::env::current_dir()?;
    let index_path = current_dir.join(".octodev/qdrant");
    
    // Check if we have an index already; if not, create one
    if !index_path.exists() {
        println!("No index found. Indexing directory first: {}", args.directory.display());
        index_codebase(store, &args.directory).await?
    }
    
    println!("Searching for: {}", args.query);
    
    // Generate embeddings for the query
    let embeddings = content::generate_embeddings(&args.query, "jina-embeddings-v2-base-code").await?;
    
    // Search for matching code blocks
    let mut results = store.get_code_blocks(embeddings).await?;
    
    // If expand flag is set, expand symbols in the results
    if args.expand {
        println!("Expanding symbols...");
        results = expand_symbols(store, results).await?;
    }
    
    // Output the results
    if args.json {
        render_results_json(&results)?
    } else {
        content::render_code_blocks(&results);
    }
    
    Ok(())
}

async fn watch_codebase(store: &Store, directory: &PathBuf) -> Result<(), anyhow::Error> {
    println!("Starting watch mode for directory: {}", directory.display());
    println!("Initial indexing...");
    
    // Do initial indexing
    index_codebase(store, directory).await?;
    
    println!("Watching for changes (press Ctrl+C to stop)...");
    
    // Setup the file watcher with debouncer
    use notify_debouncer_mini::{new_debouncer, DebouncedEvent};
    use std::sync::mpsc::channel;
    use std::time::Duration;
    
    let (tx, rx) = channel();
    
    // Create a debounced watcher to call our tx sender when files change
    let mut debouncer = new_debouncer(
        Duration::from_secs(2),
        move |res: Result<Vec<DebouncedEvent>, notify::Error>| {
            match res {
                Ok(events) => {
                    if !events.is_empty() {
                        let _ = tx.send(());
                    }
                }
                Err(e) => eprintln!("Error in file watcher: {:?}", e),
            }
        },
    )?;
    
    // Add the target directory to the watcher
    debouncer.watcher().watch(directory, notify::RecursiveMode::Recursive)?;
    
    // Create shared state for reindexing
    let state = create_shared_state();
    state.write().current_directory = directory.clone();
    
    loop {
        // Wait for changes
        match rx.recv() {
            Ok(()) => {
                println!("\nDetected file changes, reindexing...");
                
                // Reset the indexing state
                let mut state_guard = state.write();
                state_guard.indexed_files = 0;
                state_guard.indexing_complete = false;
                drop(state_guard);
                
                // Reindex the codebase
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await; // Give a bit of time for all file changes to complete
                index_codebase(store, directory).await?;
            }
            Err(e) => {
                eprintln!("Watch error: {:?}", e);
                break;
            }
        }
    }
    
    Ok(())
}

async fn expand_symbols(store: &Store, code_blocks: Vec<crate::store::CodeBlock>) -> Result<Vec<crate::store::CodeBlock>, anyhow::Error> {
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
    let mut visited_files = std::collections::HashSet::new();
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

async fn display_indexing_progress(state: Arc<RwLock<state::IndexState>>) {
	let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
	let mut spinner_idx = 0;
	let mut last_indexed = 0;

	while !state.read().indexing_complete {
		let current_indexed = state.read().indexed_files;
		if current_indexed != last_indexed {
			print!("\r{} Indexing: {} files",
				spinner_chars[spinner_idx],
				current_indexed
			);
			std::io::stdout().flush().unwrap();
			last_indexed = current_indexed;
		} else {
			print!("\r{} Indexing: {} files",
				spinner_chars[spinner_idx],
				current_indexed
			);
			std::io::stdout().flush().unwrap();
		}

		spinner_idx = (spinner_idx + 1) % spinner_chars.len();
		tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
	}

	println!("\rIndexing complete! Total files indexed: {}", state.read().indexed_files);
}

fn render_results_json(results: &[crate::store::CodeBlock]) -> Result<(), anyhow::Error> {
	let json = serde_json::to_string_pretty(results)?;
	println!("{}", json);
	Ok(())
}

