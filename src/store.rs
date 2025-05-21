use std::sync::Arc;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Arrow imports
use arrow::array::{Array, FixedSizeListArray, Float32Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

// LanceDB imports
use lancedb::{connect, Connection, index::Index, query::{ExecutableQuery, QueryBase}};
use futures::TryStreamExt;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodeBlock {
	pub path: String,
	pub language: String,
	pub content: String,
	pub symbols: Vec<String>,
	pub start_line: usize,
	pub end_line: usize,
	pub hash: String,
	// Optional distance field for relevance sorting (higher is more relevant)
	#[serde(skip_serializing_if = "Option::is_none")]
	pub distance: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TextBlock {
	pub path: String,
	pub language: String,
	pub content: String,
	pub start_line: usize,
	pub end_line: usize,
	pub hash: String,
}

pub struct Store {
	db: Connection,
	vector_dim: usize,  // Size of embedding vectors
}

// Helper struct for converting between Arrow RecordBatch and our domain models
struct BatchConverter {
	vector_dim: usize,
}

impl BatchConverter {
	fn new(vector_dim: usize) -> Self {
		Self { vector_dim }
	}

	// Convert a CodeBlock to a RecordBatch
	fn code_block_to_batch(&self, blocks: &[CodeBlock], embeddings: &[Vec<f32>]) -> Result<RecordBatch> {
		// Ensure we have the same number of blocks and embeddings
		if blocks.len() != embeddings.len() {
			return Err(anyhow::anyhow!("Number of blocks and embeddings must match"));
		}

		if blocks.is_empty() {
			return Err(anyhow::anyhow!("Empty blocks array"));
		}

		// Check if all embedding vectors have the expected dimension
		for (i, embedding) in embeddings.iter().enumerate() {
			if embedding.len() != self.vector_dim {
				return Err(anyhow::anyhow!("Embedding at index {} has dimension {} but expected {}",
					i, embedding.len(), self.vector_dim));
			}
		}

		// Create schema
		let schema = Arc::new(Schema::new(vec![
			Field::new("id", DataType::Utf8, false),
			Field::new("path", DataType::Utf8, false),
			Field::new("language", DataType::Utf8, false),
			Field::new("content", DataType::Utf8, false),
			Field::new("symbols", DataType::Utf8, true),  // Storing serialized JSON of symbols
			Field::new("start_line", DataType::UInt32, false),
			Field::new("end_line", DataType::UInt32, false),
			Field::new("hash", DataType::Utf8, false),
			Field::new(
				"embedding",
				DataType::FixedSizeList(
					Arc::new(Field::new("item", DataType::Float32, true)),
					self.vector_dim as i32,
				),
				true,
			),
		]));

		// Create arrays
		let ids: Vec<String> = blocks.iter().map(|_| Uuid::new_v4().to_string()).collect();
		let paths: Vec<&str> = blocks.iter().map(|b| b.path.as_str()).collect();
		let languages: Vec<&str> = blocks.iter().map(|b| b.language.as_str()).collect();
		let contents: Vec<&str> = blocks.iter().map(|b| b.content.as_str()).collect();
		let symbols: Vec<String> = blocks.iter().map(|b| serde_json::to_string(&b.symbols).unwrap_or_default()).collect();
		let start_lines: Vec<u32> = blocks.iter().map(|b| b.start_line as u32).collect();
		let end_lines: Vec<u32> = blocks.iter().map(|b| b.end_line as u32).collect();
		let hashes: Vec<&str> = blocks.iter().map(|b| b.hash.as_str()).collect();

		// Create the embedding fixed size list array
		let mut flattened_embeddings = Vec::with_capacity(blocks.len() * self.vector_dim);
		for embedding in embeddings {
			flattened_embeddings.extend_from_slice(embedding);
		}
		let values = Float32Array::from(flattened_embeddings);

		// Create the fixed size list array
		let embedding_array = FixedSizeListArray::new(
			Arc::new(Field::new("item", DataType::Float32, true)),
			self.vector_dim as i32,
			Arc::new(values),
			None, // No validity buffer - all values are valid
		);

		// Verify all arrays have the same length
		let expected_len = blocks.len();
		assert_eq!(ids.len(), expected_len, "ids array length mismatch");
		assert_eq!(paths.len(), expected_len, "paths array length mismatch");
		assert_eq!(languages.len(), expected_len, "languages array length mismatch");
		assert_eq!(contents.len(), expected_len, "contents array length mismatch");
		assert_eq!(symbols.len(), expected_len, "symbols array length mismatch");
		assert_eq!(start_lines.len(), expected_len, "start_lines array length mismatch");
		assert_eq!(end_lines.len(), expected_len, "end_lines array length mismatch");
		assert_eq!(hashes.len(), expected_len, "hashes array length mismatch");
		assert_eq!(embedding_array.len(), expected_len, "embedding_array length mismatch");

		// Create record batch
		let batch = RecordBatch::try_new(
			schema,
			vec![
				Arc::new(StringArray::from(ids)),
				Arc::new(StringArray::from(paths)),
				Arc::new(StringArray::from(languages)),
				Arc::new(StringArray::from(contents)),
				Arc::new(StringArray::from(symbols)),
				Arc::new(UInt32Array::from(start_lines)),
				Arc::new(UInt32Array::from(end_lines)),
				Arc::new(StringArray::from(hashes)),
				Arc::new(embedding_array),
			],
		)?;

		Ok(batch)
	}

	// Convert a TextBlock to a RecordBatch
	fn text_block_to_batch(&self, blocks: &[TextBlock], embeddings: &[Vec<f32>]) -> Result<RecordBatch> {
		// Ensure we have the same number of blocks and embeddings
		if blocks.len() != embeddings.len() {
			return Err(anyhow::anyhow!("Number of blocks and embeddings must match"));
		}

		if blocks.is_empty() {
			return Err(anyhow::anyhow!("Empty blocks array"));
		}

		// Check if all embedding vectors have the expected dimension
		for (i, embedding) in embeddings.iter().enumerate() {
			if embedding.len() != self.vector_dim {
				return Err(anyhow::anyhow!("Embedding at index {} has dimension {} but expected {}",
					i, embedding.len(), self.vector_dim));
			}
		}

		// Create schema
		let schema = Arc::new(Schema::new(vec![
			Field::new("id", DataType::Utf8, false),
			Field::new("path", DataType::Utf8, false),
			Field::new("language", DataType::Utf8, false),
			Field::new("content", DataType::Utf8, false),
			Field::new("start_line", DataType::UInt32, false),
			Field::new("end_line", DataType::UInt32, false),
			Field::new("hash", DataType::Utf8, false),
			Field::new(
				"embedding",
				DataType::FixedSizeList(
					Arc::new(Field::new("item", DataType::Float32, true)),
					self.vector_dim as i32,
				),
				true,
			),
		]));

		// Create arrays
		let ids: Vec<String> = blocks.iter().map(|_| Uuid::new_v4().to_string()).collect();
		let paths: Vec<&str> = blocks.iter().map(|b| b.path.as_str()).collect();
		let languages: Vec<&str> = blocks.iter().map(|b| b.language.as_str()).collect();
		let contents: Vec<&str> = blocks.iter().map(|b| b.content.as_str()).collect();
		let start_lines: Vec<u32> = blocks.iter().map(|b| b.start_line as u32).collect();
		let end_lines: Vec<u32> = blocks.iter().map(|b| b.end_line as u32).collect();
		let hashes: Vec<&str> = blocks.iter().map(|b| b.hash.as_str()).collect();

		// Create the embedding fixed size list array
		let mut flattened_embeddings = Vec::with_capacity(blocks.len() * self.vector_dim);
		for embedding in embeddings {
			flattened_embeddings.extend_from_slice(embedding);
		}
		let values = Float32Array::from(flattened_embeddings);

		// Create the fixed size list array
		let embedding_array = FixedSizeListArray::new(
			Arc::new(Field::new("item", DataType::Float32, true)),
			self.vector_dim as i32,
			Arc::new(values),
			None, // No validity buffer - all values are valid
		);

		// Verify all arrays have the same length
		let expected_len = blocks.len();
		assert_eq!(ids.len(), expected_len, "ids array length mismatch");
		assert_eq!(paths.len(), expected_len, "paths array length mismatch");
		assert_eq!(languages.len(), expected_len, "languages array length mismatch");
		assert_eq!(contents.len(), expected_len, "contents array length mismatch");
		assert_eq!(start_lines.len(), expected_len, "start_lines array length mismatch");
		assert_eq!(end_lines.len(), expected_len, "end_lines array length mismatch");
		assert_eq!(hashes.len(), expected_len, "hashes array length mismatch");
		assert_eq!(embedding_array.len(), expected_len, "embedding_array length mismatch");

		// Create record batch
		let batch = RecordBatch::try_new(
			schema,
			vec![
				Arc::new(StringArray::from(ids)),
				Arc::new(StringArray::from(paths)),
				Arc::new(StringArray::from(languages)),
				Arc::new(StringArray::from(contents)),
				Arc::new(UInt32Array::from(start_lines)),
				Arc::new(UInt32Array::from(end_lines)),
				Arc::new(StringArray::from(hashes)),
				Arc::new(embedding_array),
			],
		)?;

		Ok(batch)
	}

	// Convert a RecordBatch to a Vec of CodeBlocks
	fn batch_to_code_blocks(&self, batch: &RecordBatch, distances: Option<Vec<f32>>) -> Result<Vec<CodeBlock>> {
		let num_rows = batch.num_rows();
		let mut code_blocks = Vec::with_capacity(num_rows);

		let path_array = batch.column_by_name("path")
			.ok_or_else(|| anyhow::anyhow!("'path' column not found"))?
			.as_any()
			.downcast_ref::<StringArray>()
			.ok_or_else(|| anyhow::anyhow!("'path' column is not a StringArray"))?;

		let language_array = batch.column_by_name("language")
			.ok_or_else(|| anyhow::anyhow!("'language' column not found"))?
			.as_any()
			.downcast_ref::<StringArray>()
			.ok_or_else(|| anyhow::anyhow!("'language' column is not a StringArray"))?;

		let content_array = batch.column_by_name("content")
			.ok_or_else(|| anyhow::anyhow!("'content' column not found"))?
			.as_any()
			.downcast_ref::<StringArray>()
			.ok_or_else(|| anyhow::anyhow!("'content' column is not a StringArray"))?;

		let symbols_array = batch.column_by_name("symbols")
			.ok_or_else(|| anyhow::anyhow!("'symbols' column not found"))?
			.as_any()
			.downcast_ref::<StringArray>()
			.ok_or_else(|| anyhow::anyhow!("'symbols' column is not a StringArray"))?;

		let start_line_array = batch.column_by_name("start_line")
			.ok_or_else(|| anyhow::anyhow!("'start_line' column not found"))?
			.as_any()
			.downcast_ref::<UInt32Array>()
			.ok_or_else(|| anyhow::anyhow!("'start_line' column is not a UInt32Array"))?;

		let end_line_array = batch.column_by_name("end_line")
			.ok_or_else(|| anyhow::anyhow!("'end_line' column not found"))?
			.as_any()
			.downcast_ref::<UInt32Array>()
			.ok_or_else(|| anyhow::anyhow!("'end_line' column is not a UInt32Array"))?;

		let hash_array = batch.column_by_name("hash")
			.ok_or_else(|| anyhow::anyhow!("'hash' column not found"))?
			.as_any()
			.downcast_ref::<StringArray>()
			.ok_or_else(|| anyhow::anyhow!("'hash' column is not a StringArray"))?;

		for i in 0..num_rows {
			let symbols: Vec<String> = if symbols_array.is_null(i) {
				Vec::new()
			} else {
				match serde_json::from_str(symbols_array.value(i)) {
					Ok(symbols) => symbols,
					Err(_) => Vec::new(),
				}
			};

			let distance = distances.as_ref().map(|d| d[i]);

			code_blocks.push(CodeBlock {
				path: path_array.value(i).to_string(),
				language: language_array.value(i).to_string(),
				content: content_array.value(i).to_string(),
				symbols,
				start_line: start_line_array.value(i) as usize,
				end_line: end_line_array.value(i) as usize,
				hash: hash_array.value(i).to_string(),
				distance,
			});
		}

		Ok(code_blocks)
	}
}

// Implementing Drop for the Store
impl Drop for Store {
	fn drop(&mut self) {
		if cfg!(debug_assertions) {
			println!("Store instance dropped, database connection released");
		}
	}
}

impl Store {
	pub async fn new() -> Result<Self> {
		// Get current directory
		let current_dir = std::env::current_dir()?;

		// Create .octodev directory if it doesn't exist
		let octodev_dir = current_dir.join(".octodev");
		if !octodev_dir.exists() {
			std::fs::create_dir_all(&octodev_dir)?
		}

		// Create lancedb storage directory
		let lance_dir = octodev_dir.join("lancedb");
		if !lance_dir.exists() {
			std::fs::create_dir_all(&lance_dir)?
		}

		// Convert the path to a string for the file-based database
		let storage_path = lance_dir.to_str().unwrap();

		// Load the config to get the embedding provider and model info
		let config = crate::config::Config::load()?;
		let vector_dim = match config.embedding_provider {
			crate::config::EmbeddingProvider::Jina => 1536, // Jina models typically use 1536 dimensions
			crate::config::EmbeddingProvider::FastEmbed => {
				// FastEmbed models - determine dimension based on model name
				match config.fastembed.code_model.as_str() {
					"all-MiniLM-L6-v2" => 384,
					"all-MiniLM-L12-v2" => 384,
					"multilingual-e5-small" => 384,
					"multilingual-e5-base" => 768,
					"multilingual-e5-large" => 1024,
					_ => 384, // Default to 384 for unknown FastEmbed models
				}
			}
		};

		// Connect to LanceDB
		let db = connect(storage_path).execute().await?;

		Ok(Self {
			db,
			vector_dim,
		})
	}

	pub async fn initialize_collections(&self) -> Result<()> {
		// Check if tables exist, if not create them
		let table_names = self.db.table_names().execute().await?;

		// Create code_blocks table if it doesn't exist
		if !table_names.contains(&"code_blocks".to_string()) {
			// Create empty table with schema
			let schema = Arc::new(Schema::new(vec![
				Field::new("id", DataType::Utf8, false),
				Field::new("path", DataType::Utf8, false),
				Field::new("language", DataType::Utf8, false),
				Field::new("content", DataType::Utf8, false),
				Field::new("symbols", DataType::Utf8, true),
				Field::new("start_line", DataType::UInt32, false),
				Field::new("end_line", DataType::UInt32, false),
				Field::new("hash", DataType::Utf8, false),
				Field::new(
					"embedding",
					DataType::FixedSizeList(
						Arc::new(Field::new("item", DataType::Float32, true)),
						self.vector_dim as i32,
					),
					true,
				),
			]));

			let _table = self.db.create_empty_table("code_blocks", schema).execute().await?;

			// Note: We'll create the index later when we have data
		}

		// Create text_blocks table if it doesn't exist
		if !table_names.contains(&"text_blocks".to_string()) {
			// Create empty table with schema
			let schema = Arc::new(Schema::new(vec![
				Field::new("id", DataType::Utf8, false),
				Field::new("path", DataType::Utf8, false),
				Field::new("language", DataType::Utf8, false),
				Field::new("content", DataType::Utf8, false),
				Field::new("start_line", DataType::UInt32, false),
				Field::new("end_line", DataType::UInt32, false),
				Field::new("hash", DataType::Utf8, false),
				Field::new(
					"embedding",
					DataType::FixedSizeList(
						Arc::new(Field::new("item", DataType::Float32, true)),
						self.vector_dim as i32,
					),
					true,
				),
			]));

			let _table = self.db.create_empty_table("text_blocks", schema).execute().await?;

			// Note: We'll create the index later when we have data
		}

		Ok(())
	}

	pub async fn content_exists(&self, hash: &str, collection: &str) -> Result<bool> {
		let table = self.db.open_table(collection).execute().await?;

		// Query to check if a record with the given hash exists
		let results = table
			.query()
			.only_if(&format!("hash = '{}'", hash))
			.limit(1)
			.execute()
			.await?
			.try_collect::<Vec<_>>()
		.await?;

		Ok(!results.is_empty() && results[0].num_rows() > 0)
	}

	pub async fn store_code_blocks(&self, blocks: &[CodeBlock], embeddings: Vec<Vec<f32>>) -> Result<()> {
		if blocks.is_empty() {
			return Ok(());
		}

		// Check for dimension mismatches and handle them
		for (i, embedding) in embeddings.iter().enumerate() {
			if embedding.len() != self.vector_dim {
				return Err(anyhow::anyhow!("Embedding at index {} has dimension {} but expected {}",
					i, embedding.len(), self.vector_dim));
			}
		}

		// Convert blocks to RecordBatch
		let converter = BatchConverter::new(self.vector_dim);
		let batch = converter.code_block_to_batch(blocks, &embeddings)?;

		// Open or create the table
		let table = self.db.open_table("code_blocks").execute().await?;

		// Create an iterator that yields this single batch
		use std::iter::once;
		let batch_clone = batch.clone();
		let schema = batch_clone.schema();
		let batches = once(Ok(batch));
		let batch_reader = arrow::record_batch::RecordBatchIterator::new(batches, schema);

		// Add the batch to the table
		table.add(batch_reader).execute().await?;

		// Check if index exists and create it if needed
		let has_index = table.list_indices().await?.iter().any(|idx| idx.columns == vec!["embedding"]);
		let row_count = table.count_rows(None).await?;
		if !has_index && row_count > 256 {
			table.create_index(&["embedding"], Index::Auto).execute().await?
		}

		Ok(())
	}

	pub async fn store_text_blocks(&self, blocks: &[TextBlock], embeddings: Vec<Vec<f32>>) -> Result<()> {
		if blocks.is_empty() {
			return Ok(());
		}

		// Check for dimension mismatches and handle them
		for (i, embedding) in embeddings.iter().enumerate() {
			if embedding.len() != self.vector_dim {
				return Err(anyhow::anyhow!("Embedding at index {} has dimension {} but expected {}",
					i, embedding.len(), self.vector_dim));
			}
		}

		// Convert blocks to RecordBatch
		let converter = BatchConverter::new(self.vector_dim);
		let batch = converter.text_block_to_batch(blocks, &embeddings)?;

		// Open or create the table
		let table = self.db.open_table("text_blocks").execute().await?;

		// Create an iterator that yields this single batch
		use std::iter::once;
		let batch_clone = batch.clone();
		let schema = batch_clone.schema();
		let batches = once(Ok(batch));
		let batch_reader = arrow::record_batch::RecordBatchIterator::new(batches, schema);

		// Add the batch to the table
		table.add(batch_reader).execute().await?;

		// Check if index exists and create it if needed
		let has_index = table.list_indices().await?.iter().any(|idx| idx.columns == vec!["embedding"]);
		let row_count = table.count_rows(None).await?;
		if !has_index && row_count > 256 {
			table.create_index(&["embedding"], Index::Auto).execute().await?
		}

		Ok(())
	}

	pub async fn get_code_blocks(&self, embedding: Vec<f32>) -> Result<Vec<CodeBlock>> {
		// Check embedding dimension
		if embedding.len() != self.vector_dim {
			return Err(anyhow::anyhow!("Search embedding has dimension {} but expected {}",
				embedding.len(), self.vector_dim));
		}

		// Open the table
		let table = self.db.open_table("code_blocks").execute().await?;

		// Check if the table has any data
		let row_count = table.count_rows(None).await?;
		if row_count == 0 {
			// No data, return empty vector
			return Ok(Vec::new());
		}

		// Check if index exists and create it if needed
		let has_index = table.list_indices().await?.iter().any(|idx| idx.columns == vec!["embedding"]);
		let row_count = table.count_rows(None).await?;
		if !has_index && row_count > 256 {
			table.create_index(&["embedding"], Index::Auto).execute().await?
		}

		// Perform vector search
		let results = table
			.query()
			.nearest_to(embedding.as_slice())?  // Pass as slice instead of reference to Vec
			.limit(50)  // Limit to 50 results
			.execute()
			.await?
			.try_collect::<Vec<_>>()
		.await?;

		if results.is_empty() || results[0].num_rows() == 0 {
			return Ok(Vec::new());
		}

		// Extract _distance column which contains similarity scores
		let distance_column = results[0].column_by_name("_distance")
			.ok_or_else(|| anyhow::anyhow!("Distance column not found"))?;

		let distance_array = distance_column.as_any()
			.downcast_ref::<Float32Array>()
			.ok_or_else(|| anyhow::anyhow!("Could not downcast distance column to Float32Array"))?;

		// Convert distances to Vec<f32>
		let distances: Vec<f32> = (0..distance_array.len())
			.map(|i| distance_array.value(i))
			.collect();

		// Convert results to CodeBlock structs
		let converter = BatchConverter::new(self.vector_dim);
		let code_blocks = converter.batch_to_code_blocks(&results[0], Some(distances))?;

		Ok(code_blocks)
	}

	pub async fn get_code_block_by_symbol(&self, symbol: &str) -> Result<Option<CodeBlock>> {
		// Open the table
		let table = self.db.open_table("code_blocks").execute().await?;

		// Check if the table has any data
		let row_count = table.count_rows(None).await?;
		if row_count == 0 {
			// No data, return None
			return Ok(None);
		}

		// Filter by symbols using LIKE for substring match
		let results = table
			.query()
			.only_if(&format!("symbols LIKE '%{}%'", symbol))
			.limit(1)
			.execute()
			.await?
			.try_collect::<Vec<_>>()
		.await?;

		if results.is_empty() || results[0].num_rows() == 0 {
			return Ok(None);
		}

		// Convert results to CodeBlock structs
		let converter = BatchConverter::new(self.vector_dim);
		let code_blocks = converter.batch_to_code_blocks(&results[0], None)?;

		// Return the first (and only) code block
		Ok(code_blocks.into_iter().next())
	}

	// Remove all blocks associated with a file path
	pub async fn remove_blocks_by_path(&self, file_path: &str) -> Result<()> {
		// Check if tables exist
		let table_names = self.db.table_names().execute().await?;

		// Delete from code_blocks table if it exists
		if table_names.contains(&"code_blocks".to_string()) {
			let code_blocks_table = self.db.open_table("code_blocks").execute().await?;
			code_blocks_table.delete(&format!("path = '{}'", file_path)).await?;
		}

		// Delete from text_blocks table if it exists
		if table_names.contains(&"text_blocks".to_string()) {
			let text_blocks_table = self.db.open_table("text_blocks").execute().await?;
			text_blocks_table.delete(&format!("path = '{}'", file_path)).await?;
		}

		Ok(())
	}

	// Close the database connection explicitly (for debugging or cleanup)
	pub async fn close(self) -> Result<()> {
		// The database connection is closed automatically when the Store is dropped
		// This method is provided for explicit control over connection lifetime
		Ok(())
	}
}
