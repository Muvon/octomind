// Context reduction for session optimization

use crate::config::Config;
use crate::session::openrouter;
use crate::session::chat::session::ChatSession;
use colored::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;
use super::animation::show_loading_animation;

// Process context reduction (extracted from reducer layer)
pub async fn perform_context_reduction(
    chat_session: &mut ChatSession,
    config: &Config,
    operation_cancelled: Arc<AtomicBool>
) -> Result<()> {
    println!("{}", "Performing context reduction and optimization...".cyan());
    
    // Create a task to show loading animation
    let animation_cancel = operation_cancelled.clone();
    let animation_task = tokio::spawn(async move {
        let _ = show_loading_animation(animation_cancel).await;
    });

    // Extract elements from the session to create an optimized version
    // First get the original user request and the current context
    let user_message = chat_session.session.messages.iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_else(|| "No original query found".to_string());
    
    // Get the last assistant message as the context
    let assistant_message = chat_session.session.messages.iter()
        .filter(|m| m.role == "assistant")
        .last()
        .map(|m| m.content.clone())
        .unwrap_or_else(|| "No assistant response found".to_string());
    
    // Create a message for the reducer to summarize everything
    let reducer_input = format!("Original request: {}\n\nDeveloper solution: {}", 
                          user_message, assistant_message);
    
    // Create messages for the OpenRouter API
    let mut messages = Vec::new();
    
    // System message with reducer-specific prompt
    messages.push(crate::session::Message {
        role: "system".to_string(),
        content: crate::session::get_layer_system_prompt(crate::session::LayerType::Reducer),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        cached: false,
    });
    
    // Add user message with the context to reduce
    messages.push(crate::session::Message {
        role: "user".to_string(),
        content: reducer_input,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        cached: false,
    });
    
    // Convert to OpenRouter format
    let or_messages = openrouter::convert_messages(&messages);
    
    // Choose what model to use for reduction
    // Try to use the reducer model if configured, otherwise use GPT-4o as fallback
    let reducer_model = match &config.openrouter.reducer_model {
        Some(model) => model.clone(),
        None => "openai/gpt-4o".to_string(),
    };
    
    // Call the model
    let reduction_result = openrouter::chat_completion(
        or_messages,
        &reducer_model,
        0.7, // Moderate temperature
        config
    ).await;
    
    // Stop the animation
    operation_cancelled.store(true, Ordering::SeqCst);
    let _ = animation_task.await;
    
    match reduction_result {
        Ok((reduced_content, exchange)) => {
            println!("{}", "Context reduction complete".bright_green());
            println!("{}", reduced_content.bright_blue());
            
            // Clear the session while preserving key elements
            let system_message = chat_session.session.messages.iter()
                .find(|m| m.role == "system")
                .cloned();
            
            // Store user message if it exists to prevent warning
            let user_message = chat_session.session.messages.iter()
                .find(|m| m.role == "user")
                .cloned();
            
            chat_session.session.messages.clear();
            
            // Restore system message
            if let Some(system) = system_message {
                chat_session.session.messages.push(system);
            }
            
            // Add back the user message to prevent 'warning: user message not found' issues
            if let Some(user) = user_message {
                chat_session.session.messages.push(user);
            }
            
            // Add the reduced content as a cached context for next iteration
            chat_session.session.add_message("assistant", &reduced_content);
            let last_index = chat_session.session.messages.len() - 1;
            chat_session.session.messages[last_index].cached = true;
            
            // Save stats for the reduction
            if let Some(usage) = &exchange.usage {
                // Calculate cost if available, or estimate it
                let cost = if let Some(cost_credits) = usage.cost {
                    // Convert from credits to dollars (100,000 credits = $1)
                    cost_credits as f64 / 100000.0
                } else {
                    // Fallback to estimating cost using model pricing
                    let input_price = config.openrouter.pricing.input_price;
                    let output_price = config.openrouter.pricing.output_price;
                    let input_cost = usage.prompt_tokens as f64 * input_price;
                    let output_cost = usage.completion_tokens as f64 * output_price;
                    input_cost + output_cost
                };
                
                // Add the stats to the session
                chat_session.session.add_layer_stats(
                    "context_optimizer",
                    &reducer_model,
                    usage.prompt_tokens,
                    usage.completion_tokens,
                    cost
                );
            }
            
            println!("{}", "Session context optimized for next interaction".bright_green());
            println!("{}", "The next message will be processed through the full layered pipeline.".bright_cyan());
            
            // Save the updated session
            chat_session.save()?;
            
            Ok(())
        },
        Err(e) => {
            println!("{}: {}", "Error during context reduction".bright_red(), e);
            Err(anyhow::anyhow!("Context reduction failed: {}", e))
        }
    }
}