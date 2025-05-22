use std::io::Write;
use std::sync::Arc;
use parking_lot::RwLock;
use clap::Args;

use octodev::config::Config;
use octodev::store::Store;
use octodev::state;
use octodev::indexer;

#[derive(Args, Debug)]
pub struct IndexArgs {
	/// Force reindex files that have been previously indexed
	#[arg(long)]
	pub reindex: bool,
}

pub async fn execute(store: &Store, config: &Config, args: &IndexArgs) -> Result<(), anyhow::Error> {
	let current_dir = std::env::current_dir()?;
	println!("Indexing current directory: {}", current_dir.display());

	let state = state::create_shared_state();
	state.write().current_directory = current_dir;

	// Set reindex flag in state if requested
	if args.reindex {
		println!("Reindex flag set - forcing reindex of all files");
		state.write().force_reindex = true;
	}

	// Spawn the progress display task
	let progress_handle = tokio::spawn(display_indexing_progress(state.clone()));

	// Start indexing
	indexer::index_files(store, state.clone(), config).await?;

	// Explicitly flush to ensure all data is persisted
	println!("Flushing index to disk...");
	store.flush().await?;

	// Wait for the progress display to finish
	let _ = progress_handle.await;

	println!("✓ Indexing complete!");
	Ok(())
}

pub async fn display_indexing_progress(state: Arc<RwLock<state::IndexState>>) {
	let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
	let mut spinner_idx = 0;
	let mut last_indexed = 0;
	let mut last_graphrag_blocks = 0;
	let mut indexing_complete = false;

	while !indexing_complete {
		// Gather all necessary state in local variables before the await
		let current_indexed;
		let graphrag_blocks;
		let status_message;
		let graphrag_enabled;

		{
			let current_state = state.read();
			current_indexed = current_state.indexed_files;
			graphrag_blocks = current_state.graphrag_blocks;
			status_message = current_state.status_message.clone();
			graphrag_enabled = current_state.graphrag_enabled;
			indexing_complete = current_state.indexing_complete; // Update our loop control variable
			// Lock is dropped here when we exit the scope
		}

		// Exit early if indexing is complete
		if indexing_complete {
			break;
		}

		// Only redraw if something changed or on spinner change
		if current_indexed != last_indexed ||
		   graphrag_blocks != last_graphrag_blocks {
			// Clear the line and move cursor to beginning with \r
			print!("\r");

			// Build display string
			if !graphrag_enabled {
				print!("{} Indexing: {} files",
					spinner_chars[spinner_idx],
					current_indexed
				);
			} else {
				print!("{} Indexing: {} files{}",
					spinner_chars[spinner_idx],
					current_indexed,
					if graphrag_blocks > 0 { format!(", GraphRAG: {} blocks", graphrag_blocks) } else { String::new() },
				);
			}

			// Add status message if there is one
			if !status_message.is_empty() {
				print!(": {}", status_message);
			}

			std::io::stdout().flush().unwrap();
			last_indexed = current_indexed;
			last_graphrag_blocks = graphrag_blocks;
		} else {
			// Just update the spinner
			print!("\r");
			if !graphrag_enabled {
				print!("{} Indexing: {} files",
					spinner_chars[spinner_idx],
					current_indexed
				);
			} else {
				print!("{} Indexing: {} files{}",
					spinner_chars[spinner_idx],
					current_indexed,
					if graphrag_blocks > 0 { format!(", GraphRAG: {} blocks", graphrag_blocks) } else { String::new() },
				);
			}

			// Add status message if there is one
			if !status_message.is_empty() {
				print!(": {}", status_message);
			}
			std::io::stdout().flush().unwrap();
		}

		spinner_idx = (spinner_idx + 1) % spinner_chars.len();
		tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
	}

	// Final summary message
	let final_indexed;
	let final_graphrag_enabled;
	let final_graphrag_blocks;

	{
		let final_state = state.read();
		final_indexed = final_state.indexed_files;
		final_graphrag_enabled = final_state.graphrag_enabled;
		final_graphrag_blocks = final_state.graphrag_blocks;
	}

	if !final_graphrag_enabled {
		print!("\r");
		println!("Indexing complete! Total files indexed: {}", final_indexed);
	} else {
		print!("\r");
		println!("Indexing complete! Files: {}, GraphRAG: {} blocks",
			final_indexed, final_graphrag_blocks);
	}
}
