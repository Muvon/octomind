use clap::Args;

use octodev::config::Config;
use octodev::store::Store;
use octodev::indexer;

#[derive(Args, Debug)]
pub struct ViewArgs {
	/// Files to view (may include glob patterns)
	pub files: Vec<String>,

	/// Output in JSON format
	#[arg(long)]
	pub json: bool,

	/// Output in Markdown format
	#[arg(long)]
	pub md: bool,
}

pub async fn execute(_store: &Store, args: &ViewArgs, _config: &Config) -> Result<(), anyhow::Error> {
	// Get current directory
	let current_dir = std::env::current_dir()?;

	// Note: View command doesn't require an index as it parses files directly
	let octodev_dir = current_dir.join(".octodev");
	let index_path = octodev_dir.join("storage");
	if !index_path.exists() {
		println!("Note: No index found. The view command works without an index, but you can run 'octodev index' to create one if needed for other commands.");
	}

	// Get files matching patterns
	let mut matching_files = Vec::new();

	for pattern in &args.files {
		// Use glob pattern matching
		let glob_pattern = match globset::Glob::new(pattern) {
			Ok(g) => g.compile_matcher(),
			Err(e) => {
				println!("Invalid glob pattern '{}': {}", pattern, e);
				continue;
			}
		};

		// Use ignore crate to respect .gitignore files while finding files
		let walker = ignore::WalkBuilder::new(&current_dir)
			.hidden(false)  // Don't ignore hidden files (unless in .gitignore)
			.git_ignore(true)  // Respect .gitignore files
			.git_global(true) // Respect global git ignore files
			.git_exclude(true) // Respect .git/info/exclude files
			.build();

		for result in walker {
			let entry = match result {
				Ok(entry) => entry,
				Err(_) => continue,
			};

			// Skip directories, only process files
			if !entry.file_type().map_or(false, |ft| ft.is_file()) {
				continue;
			}

			// See if this file matches our pattern
			let relative_path = entry.path().strip_prefix(&current_dir).unwrap_or(entry.path());
			if glob_pattern.is_match(relative_path) {
				matching_files.push(entry.path().to_path_buf());
			}
		}
	}

	if matching_files.is_empty() {
		println!("No matching files found.");
		return Ok(());
	}

	// Extract signatures from matching files
	let signatures = indexer::extract_file_signatures(&matching_files)?;

	// Display results in the requested format
	if args.json {
		indexer::render_signatures_json(&signatures)?
	} else if args.md {
		// Use markdown format
		let markdown = indexer::signatures_to_markdown(&signatures);
		println!("{}", markdown);
	} else {
		indexer::render_signatures_text(&signatures);
	}

	Ok(())
}
