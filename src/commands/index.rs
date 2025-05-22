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

	// Wait for the progress display to finish
	let _ = progress_handle.await;

	println!("✓ Indexing complete!");
	Ok(())
}

pub async fn display_indexing_progress(state: Arc<RwLock<state::IndexState>>) {
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