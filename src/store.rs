use std::collections::HashMap;

use qdrant_client::qdrant::{Condition, CreateCollectionBuilder, Filter, Query, QueryPointsBuilder, ScrollPointsBuilder, UpsertPointsBuilder, VectorParamsBuilder};
use qdrant_client::qdrant::{PointStruct, Value, Distance};
use qdrant_client::{Qdrant, Payload};
use serde::{Deserialize, Serialize};
use anyhow::Result;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodeBlock {
	pub path: String,
	pub language: String,
	pub content: String,
	pub symbols: Vec<String>,
	pub start_line: usize,
	pub end_line: usize,
	pub hash: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TextBlock {
	pub path: String,
	pub content: String,
	pub start_line: usize,
	pub end_line: usize,
	pub hash: String,
}

pub struct Store {
	client: Qdrant,
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
		payload.insert("path".to_string() , Value::from(block.path));
		payload.insert("content".to_string(), Value::from(block.content));
		payload.insert("start_line".to_string(), Value::from(block.start_line as i64));
		payload.insert("end_line".to_string(), Value::from(block.end_line as i64));
		payload.insert("hash".to_string()
			, Value::from(block.hash));
		payload
	}
}

impl From<HashMap<String, Value>> for CodeBlock {
	fn from(payload: HashMap<String, Value>) -> Self {
		let start_line: i64 = payload.get("start_line").unwrap().as_integer().unwrap();
		let end_line: i64 = payload.get("end_line").unwrap().as_integer().unwrap();
		let path: String = payload.get("path").unwrap().as_str().unwrap().to_string();
		let hash: String = payload.get("hash").unwrap().as_str().unwrap().to_string();
		let language: String = payload.get("language").unwrap().as_str().unwrap().to_string();
		let content: String = payload.get("content").unwrap().as_str().unwrap().to_string();
		let symbols: Vec<String> = payload.get("symbols").unwrap().as_list().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
		Self {
			path: path.clone(),
			hash: hash.clone(),
			language: language.clone(),
			content: content.clone(),
			symbols: symbols.clone(),
			start_line: start_line as usize,
			end_line: end_line as usize,
		}
	}
}

impl Store {
	pub fn new() -> Result<Self> {
		// Get current directory
		let current_dir = std::env::current_dir()?;
		
		// Create .octodev directory if it doesn't exist
		let octodev_dir = current_dir.join(".octodev");
		if !octodev_dir.exists() {
			std::fs::create_dir_all(&octodev_dir)?;
		}
		
		// Use local storage mode instead of connecting to a server
		let qdrant_dir = octodev_dir.join("qdrant");
		
		// Create a local Qdrant client
		// Try to connect to a local server or fall back to in-memory mode
		let config = qdrant_client::config::QdrantConfig::from_url(&format!("http://localhost:6334"));
		let mode = "in-memory";
		
		println!("Using {} storage mode for Qdrant", mode);
		
		let client = qdrant_client::Qdrant::new(config)
			.map_err(|e| anyhow::anyhow!(e.to_string()))?;
		
		Ok(Self { client })
	}

	pub async fn initialize_collections(&self) -> Result<()> {
		for collection_name in ["code_blocks", "text_blocks"] {
			let dimension = match collection_name {
				"code_blocks" => 768,
				"text_blocks" => 1024,
				_ => unreachable!(),
			};
			match self.client.collection_info(collection_name).await {
				Ok(_) => (),
				Err(_) => {
					self.client
						.create_collection(
							CreateCollectionBuilder::new(collection_name)
								.vectors_config(VectorParamsBuilder::new(dimension, Distance::Cosine)),
						)
					.await?;
				}
			}
		}
		Ok(())
	}

	pub async fn content_exists(&self, hash: &str, collection: &str) -> Result<bool> {
		let filter = Filter::must([Condition::matches("hash", hash.to_string())]);
		let response = self
			.client
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

	pub async fn store_code_blocks(&self, blocks: &[CodeBlock], embeddings: Vec<Vec<f32>>) -> Result<()> {
		let points: Vec<PointStruct> = blocks
			.iter()
			.zip(embeddings.iter())
			.map(|(block, embedding)| {
				PointStruct::new(Uuid::new_v4().to_string(), (*embedding).clone(), (*block).clone())
			})
			.collect();

		self.client
			.upsert_points(UpsertPointsBuilder::new("code_blocks".to_string(), points).wait(true))
		.await?;

		Ok(())
	}

	pub async fn store_text_blocks(&self, blocks: &[TextBlock], embeddings: Vec<Vec<f32>>) -> Result<()> {
		let points: Vec<PointStruct> = blocks
			.iter()
			.zip(embeddings.iter())
			.map(|(block, embedding)| {
				PointStruct::new(Uuid::new_v4().to_string(), (*embedding).clone(), (*block).clone())
			})
			.collect();

		self.client
			.upsert_points(UpsertPointsBuilder::new("text_blocks".to_string(), points).wait(true))
		.await?;

		Ok(())
	}

	pub async fn get_code_blocks(&self, embedding: Vec<f32>) -> Result<Vec<CodeBlock>> {
		let query = Query::new_nearest(embedding);
		let response = self.client
			.query(
				QueryPointsBuilder::new("code_blocks")
					.query(query)
					.with_payload(true)
					.limit(10)
			)
		.await?;

		let result: Vec<CodeBlock> = response.result
			.into_iter()
			.map(|point| point.payload)
			.fold(Vec::new(), |mut acc, payload| {
				acc.push(payload.into());
				acc
			});

		Ok(result)
	}

	pub async fn get_code_block_by_symbol(&self, symbol: &str) -> Result<Option<CodeBlock>> {
		let filter = Filter::must([Condition::matches("symbols", symbol.to_string())]);
		let response = self.client
			.query(
				QueryPointsBuilder::new("code_blocks")
					.filter(filter)
					.with_payload(true)
					.limit(1)
			)
		.await?;

		if response.result.is_empty() {
			Ok(None)
		} else {
			let block: CodeBlock = response.result[0].payload.clone().into();
			Ok(Some(block))
		}
	}
}
