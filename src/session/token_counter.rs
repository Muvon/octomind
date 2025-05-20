// Token counting utilities

use tiktoken_rs::cl100k_base;

// Simple token counter that uses tiktoken to estimate token counts
pub fn estimate_tokens(text: &str) -> usize {
    // Use CL100K base tokenizer (used by Claude and GPT models)
    let tokenizer = match cl100k_base() {
        Ok(t) => t,
        Err(_) => return text.len() / 4, // Fallback to rough approximation
    };
    
    let tokens = tokenizer.encode_ordinary(text);
    tokens.len()
}

// Estimate tokens for a full message list
pub fn estimate_message_tokens(messages: &[crate::session::Message]) -> usize {
    let mut total = 0;
    
    for msg in messages {
        // Add ~4 tokens for role
        total += 4;
        
        // Add content tokens
        total += estimate_tokens(&msg.content);
    }
    
    // Add some overhead for message formatting
    total += messages.len() * 2;
    
    total
}