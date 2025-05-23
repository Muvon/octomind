// Layered response processing implementation

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use colored::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;
use super::animation::show_loading_animation;

// Process a response using the layered architecture
// Returns the final processed text that should be used as input for the main model
pub async fn process_layered_response(
	input: &str,
	chat_session: &mut ChatSession,
	config: &Config,
	operation_cancelled: Arc<AtomicBool>
) -> Result<String> {
	// Debug output
	// println!("{}", "Using layered processing architecture...".cyan());

	// Add user message to the session at the beginning
	chat_session.add_user_message(input)?;

	// Ensure system message is cached before processing with layers
	// This is important because system messages contain all the function definitions
	// and developer context needed for the layered processing
	let mut system_message_cached = false;

	// Check if system message is already cached
	for msg in &chat_session.session.messages {
		if msg.role == "system" && msg.cached {
			system_message_cached = true;
			break;
		}
	}

	// If system message not already cached, add a cache checkpoint
	if !system_message_cached {
		if let Ok(cached) = chat_session.session.add_cache_checkpoint(true) {
			if cached && crate::session::model_supports_caching(&chat_session.model) {
				println!("{}", "System message has been automatically marked for caching to save tokens.".yellow());
				// Save the session to ensure the cached status is persisted
				let _ = chat_session.save();
			}
		}
	}

	// Create a task to show loading animation with current cost
	let animation_cancel = operation_cancelled.clone();
	let current_cost = chat_session.session.info.total_cost;
	let animation_task = tokio::spawn(async move {
		let _ = show_loading_animation(animation_cancel, current_cost).await;
	});

	// Process through the layers using the modular layered architecture
	// Each layer operates on its own session context and passes only the necessary output
	// to the next layer, ensuring proper isolation
	//
	// IMPORTANT: Each layer handles its own function calls internally with its own model
	// using the process method in processor.rs
	let layer_output: String = match crate::session::layers::process_with_layers(
		input,
		&mut chat_session.session,
		config,
		operation_cancelled.clone()
	).await {
		Ok(output) => output,
		Err(e) => {
			// Stop the animation
			operation_cancelled.store(true, Ordering::SeqCst);
			let _ = animation_task.await;
			return Err(e);
		}
	};

	// Stop the animation
	operation_cancelled.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

	// Display status message for layered sessions - minimal for non-debug
	if config.openrouter.debug {
		println!("{}", "Using layered processing with model-specific caching - only supported models will use caching".bright_cyan());
	} else {
		println!("{}", "Using layered processing".bright_cyan());
	}

	// Return the processed output from layers for use in the main model conversation
	// This output already includes the results of any function calls handled by each layer
	Ok(layer_output)
}
