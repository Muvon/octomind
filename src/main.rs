
mod indexer;
mod prompt;
mod state;

use std::io::Write;
use std::path::PathBuf;
use prompt::Prompt;
use state::create_shared_state;
use clap::Parser;

#[derive(Parser)]
#[command(name = "octodev")]
#[command(about = "OctoDev is smart developer assistant based on your codebase")]
struct Args {
	#[arg(default_value = ".")]
	directory: PathBuf,
}

#[tokio::main]
async fn main() {
	let args = Args::parse();

	let state = create_shared_state();
	let state_clone = state.clone();

	state.write().current_directory = args.directory.clone();

	// Start indexing in background with the specified directory
	let _ = indexer::index_files(state_clone).await;

	let mut prompt = Prompt::new();
	println!("File Search CLI (Press ESC to exit)");
	println!("Indexing directory: {}", args.directory.display());

	loop {
		print!("> ");
		std::io::stdout().flush().unwrap();

		if let Some(input) = prompt.read_line() {
			let state_guard = state.read();

			if !state_guard.indexing_complete {
				println!("Still indexing files, please wait...");
				continue;
			}

			// For now, just echo the input and show number of indexed files
			println!("You entered: {}", input);
			println!("Number of indexed files: {}", state_guard.files.len());
		} else {
			break;
		}
	}
}

