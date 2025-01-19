use crate::state::SharedState;
use std::fs;
use qdrant_client::qdrant::{Condition, CreateCollectionBuilder, Filter, ScrollPointsBuilder, UpsertPointsBuilder, VectorParamsBuilder};
use walkdir::WalkDir;
use qdrant_client::qdrant::{PointStruct, Value, Distance};
use qdrant_client::{Qdrant, Payload};
use serde::{Deserialize, Serialize};
use tree_sitter::Parser;
use uuid::Uuid;
use serde_json::{json, Value as JsonValue};
use reqwest::Client;
use anyhow::{Result, Context};
use sha2;

// Function to calculate file hash
fn calculate_content_hash(contents: &str) -> String {
	use sha2::{Sha256, Digest};
	let mut hasher = Sha256::new();
	hasher.update(contents.as_bytes());
	format!("{:x}", hasher.finalize())
}

async fn content_already_indexed(
	client: &Qdrant,
	hash: &str,
	collection: &str
) -> Result<bool, Box<dyn std::error::Error>> {
	let filter = Filter::must([
		Condition::matches("hash", hash.to_string()),
	]);
	let response = client
		.scroll(
			ScrollPointsBuilder::new(collection)
				.filter(filter)
				.limit(1)
				.with_payload(false)
				.with_vectors(false),
		)
	.await?;
	Ok(!response.result.is_empty())
}

#[derive(Serialize, Deserialize, Clone)]
struct CodeBlock {
	path: String,
	language: String,
	content: String,
	symbols: Vec<String>,
	start_line: usize,
	end_line: usize,
	hash: String,
}

impl From<CodeBlock> for Payload {
	fn from(block: CodeBlock) -> Self {
		let mut payload = Payload::new();
		payload.insert("path".to_string(), Value::from(block.path));
		payload.insert("language".to_string(), Value::from(block.language));
		payload.insert("content".to_string(), Value::from(block.content));
		payload.insert("symbols".to_string(), Value::from(block.symbols));
		payload.insert("start_line".to_string(), Value::from(block.start_line as i64));
		payload.insert("end_line".to_string(), Value::from(block.end_line as i64));
		payload.insert("hash".to_string(), Value::from(block.hash));
		payload
	}
}

impl From<TextBlock> for Payload {
	fn from(block: TextBlock) -> Self {
		let mut payload = Payload::new();
		payload.insert("path".to_string(), Value::from(block.path));
		payload.insert("content".to_string(), Value::from(block.content));
		payload.insert("start_line".to_string(), Value::from(block.start_line as i64));
		payload.insert("end_line".to_string(), Value::from(block.end_line as i64));
		payload.insert("hash".to_string(), Value::from(block.hash));
		payload
	}
}

#[derive(Serialize, Deserialize, Clone)]
struct TextBlock {
	path: String,
	content: String,
	start_line: usize,
	end_line: usize,
	hash: String,
}


// Batch embedding generation
async fn generate_embeddings_batch(
	texts: Vec<String>,
	_model: &str
) -> Result<Vec<Vec<f32>>> {
	let mut res = vec![];
	for _text in texts.iter() {
		res.push(vec![0.1; 768]);
	}
	Ok(res)
	// let client = Client::new();
	// let jina_api_key = std::env::var("JINA_API_KEY")
	// 	.context("JINA_API_KEY environment variable not set")?;

	// let response = client
	// 	.post("https://api.jina.ai/v1/embeddings")
	// 	.header("Authorization", format!("Bearer {}", jina_api_key))
	// 	.json(&json!({
	// 	"input": texts,
	// 	"model": model,
	// 	}))
	// 	.send()
	// .await?;

	// let response_json: JsonValue = response.json().await?;

	// let embeddings = response_json["data"]
	// 	.as_array()
	// 	.context("Failed to get embeddings array")?
	// 	.iter()
	// 	.map(|data| {
	// 		data["embedding"]
	// 			.as_array()
	// 			.unwrap_or(&Vec::new())
	// 			.iter()
	// 			.map(|v| v.as_f64().unwrap_or_default() as f32)
	// 			.collect()
	// 	})
	// 	.collect();

	// Ok(embeddings)
}

fn detect_language(path: &std::path::Path) -> Option<&str> {
	match path.extension()?.to_str()? {
		"rs" => Some("rust"),
		"php" => Some("php"),
		"sh" => Some("bash"),
		_ => None,
	}
}

pub async fn index_files(state: SharedState) -> Result<(), Box<dyn std::error::Error>> {
	let client = Qdrant::from_url("http://localhost:6334").build()?;

	// Create collections if they don't exist
	for collection_name in ["code_blocks", "text_blocks"] {
		match client.collection_info(collection_name).await {
			Ok(_) => (),
			Err(_) => {
				client.create_collection(
					CreateCollectionBuilder::new(collection_name)
						.vectors_config(
							VectorParamsBuilder::new(768, Distance::Cosine)
						)
				).await?;
			}
		}
	}

	// Collect files to process
	let current_dir = state.read().current_directory.clone();
	let mut code_blocks_batch = Vec::new();
	let mut text_blocks_batch = Vec::new();

	// Process files in batches
	const BATCH_SIZE: usize = 10;

	for entry in WalkDir::new(current_dir)
		.into_iter()
		.filter_map(|e| e.ok())
		.filter(|e| e.file_type().is_file())
	{
		if let Some(language) = detect_language(entry.path()) {
			if let Ok(contents) = fs::read_to_string(entry.path()) {
				let file_path = entry.path().to_string_lossy().to_string();

				// Process code blocks
				let mut parser = Parser::new();
				let ts_lang = match language {
					"rust" => tree_sitter_rust::LANGUAGE,
					"php" => tree_sitter_php::LANGUAGE_PHP,
					"bash" => tree_sitter_bash::LANGUAGE,
					_ => continue,
				};
				parser.set_language(&ts_lang.into())?;

				let tree = parser.parse(&contents, None).unwrap();
				let mut cursor = tree.walk();

				cursor.goto_first_child();
				loop {
					let node = cursor.node();
					let content = contents[node.start_byte()..node.end_byte()].to_string();
					let content_hash = calculate_content_hash(&content);
					let is_indexed = content_already_indexed(&client, &content_hash, "code_blocks").await?;
					if !is_indexed {
						code_blocks_batch.push(CodeBlock {
							path: file_path.clone(),
							hash: content_hash.clone(),
							language: language.to_string(),
							content: content.clone(),
							symbols: vec![node.kind().to_string()],
							start_line: node.start_position().row,
							end_line: node.end_position().row,
						});
					}

					if !cursor.goto_next_sibling() {
						break;
					}
				}

				let content_hash = calculate_content_hash(&contents);

				let is_indexed = content_already_indexed(&client, &content_hash, "code_blocks").await?;
				if !is_indexed {
					// Add text block
					text_blocks_batch.push(TextBlock {
						path: file_path.clone(),
						hash: content_hash.clone(),
						content: contents.clone(),
						start_line: 0,
						end_line: contents.lines().count(),
					});
				}
			}
		}

		// Process batches when they reach the size limit
		if code_blocks_batch.len() >= BATCH_SIZE {
			process_code_blocks_batch(&client, &code_blocks_batch).await?;
			code_blocks_batch.clear();
		}
		if text_blocks_batch.len() >= BATCH_SIZE {
			process_text_blocks_batch(&client, &text_blocks_batch).await?;
			text_blocks_batch.clear();
		}
	}

	// Process remaining items
	if !code_blocks_batch.is_empty() {
		process_code_blocks_batch(&client, &code_blocks_batch).await?;
	}
	if !text_blocks_batch.is_empty() {
		process_text_blocks_batch(&client, &text_blocks_batch).await?;
	}

	let mut state_guard = state.write();
	state_guard.indexing_complete = true;

	Ok(())
}

async fn process_code_blocks_batch(
	client: &Qdrant,
	blocks: &[CodeBlock]
) -> Result<(), Box<dyn std::error::Error>> {
	let contents: Vec<String> = blocks.iter()
		.map(|block| block.content.clone())
		.collect();

	let embeddings = generate_embeddings_batch(
		contents,
		"jina-embeddings-v2-base-code"
	).await?;

	let points: Vec<PointStruct> = blocks.iter()
		.zip(embeddings.iter())
		.map(|(block, embedding)| {
			PointStruct::new(
				Uuid::new_v4().to_string(),
				(*embedding).clone(),
				(*block).clone(),
			)
		})
		.collect();

	client.upsert_points(
		UpsertPointsBuilder::new(
			"code_blocks".to_string(),
			points
		).wait(true)
	).await?;

	Ok(())
}

async fn process_text_blocks_batch(
	client: &Qdrant,
	blocks: &[TextBlock]
) -> Result<(), Box<dyn std::error::Error>> {
	let contents: Vec<String> = blocks.iter()
		.map(|block| block.content.clone())
		.collect();

	let embeddings = generate_embeddings_batch(
		contents,
		"jina-embeddings-v3"
	).await?;


	let points: Vec<PointStruct> = blocks.iter()
		.zip(embeddings.iter().cloned())
		.map(|(block, embedding)| {
			PointStruct::new(
				Uuid::new_v4().to_string(),
				embedding,
				(*block).clone(),
			)
		})
		.collect();

	client.upsert_points(
		UpsertPointsBuilder::new(
			"text_blocks".to_string(),
			points
		).wait(true)
	).await?;

	Ok(())
}

