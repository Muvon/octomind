use crate::store::CodeBlock;
use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub fn calculate_content_hash(contents: &str) -> String {
	let mut hasher = Sha256::new();
	hasher.update(contents.as_bytes());
	format!("{:x}", hasher.finalize())
}

pub async fn generate_embeddings(contents: &str, model: &str) -> Result<Vec<f32>> {
	let result = generate_embeddings_batch(vec![contents.to_string()], &model).await?;

	match result.first() {
		Some(value) => Ok(value.to_vec()),
		None => Err(anyhow::anyhow!("No embeddings found"))
	}
}

pub async fn generate_embeddings_batch(texts: Vec<String>, model: &str) -> Result<Vec<Vec<f32>>> {
	let client = Client::new();
	let jina_api_key = std::env::var("JINA_API_KEY")
		.context("JINA_API_KEY environment variable not set")?;

	let response = client
		.post("https://api.jina.ai/v1/embeddings")
		.header("Authorization", format!("Bearer {}", jina_api_key))
		.json(&json!({
		"input": texts,
		"model": model,
		}))
		.send()
	.await?;

	let response_json: Value = response.json().await?;

	let embeddings = response_json["data"]
		.as_array()
		.context("Failed to get embeddings array")?
		.iter()
		.map(|data| {
			data["embedding"]
				.as_array()
				.unwrap_or(&Vec::new())
				.iter()
				.map(|v| v.as_f64().unwrap_or_default() as f32)
				.collect()
		})
		.collect();

	Ok(embeddings)
}

pub fn render_code_blocks(blocks: &[CodeBlock]) {
    if blocks.is_empty() {
        println!("No code blocks found for the query.");
        return;
    }
    
    println!("Found {} code blocks:\n", blocks.len());

    // Group blocks by file path for better organization
    let mut blocks_by_file: std::collections::HashMap<String, Vec<&CodeBlock>> = std::collections::HashMap::new();
    
    for block in blocks {
        blocks_by_file
            .entry(block.path.clone())
            .or_insert_with(Vec::new)
            .push(block);
    }
    
    // Print results organized by file
    for (file_path, file_blocks) in blocks_by_file.iter() {
        println!("╔══════════════════ File: {} ══════════════════", file_path);
        
        for (idx, block) in file_blocks.iter().enumerate() {
            println!("║");
            println!("║ Block {} of {} in file", idx + 1, file_blocks.len());
            println!("║ Language: {}", block.language);
            println!("║ Lines: {}-{}", block.start_line, block.end_line);
            
            if !block.symbols.is_empty() {
                println!("║ Symbols:");
                for symbol in &block.symbols {
                    // Only show non-type symbols to users
                    if !symbol.contains("_") {
                        println!("║   • {}", symbol);
                    }
                }
            }

            println!("║ Content:");
            println!("║ ┌────────────────────────────────────");
            for line in block.content.lines() {
                println!("║ │ {}", line);
            }
            println!("║ └────────────────────────────────────");
        }
        
        println!("╚════════════════════════════════════════\n");
    }
}

