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

    // Create a task to show loading animation
    let animation_cancel = operation_cancelled.clone();
    let animation_task = tokio::spawn(async move {
        let _ = show_loading_animation(animation_cancel).await;
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

    // Check for tool calls in the developer layer output
    if config.mcp.enabled && crate::session::mcp::parse_tool_calls(&layer_output).len() > 0 {
        // Create a new cancellation flag to avoid any "Operation cancelled" messages when not requested
        let fresh_tool_cancellation = Arc::new(AtomicBool::new(false));

        // Process the response with tool handling using the existing process_response function
        // Create a dummy exchange for initial processing
        let dummy_exchange = openrouter::OpenRouterExchange {
            request: serde_json::json!({}),
            response: serde_json::json!({}),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            usage: Some(openrouter::TokenUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                cost: None,
                completion_tokens_details: None,
                prompt_tokens_details: None,
                breakdown: None,
            }),
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
    let dummy_exchange = openrouter::OpenRouterExchange {
        request: serde_json::json!({}),
        response: serde_json::json!({}),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        usage: Some(openrouter::TokenUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cost: None,
            completion_tokens_details: None,
            prompt_tokens_details: None,
            breakdown: None,
        }),
    };

    // Add the output to the message history without further processing
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
