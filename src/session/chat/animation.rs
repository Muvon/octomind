// Animation module for loading indicators

use std::io::{Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crossterm::{cursor, execute};
use anyhow::Result;
use colored::*;

// Animation frames for loading indicator
const LOADING_FRAMES: [&str; 8] = [
	"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧",
];

// Show loading animation while waiting for response
pub async fn show_loading_animation(cancel_flag: Arc<AtomicBool>, cost: f64) -> Result<()> {
	let mut stdout = stdout();
	let mut frame_idx = 0;

	// Save cursor position
	execute!(stdout, cursor::SavePosition)?;

	while !cancel_flag.load(Ordering::SeqCst) {
		// Display frame with color if supported
		execute!(stdout, cursor::RestorePosition)?;

		print!(" {} {} ${:.5}",
			LOADING_FRAMES[frame_idx].cyan(),
			"Generating response...".bright_blue(),
			cost);

		stdout.flush()?;

		// Update frame index
		frame_idx = (frame_idx + 1) % LOADING_FRAMES.len();

		// Shorter delay to be more responsive to cancellation
		tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
	}

	// Clear loading message completely and print a newline
	execute!(stdout, cursor::RestorePosition)?;
	print!("                                        "); // Clear the entire loading message with spaces
	execute!(stdout, cursor::RestorePosition)?;
	stdout.flush()?;

	Ok(())
}
