// Layered response processing implementation

use crate::config::Config;
use crate::session::openrouter;
use crate::session::chat::session::ChatSession;
use colored::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;
use super::animation::show_loading_animation;

// Process a response using the layered architecture
pub async fn process_layered_response(
    input: &str,
    chat_session: &mut ChatSession,
    config: &Config,
    operation_cancelled: Arc<AtomicBool>
) -> Result<()> {
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

    // Process through the layers using the new modular layered architecture
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
    
    // Display status message for layered sessions
    println!("{}", "Using layered processing with model-specific caching - only supported models will use caching".bright_cyan());

    // Check for tool calls in the developer layer output
    if config.mcp.enabled && crate::session::mcp::parse_tool_calls(&layer_output).len() > 0 {
        // Create a new cancellation flag to avoid any "Operation cancelled" messages when not requested
        let fresh_tool_cancellation = Arc::new(AtomicBool::new(false));

        // Process the response with tool handling using the existing process_response function
        // Create a dummy exchange that reflects usage tracking was enabled
        let dummy_exchange = openrouter::OpenRouterExchange {
            request: serde_json::json!({
                "usage": {
                    "include": true
                }
            }),
            response: serde_json::json!({}),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            usage: None,  // Don't include usage info to avoid double-counting costs
        };

        // Process the response with tool calls using the existing handler with fresh cancellation flag
        return super::response::process_response(
            layer_output,
            dummy_exchange,
            chat_session,
            config,
            fresh_tool_cancellation
        ).await;
    }

    // If no tool calls, add the output to session for message history
    // but don't print it or process it again since it's already been processed
    // Note: Don't create a dummy exchange with zero costs - this interferes with the
    // cost tracking. Instead, create a realistic exchange that properly reflects
    // the costs that were already tracked in the layer statistics.
    let dummy_exchange = openrouter::OpenRouterExchange {
        request: serde_json::json!({
            "usage": {
                "include": true
            }
        }),
        response: serde_json::json!({}),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        usage: None,  // Don't include any usage info to avoid double-counting
    };

    // Add the output to the message history without further cost accounting
    // since costs have already been tracked in the session layer_stats
    chat_session.add_assistant_message(&layer_output, Some(dummy_exchange), config)?;

    // Print assistant response with color
    println!("\n{}", layer_output.bright_green());

    // Just show a short summary with the total cost
    // Detailed breakdowns are available via the /info command
    println!();
    println!("{} ${:.5}", "Session total cost:".bright_cyan(),
             chat_session.session.info.total_cost);
    println!("{}", "Use /info to see detailed token and cost breakdowns by layer".bright_blue());
    println!();

    Ok(())
}
