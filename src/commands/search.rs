use clap::Args;

use octodev::config::Config;
use octodev::store::Store;
use octodev::indexer;

use super::index::IndexArgs;

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
}

pub async fn execute(store: &Store, args: &SearchArgs, config: &Config) -> Result<(), anyhow::Error> {
	let current_dir = std::env::current_dir()?;
	let octodev_dir = current_dir.join(".octodev");
	let index_path = octodev_dir.join("storage");

	// Check if we have an index already; if not, create one
	if !index_path.exists() {
		println!("No index found. Indexing current directory first");
		super::index::execute(store, config, &IndexArgs { reindex: false }).await?
	}

	println!("Searching for: {}", args.query);
	println!("Using embedding provider: {:?}", config.embedding_provider);

	// Generate embeddings for the query
	let embeddings = indexer::generate_embeddings(&args.query, true, config).await?;

	// Search for matching code blocks
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
		// Use markdown format
		let markdown = indexer::code_blocks_to_markdown(&results);
		println!("{}", markdown);
	} else {
		indexer::render_code_blocks(&results);
	}

	Ok(())
}