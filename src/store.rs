use std::collections::HashMap;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::opt::RecordId;
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

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    id: RecordId,
}

#[derive(Debug, Serialize, Deserialize)]
struct VectorSearch {
    id: RecordId,
    embedding_distance: f32,
    embedding_vector: Vec<f32>,
    #[serde(flatten)]
    payload: HashMap<String, serde_json::Value>,
}

pub struct Store {
	db: Surreal<Db>,
}

impl Store {
	pub async fn new() -> Result<Self> {
		// Get current directory
		let current_dir = std::env::current_dir()?;

		// Create .octodev directory if it doesn't exist
		let octodev_dir = current_dir.join(".octodev");
		if !octodev_dir.exists() {
			std::fs::create_dir_all(&octodev_dir)?;
		}

		// Create surrealdb storage directory
		let surreal_dir = octodev_dir.join("storage");
		if !surreal_dir.exists() {
			std::fs::create_dir_all(&surreal_dir)?;
		}

		// Convert the path to a string for the file-based database
		let storage_path = surreal_dir.to_str().unwrap();

		// Create a connection to the RocksDB storage engine
		let db = Surreal::new::<surrealdb::engine::local::RocksDb>(storage_path)
			.await
			.map_err(|e| anyhow::anyhow!(e.to_string()))?;

        // Select a specific namespace / database
        db.use_ns("octodev").use_db("octodev").await?;

		Ok(Self { db })
	}

	pub async fn initialize_collections(&self) -> Result<()> {
        // Initialize tables with schema - vector search enabled
        // For code_blocks
        self.db.query("
            DEFINE TABLE code_blocks SCHEMALESS;
            DEFINE FIELD path ON code_blocks TYPE string;
            DEFINE FIELD language ON code_blocks TYPE string;
            DEFINE FIELD content ON code_blocks TYPE string;
            DEFINE FIELD symbols ON code_blocks TYPE array;
            DEFINE FIELD start_line ON code_blocks TYPE int;
            DEFINE FIELD end_line ON code_blocks TYPE int;
            DEFINE FIELD hash ON code_blocks TYPE string;
            DEFINE FIELD embedding_vector ON code_blocks TYPE array;
            DEFINE INDEX code_blocks_hash ON TABLE code_blocks COLUMNS hash UNIQUE;
        ")
        .await?;

        // For text_blocks
        self.db.query("
            DEFINE TABLE text_blocks SCHEMALESS;
            DEFINE FIELD path ON text_blocks TYPE string;
            DEFINE FIELD content ON text_blocks TYPE string;
            DEFINE FIELD start_line ON text_blocks TYPE int;
            DEFINE FIELD end_line ON text_blocks TYPE int;
            DEFINE FIELD hash ON text_blocks TYPE string;
            DEFINE FIELD embedding_vector ON text_blocks TYPE array;
            DEFINE INDEX text_blocks_hash ON TABLE text_blocks COLUMNS hash UNIQUE;
        ")
        .await?;

		Ok(())
	}

	pub async fn content_exists(&self, hash: &str, collection: &str) -> Result<bool> {
		let query = format!("SELECT * FROM {} WHERE hash = $hash LIMIT 1", collection);
		let mut result = self.db
			.query(query)
			.bind(("hash", hash))
			.await?;

		let records: Vec<Record> = result.take(0)?;
		Ok(!records.is_empty())
	}

	pub async fn store_code_blocks(&self, blocks: &[CodeBlock], embeddings: Vec<Vec<f32>>) -> Result<()> {
		for (block, embedding) in blocks.iter().zip(embeddings.iter()) {
			let id = Uuid::new_v4().to_string();

			let result = self.db
				.query("CREATE code_blocks SET id = $id, path = $path, language = $language, content = $content,
					symbols = $symbols, start_line = $start_line, end_line = $end_line, hash = $hash, embedding_vector = $embedding")
				.bind(("id", &id))
				.bind(("path", &block.path))
				.bind(("language", &block.language))
				.bind(("content", &block.content))
				.bind(("symbols", &block.symbols))
				.bind(("start_line", block.start_line as i64))
				.bind(("end_line", block.end_line as i64))
				.bind(("hash", &block.hash))
				.bind(("embedding", embedding))
				.await?;

			result.check()?;
		}

		Ok(())
	}

	pub async fn store_text_blocks(&self, blocks: &[TextBlock], embeddings: Vec<Vec<f32>>) -> Result<()> {
		for (block, embedding) in blocks.iter().zip(embeddings.iter()) {
			let id = Uuid::new_v4().to_string();

			let result = self.db
				.query("CREATE text_blocks SET id = $id, path = $path, content = $content,
					start_line = $start_line, end_line = $end_line, hash = $hash, embedding_vector = $embedding")
				.bind(("id", &id))
				.bind(("path", &block.path))
				.bind(("content", &block.content))
				.bind(("start_line", block.start_line as i64))
				.bind(("end_line", block.end_line as i64))
				.bind(("hash", &block.hash))
				.bind(("embedding", embedding))
				.await?;

			result.check()?;
		}

		Ok(())
	}

	pub async fn get_code_blocks(&self, embedding: Vec<f32>) -> Result<Vec<CodeBlock>> {
		// Using a SurrealQL query with vector similarity using dot product
        // Note: We could use cosine similarity as in Qdrant, but SurrealDB's vector_dot is available by default
		let mut result = self.db
			.query(r#"
                SELECT *, vector::dot(embedding_vector, $query_embedding) AS embedding_distance
                FROM code_blocks
                ORDER BY embedding_distance DESC
                LIMIT 50;
            "#)
			.bind(("query_embedding", embedding))
			.await?;

		let vector_results: Vec<VectorSearch> = result.take(0)?;

		// Convert results to CodeBlock structs
		let results: Vec<CodeBlock> = vector_results
			.into_iter()
			.map(|vr| {
				CodeBlock {
					path: vr.payload.get("path").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
					language: vr.payload.get("language").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
					content: vr.payload.get("content").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
					symbols: vr.payload.get("symbols")
						.and_then(|v| v.as_array())
						.map(|a| a.iter().filter_map(|item| item.as_str().map(|s| s.to_string())).collect())
						.unwrap_or_default(),
					start_line: vr.payload.get("start_line").and_then(|v| v.as_i64()).unwrap_or_default() as usize,
					end_line: vr.payload.get("end_line").and_then(|v| v.as_i64()).unwrap_or_default() as usize,
					hash: vr.payload.get("hash").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
				}
			})
			.collect();

		Ok(results)
	}

	pub async fn get_code_block_by_symbol(&self, symbol: &str) -> Result<Option<CodeBlock>> {
        // Query by symbol
		let mut result = self.db
			.query(r#"
                SELECT *
                FROM code_blocks
                WHERE $symbol IN symbols
                LIMIT 1;
            "#)
			.bind(("symbol", symbol))
			.await?;

		// Process the result
		let records: Vec<serde_json::Value> = result.take(0)?;

		if records.is_empty() {
			return Ok(None);
		}

		// Convert the first record to a CodeBlock
		let record = &records[0];

		let code_block = CodeBlock {
			path: record.get("path").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
			language: record.get("language").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
			content: record.get("content").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
			symbols: record.get("symbols")
				.and_then(|v| v.as_array())
				.map(|a| a.iter().filter_map(|item| item.as_str().map(|s| s.to_string())).collect())
				.unwrap_or_default(),
			start_line: record.get("start_line").and_then(|v| v.as_i64()).unwrap_or_default() as usize,
			end_line: record.get("end_line").and_then(|v| v.as_i64()).unwrap_or_default() as usize,
			hash: record.get("hash").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
		};

		Ok(Some(code_block))
	}

	// Remove all blocks associated with a file path
	pub async fn remove_blocks_by_path(&self, file_path: &str) -> Result<()> {
		// Delete all code blocks with the given path
		let result = self.db
			.query("DELETE FROM code_blocks WHERE path = $path")
			.bind(("path", file_path))
			.await?;
		result.check()?;

		// Delete all text blocks with the given path
		let result = self.db
			.query("DELETE FROM text_blocks WHERE path = $path")
			.bind(("path", file_path))
			.await?;
		result.check()?;

		Ok(())
	}
}
