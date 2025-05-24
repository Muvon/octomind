// EditorConfig formatter module

use anyhow::{Result, anyhow};
use colored::*;
use std::path::{Path, PathBuf};
use std::fs::{self, File};
use std::io::{Read};
use ignore::WalkBuilder;

/// Apply formatting based on .editorconfig to modified files
pub async fn apply_editorconfig_formatting(modified_files: Option<Vec<PathBuf>>) -> Result<()> {
	println!("{}", "Applying EditorConfig formatting...".cyan());

	// Check if .editorconfig exists in the current directory
	let current_dir = std::env::current_dir()?;
	let editorconfig_path = current_dir.join(".editorconfig");

	if !editorconfig_path.exists() {
		println!("{}", "No .editorconfig file found, skipping formatting.".yellow());
		return Ok(());
	}

	// Get config to check debug flag
	let config = crate::config::Config::load().unwrap_or_default();

	if config.openrouter.log_level.is_debug_enabled() {
		println!("{}", "Found .editorconfig file".green());
	}

	// Determine which files to process
	let files_to_format = if let Some(files) = modified_files {
		if config.openrouter.log_level.is_debug_enabled() {
			println!("{} {} files", "Processing".blue(), files.len());
		}
		files
	} else {
		// Try to get modified files from git
		match get_modified_files_from_git() {
			Ok(git_files) if !git_files.is_empty() => {
				if config.openrouter.log_level.is_debug_enabled() {
					println!("{} {} files from git", "Processing".blue(), git_files.len());
				}
				git_files
			}
			_ => {
				// Fallback: Process all files respecting .gitignore
				if config.openrouter.log_level.is_debug_enabled() {
					println!("{}", "No git changes detected or git not available, processing all files...".yellow());
				}
				let all_files = collect_files_respecting_gitignore(&current_dir)?;
				if config.openrouter.log_level.is_debug_enabled() {
					println!("{} {} files from project", "Processing".blue(), all_files.len());
				}
				all_files
			}
		}
	};

	// Skip if no files to format
	if files_to_format.is_empty() {
		println!("{}", "No files to format.".yellow());
		return Ok(());
	}

	// Apply formatting to each file
	let mut formatted_count = 0;
	for file_path in files_to_format {
		if format_file(&file_path).await? {
			formatted_count += 1;
		}
	}

	println!("{} {} files", "Formatted".bright_green(), formatted_count);
	Ok(())
}

/// Format a single file according to EditorConfig rules
async fn format_file(file_path: &Path) -> Result<bool> {
	use std::io::ErrorKind;

	// Skip directories and non-existent files
	if !file_path.exists() || file_path.is_dir() {
		return Ok(false);
	}

	// Skip binary files by checking file extension or content
	if is_likely_binary_file(file_path) {
		return Ok(false);
	}

	// Get config to check debug flag
	let config = crate::config::Config::load().unwrap_or_default();

	// Get EditorConfig properties for this file
	let properties = match editorconfig::get_config(file_path) {
		Ok(props) => props,
		Err(e) => {
			if config.openrouter.log_level.is_debug_enabled() {
				println!("{} {}: {}", "Error getting EditorConfig properties for".yellow(),
					file_path.display(), e);
			}
			return Ok(false);
		}
	};

	// Skip if no properties apply to this file
	if properties.is_empty() {
		return Ok(false);
	}

	// Read file content
	let mut file = match File::open(file_path) {
		Ok(f) => f,
		Err(e) => {
			if config.openrouter.log_level.is_debug_enabled() {
				println!("{} {}: {}", "Error reading file".yellow(), file_path.display(), e);
			}
			return Ok(false);
		}
	};

	let mut content = String::new();
	if let Err(e) = file.read_to_string(&mut content) {
		// Skip files that can't be read as UTF-8
		if e.kind() == ErrorKind::InvalidData {
			return Ok(false);
		}
		if config.openrouter.log_level.is_debug_enabled() {
			println!("{} {}: {}", "Error reading file content".yellow(), file_path.display(), e);
		}
		return Ok(false);
	}

	// Apply EditorConfig rules
	let mut modified = false;

	// Handle indentation style
	if let Some(indent_style) = properties.get("indent_style") {
		let indent_size = properties.get("indent_size")
			.and_then(|s| s.parse::<usize>().ok())
			.unwrap_or(4);

		let indent_char = match indent_style.as_str() {
			"tab" => '\t',
			"space" => ' ',
			_ => ' ', // Default to space
		};

		// Replace indentation
		let mut formatted_content = String::new();
		for line in content.lines() {
			let trimmed = line.trim_start();
			let indent_level = (line.len() - trimmed.len()) / if indent_char == '\t' { 1 } else { indent_size };

			let indent = if indent_char == '\t' {
				"\t".repeat(indent_level)
			} else {
				" ".repeat(indent_level * indent_size)
			};

			formatted_content.push_str(&format!("{}{}{}", indent, trimmed, "\n"));
		}

		// Remove the last newline if the original content didn't end with one
		if !content.ends_with('\n') {
			formatted_content.pop();
		}

		if content != formatted_content {
			content = formatted_content;
			modified = true;
		}
	}

	// Handle end of line
	if let Some(end_of_line) = properties.get("end_of_line") {
		let eol = match end_of_line.as_str() {
			"lf" => "\n",
			"crlf" => "\r\n",
			"cr" => "\r",
			_ => "\n", // Default to LF
		};

		// Normalize line endings
		let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
		let formatted_content = normalized.replace("\n", eol);

		if content != formatted_content {
			content = formatted_content;
			modified = true;
		}
	}

	// Handle trim trailing whitespace
	if let Some(trim_trailing) = properties.get("trim_trailing_whitespace") {
		if trim_trailing == "true" {
			let formatted_content = content.lines()
				.map(|line| line.trim_end())
				.collect::<Vec<_>>()
				.join("\n");

			if content != formatted_content {
				content = formatted_content;
				modified = true;
			}
		}
	}

	// Handle insert final newline
	if let Some(insert_final_newline) = properties.get("insert_final_newline") {
		let should_end_with_newline = insert_final_newline == "true";

		if should_end_with_newline && !content.ends_with('\n') {
			content.push('\n');
			modified = true;
		} else if !should_end_with_newline && content.ends_with('\n') {
			content.pop();
			modified = true;
		}
	}

	// Write back to file if modified
	if modified {
		if let Err(e) = fs::write(file_path, &content) {
			// Always show write errors as they're important
			println!("{} {}: {}", "Error writing formatted content to".bright_red(),
				file_path.display(), e);
			return Ok(false);
		}

		println!("{}: {}", "Formatted".bright_green(), file_path.display());
		return Ok(true);
	}

	Ok(false) // Not modified
}

/// Get list of modified files from git
fn get_modified_files_from_git() -> Result<Vec<PathBuf>> {
	use std::process::Command;
	use std::collections::HashSet;

	let current_dir = std::env::current_dir()?;
	let mut modified_files = HashSet::new();

	// Get modified but unstaged files
	let unstaged_output = Command::new("git")
		.args(["ls-files", "--modified", "--others", "--exclude-standard"])
		.output();

	if let Ok(ref output) = unstaged_output {
		if output.status.success() {
			let files_str = String::from_utf8_lossy(&output.stdout);
			for line in files_str.lines().filter(|line| !line.is_empty()) {
				modified_files.insert(current_dir.join(line));
			}
		}
	}

	// Get staged files
	let staged_output = Command::new("git")
		.args(["diff", "--cached", "--name-only"])
		.output();

	if let Ok(ref output) = staged_output {
		if output.status.success() {
			let files_str = String::from_utf8_lossy(&output.stdout);
			for line in files_str.lines().filter(|line| !line.is_empty()) {
				modified_files.insert(current_dir.join(line));
			}
		}
	}

	// If neither command worked, return an error
	if modified_files.is_empty() && (unstaged_output.is_err() || staged_output.is_err()) {
		return Err(anyhow!("Failed to execute git commands"));
	}

	Ok(modified_files.into_iter().collect())
}

/// Collect all files in the project respecting .gitignore
fn collect_files_respecting_gitignore(dir: &Path) -> Result<Vec<PathBuf>> {
	let mut files = Vec::new();

	// Use the ignore crate to respect .gitignore rules
	let walker = WalkBuilder::new(dir)
		.hidden(false) // Process hidden files
		.git_ignore(true) // Respect .gitignore
		.build();

	for result in walker {
		match result {
			Ok(entry) => {
				let path = entry.path();
				if path.is_file() {
					files.push(path.to_path_buf());
				}
			},
			Err(e) => {
				// Get config to check debug flag
				let config = crate::config::Config::load().unwrap_or_default();

				if config.openrouter.log_level.is_debug_enabled() {
					println!("{}: {}", "Warning: Error walking directory".yellow(), e);
				}
			}
		}
	}

	Ok(files)
}

/// Check if a file is likely to be binary
fn is_likely_binary_file(file_path: &Path) -> bool {
	// Check file extension first, as it's faster
	if let Some(ext) = file_path.extension().and_then(|e| e.to_str()) {
		// Common binary file extensions
		let binary_extensions = [
			"exe", "dll", "so", "dylib", "bin", "obj", "o",
			"a", "lib", "pdf", "doc", "docx", "xls", "xlsx",
			"ppt", "pptx", "zip", "gz", "tar", "rar", "7z",
			"jpg", "jpeg", "png", "gif", "bmp", "ico", "webp",
			"mp3", "mp4", "wav", "ogg", "avi", "mov", "mkv",
			"ttf", "woff", "woff2", "eot", "otf"
		];

		if binary_extensions.contains(&ext.to_lowercase().as_str()) {
			return true;
		}
	}

	// If extension check doesn't identify it as binary, check the content
	// Try to open and read a small chunk to detect binary content
	if let Ok(mut file) = File::open(file_path) {
		let mut buffer = [0; 512]; // Read up to 512 bytes
		if let Ok(bytes_read) = file.read(&mut buffer) {
			if bytes_read > 0 {
				// Count null bytes - binary files often have many null bytes
				let null_count = buffer[..bytes_read].iter().filter(|&&b| b == 0).count();

				// If more than 10% of the first bytes are null, assume it's binary
				if null_count > bytes_read / 10 {
					return true;
				}

				// Also check for non-printable characters
				let non_printable = buffer[..bytes_read].iter()
					.filter(|&&b| !(32..=126).contains(&b) && b != 9 && b != 10 && b != 13)
					.count();

				// If more than 20% are non-printable characters, assume binary
				if non_printable > bytes_read / 5 {
					return true;
				}
			}
		}
	}

	false
}
