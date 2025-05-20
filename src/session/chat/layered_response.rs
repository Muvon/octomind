// Layered response processing implementation

use crate::config::Config;
use crate::session::openrouter;
use crate::session::chat::session::ChatSession;
use crate::session::layers::LayeredProcessor;
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
    println!("{}", "Using layered processing architecture...".cyan());
    
    // Create the layered processor
    let layered_processor = LayeredProcessor::from_config(config);
    
    // Add user message
    chat_session.add_user_message(input)?;
    
    // Create a task to show loading animation
    let animation_cancel = operation_cancelled.clone();
    let animation_task = tokio::spawn(async move {
        let _ = show_loading_animation(animation_cancel).await;
    });
    
    // Process through the layers
    let final_output = match layered_processor.process(
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
    
    // Create a dummy exchange for token tracking
    // In a production system, we'd track tokens for each layer
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
    
    // Add the final output to the chat session
    chat_session.add_assistant_message(&final_output, Some(dummy_exchange), config)?;
    
    // Print assistant response with color
    println!("\n{}", final_output.bright_green());
    
    // Display cumulative token usage
    println!();
    println!("{}", "── session usage ────────────────────────────────────────".bright_cyan());
    
    // Format token usage with cached tokens
    let cached = chat_session.session.info.cached_tokens;
    let prompt = chat_session.session.info.input_tokens;
    let completion = chat_session.session.info.output_tokens;
    let total = prompt + completion + cached;
    
    println!("{} {} prompt ({} cached), {} completion, {} total, ${:.5}",
        "tokens:".bright_blue(),
        prompt,
        cached,
        completion,
        total,
        chat_session.session.info.total_cost);
    
    // If we have cached tokens, show the savings percentage
    if cached > 0 {
        let saving_pct = (cached as f64 / (prompt + cached) as f64) * 100.0;
        println!("{} {:.1}% of prompt tokens ({} tokens saved)",
            "cached:".bright_green(),
            saving_pct,
            cached);
    }
    
    println!();
    
    Ok(())
}