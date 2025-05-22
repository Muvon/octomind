// GraphRAG module for OctoDev
// Handles code relationship extraction and graph generation

use crate::config::Config;
use crate::store::{Store, CodeBlock};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::array::Array;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::RwLock;
use std::sync::Arc;
use reqwest::Client;
use serde_json::json;
use fastembed::{TextEmbedding, EmbeddingModel, InitOptions};
use std::fs;
use std::path::Path;

// A node in the code graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeNode {
	pub id: String,           // Unique ID (typically the path + name)
	pub name: String,         // Name of the code entity (function, class, etc.)
	pub kind: String,         // Type of the node (function, class, struct, etc.)
	pub path: String,         // File path
	pub description: String,  // Description/summary of what the node does
	pub symbols: Vec<String>, // Associated symbols
	pub hash: String,         // Content hash to detect changes
	pub embedding: Vec<f32>,  // Vector embedding of the node
}

// A relationship between code nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeRelationship {
	pub source: String,      // Source node ID
	pub target: String,      // Target node ID
	pub relation_type: String, // Type of relationship (calls, imports, extends, etc.)
	pub description: String, // Description of the relationship
	pub confidence: f32,     // Confidence score of this relationship
}

// The full code graph
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodeGraph {
	pub nodes: HashMap<String, CodeNode>,
	pub relationships: Vec<CodeRelationship>,
}

// A simple prompt template for extracting code descriptions
const DESCRIPTION_PROMPT: &str = r#"You are an expert code summarizer.
Your task is to provide a brief, clear description of what the following code does.
Limit your response to 2 sentences maximum, focusing only on the main functionality.
Don't list parameters or mention "this code" or "this function".
Don't use codeblocks or formatting.

Code:
```
{code}
```

Description:"#;

// A prompt template for extracting relationships for a single pair
const RELATIONSHIP_PROMPT: &str = r#"You are an expert code analyzer.
Your task is to identify relationships between two code entities and return them in JSON format.

Here are two code entities:

Entity 1 (Source):
Name: {source_name}
Kind: {source_kind}
Description: {source_description}
Code: ```
{source_code}
```

Entity 2 (Target):
Name: {target_name}
Kind: {target_kind}
Description: {target_description}
Code: ```
{target_code}
```

Analyze these entities and detect possible relationships between them.
Only respond with a JSON object containing the following fields:
- relation_type: A simple relationship type like "calls", "imports", "extends", "implements", "uses", "defines", "references", etc.
- description: A brief description of this relationship (max 1 sentence)
- confidence: A number between 0.0 and 1.0 representing your confidence in this relationship
- exists: Boolean indicating whether a relationship exists at all

Only return the JSON response and nothing else. If you do not detect any relationship, set exists to false."#;

// Define a constant for multi-relationship batching
const MULTI_REL_BATCH_PROMPT: &str = r#"You are an expert code analyzer.
Your task is to identify relationships between multiple pairs of code entities and return them in JSON format.

I will provide you with multiple pairs of code entities to analyze. For each pair, determine if there's a relationship between them.

Format your response as a JSON array where each element contains:
- source_id: ID of the source entity
- target_id: ID of the target entity
- relation_type: A simple relationship type like "calls", "imports", "extends", "implements", "uses", "defines", "references", etc.
- description: A brief description of this relationship (max 1 sentence)
- confidence: A number between 0.0 and 1.0 representing your confidence in this relationship
- exists: Boolean indicating whether a relationship exists at all

Only include pairs where a relationship exists (exists=true).

Here are the code entity pairs to analyze:

{pairs}

Respond with a JSON array of relationships.
"#;

// Helper struct for batch relationship analysis request
#[derive(Debug, Serialize, Deserialize)]
struct BatchRelationshipResult {
	source_id: String,
	target_id: String,
	relation_type: String,
	description: String,
	confidence: f32,
	exists: bool,
}

// Manages the creation and storage of the code graph
pub struct GraphBuilder {
	config: Config,
	graph: Arc<RwLock<CodeGraph>>,
	client: Client,
	embedding_model: Arc<TextEmbedding>,
	store: Store,
}

impl GraphBuilder {
	pub async fn new(config: Config) -> Result<Self> {
		// Initialize embedding model
		let cache_dir = std::path::PathBuf::from(".octodev/fastembed");
		std::fs::create_dir_all(&cache_dir).context("Failed to create FastEmbed cache directory")?;

		let model = TextEmbedding::try_new(
			InitOptions::new(EmbeddingModel::AllMiniLML6V2)
				.with_show_download_progress(true)
				.with_cache_dir(cache_dir),
		).context("Failed to initialize embedding model")?;

		// Initialize the store for database access
		let store = Store::new().await?;

		// Load existing graph from database
		let graph = Arc::new(RwLock::new(Self::load_graph(&store).await?));

		Ok(Self {
			config,
			graph,
			client: Client::new(),
			embedding_model: Arc::new(model),
			store,
		})
	}

	// Load the existing graph from database
	async fn load_graph(store: &Store) -> Result<CodeGraph> {
		let mut graph = CodeGraph::default();

		// Check if the tables exist
		if !store.tables_exist(&["graphrag_nodes", "graphrag_relationships"]).await? {
			return Ok(graph); // Return empty graph if tables don't exist
		}

		// Get vector dimension for embedding work
		let vector_dim = store.get_vector_dim();

		// Get all nodes
		let node_batch = store.search_graph_nodes(&vec![0.0; vector_dim], 10000).await?;
		if node_batch.num_rows() == 0 {
			return Ok(graph); // No nodes found
		}

		println!("Loading {} GraphRAG nodes from database...", node_batch.num_rows());

		// Process nodes
		let id_array = node_batch.column_by_name("id").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let name_array = node_batch.column_by_name("name").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let kind_array = node_batch.column_by_name("kind").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let path_array = node_batch.column_by_name("path").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let description_array = node_batch.column_by_name("description").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let symbols_array = node_batch.column_by_name("symbols").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let hash_array = node_batch.column_by_name("hash").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();

		// Get the embedding fixed size list array
		let embedding_array = node_batch.column_by_name("embedding").unwrap()
			.as_any()
			.downcast_ref::<arrow::array::FixedSizeListArray>()
			.unwrap();

		// Get the values of the embedding array
		let embedding_values = embedding_array.values()
			.as_any()
			.downcast_ref::<arrow::array::Float32Array>()
			.unwrap();

		// Process each row
		for i in 0..node_batch.num_rows() {
			let id = id_array.value(i).to_string();
			let name = name_array.value(i).to_string();
			let kind = kind_array.value(i).to_string();
			let path = path_array.value(i).to_string();
			let description = description_array.value(i).to_string();

			// Parse symbols JSON
			let symbols: Vec<String> = if symbols_array.is_null(i) {
				Vec::new()
			} else {
				serde_json::from_str(symbols_array.value(i)).unwrap_or_default()
			};

			let hash = hash_array.value(i).to_string();

			// Extract the embedding for this node
			let embedding_offset = i * embedding_array.value_length() as usize;
			let embedding_len = embedding_array.value_length() as usize;
			let mut embedding = Vec::with_capacity(embedding_len);

			for j in 0..embedding_len {
				embedding.push(embedding_values.value(embedding_offset + j));
			}

			// Create the node
			let node = CodeNode {
				id,
				name,
				kind,
				path,
				description,
				symbols,
				hash,
				embedding,
			};

			// Add to graph
			graph.nodes.insert(node.id.clone(), node);
		}

		// Load relationships
		let rel_batch = store.get_graph_relationships().await?;
		if rel_batch.num_rows() > 0 {
			println!("Loading {} GraphRAG relationships from database...", rel_batch.num_rows());

			// Process relationships
			let source_array = rel_batch.column_by_name("source").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
			let target_array = rel_batch.column_by_name("target").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
			let type_array = rel_batch.column_by_name("relation_type").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
			let desc_array = rel_batch.column_by_name("description").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
			let conf_array = rel_batch.column_by_name("confidence").unwrap().as_any().downcast_ref::<arrow::array::Float32Array>().unwrap();

			// Process each relationship
			for i in 0..rel_batch.num_rows() {
				let relationship = CodeRelationship {
					source: source_array.value(i).to_string(),
					target: target_array.value(i).to_string(),
					relation_type: type_array.value(i).to_string(),
					description: desc_array.value(i).to_string(),
					confidence: conf_array.value(i),
				};

				// Add to graph
				graph.relationships.push(relationship);
			}
		}

		if !graph.nodes.is_empty() {
			println!("Loaded GraphRAG knowledge graph with {} nodes and {} relationships",
				graph.nodes.len(), graph.relationships.len());
		}

		Ok(graph)
	}

	// Save the graph to database with support for incremental updates
	#[allow(dead_code)]
	async fn save_graph(&self) -> Result<()> {
		// Get a read lock on the graph
		let graph = self.graph.read().await;

		// Check if there are any nodes to save
		if graph.nodes.is_empty() {
			println!("No GraphRAG nodes to save to database.");
			return Ok(());
		}

		// First, save the nodes
		println!("Saving {} GraphRAG nodes to database...", graph.nodes.len());

		// Convert the nodes to a RecordBatch
		let nodes_batch = self.nodes_to_batch(&graph.nodes).await?;

		// Clear existing nodes first for a clean update
		self.store.clear_graph_nodes().await?;

		// Store the nodes in the database
		self.store.store_graph_nodes(nodes_batch).await?;

		// Now save the relationships if any exist
		if !graph.relationships.is_empty() {
			println!("Saving {} GraphRAG relationships to database...", graph.relationships.len());

			// Convert the relationships to a RecordBatch
			let rel_batch = self.relationships_to_batch(&graph.relationships).await?;

			// Clear existing relationships first for a clean update
			self.store.clear_graph_relationships().await?;

			// Store the relationships in the database
			self.store.store_graph_relationships(rel_batch).await?;
		}

		println!("GraphRAG knowledge graph saved to database successfully.");

		Ok(())
	}

	// Save just the newly added nodes and relationships in batches
	async fn save_graph_incremental(&self, new_nodes: &[CodeNode], new_relationships: &[CodeRelationship]) -> Result<()> {
		if new_nodes.is_empty() && new_relationships.is_empty() {
			// Nothing to save
			return Ok(());
		}

		// First, save any new nodes
		if !new_nodes.is_empty() {
			println!("Incrementally saving {} new GraphRAG nodes to database...", new_nodes.len());

			// Create a HashMap for the batch conversion function
			let nodes_map: HashMap<String, CodeNode> = new_nodes.iter()
				.map(|node| (node.id.clone(), node.clone()))
				.collect();

			// Convert just these nodes to a RecordBatch
			let nodes_batch = self.nodes_to_batch(&nodes_map).await?;

			// Store the nodes in the database (appending to existing data)
			self.store.store_graph_nodes(nodes_batch).await?;
		}

		// Now save any new relationships
		if !new_relationships.is_empty() {
			println!("Incrementally saving {} new GraphRAG relationships to database...", new_relationships.len());

			// Convert just these relationships to a RecordBatch
			let rel_batch = self.relationships_to_batch(new_relationships).await?;

			// Store the relationships in the database (appending to existing data)
			self.store.store_graph_relationships(rel_batch).await?;
		}

		println!("GraphRAG knowledge graph incrementally updated successfully.");
		Ok(())
	}

	// Convert nodes to a RecordBatch for database storage
	async fn nodes_to_batch(&self, nodes: &HashMap<String, CodeNode>) -> Result<arrow::record_batch::RecordBatch> {
		// Get the vector dimension from the store
		let vector_dim = self.store.get_vector_dim();

		// Create schema
		let schema = Arc::new(Schema::new(vec![
			Field::new("id", DataType::Utf8, false),
			Field::new("name", DataType::Utf8, false),
			Field::new("kind", DataType::Utf8, false),
			Field::new("path", DataType::Utf8, false),
			Field::new("description", DataType::Utf8, false),
			Field::new("symbols", DataType::Utf8, true),  // JSON serialized
			Field::new("hash", DataType::Utf8, false),
			Field::new(
				"embedding",
				DataType::FixedSizeList(
					Arc::new(Field::new("item", DataType::Float32, true)),
					vector_dim as i32,
				),
				true,
			),
		]));

		// Prepare arrays
		let nodes_vec: Vec<&CodeNode> = nodes.values().collect();
		if nodes_vec.is_empty() {
			return Err(anyhow::anyhow!("No nodes to convert to batch"));
		}

		let ids: Vec<&str> = nodes_vec.iter().map(|n| n.id.as_str()).collect();
		let names: Vec<&str> = nodes_vec.iter().map(|n| n.name.as_str()).collect();
		let kinds: Vec<&str> = nodes_vec.iter().map(|n| n.kind.as_str()).collect();
		let paths: Vec<&str> = nodes_vec.iter().map(|n| n.path.as_str()).collect();
		let descriptions: Vec<&str> = nodes_vec.iter().map(|n| n.description.as_str()).collect();
		let symbols: Vec<String> = nodes_vec.iter().map(|n| serde_json::to_string(&n.symbols).unwrap_or_default()).collect();
		let hashes: Vec<&str> = nodes_vec.iter().map(|n| n.hash.as_str()).collect();

		// Create the embedding fixed size list array
		let mut flattened_embeddings = Vec::with_capacity(nodes_vec.len() * vector_dim);
		for node in &nodes_vec {
			// Ensure embeddings have the correct dimension
			if node.embedding.len() != vector_dim {
				return Err(anyhow::anyhow!(
					"Node embedding has dimension {} but expected {}",
					node.embedding.len(), vector_dim
				));
			}
			flattened_embeddings.extend_from_slice(&node.embedding);
		}
		let values = arrow::array::Float32Array::from(flattened_embeddings);

		// Create the fixed size list array
		let embedding_array = arrow::array::FixedSizeListArray::new(
			Arc::new(Field::new("item", DataType::Float32, true)),
			vector_dim as i32,
			Arc::new(values),
			None, // No validity buffer - all values are valid
		);

		// Create record batch
		let batch = arrow::record_batch::RecordBatch::try_new(
			schema,
			vec![
				Arc::new(arrow::array::StringArray::from(ids)),
				Arc::new(arrow::array::StringArray::from(names)),
				Arc::new(arrow::array::StringArray::from(kinds)),
				Arc::new(arrow::array::StringArray::from(paths)),
				Arc::new(arrow::array::StringArray::from(descriptions)),
				Arc::new(arrow::array::StringArray::from(symbols)),
				Arc::new(arrow::array::StringArray::from(hashes)),
				Arc::new(embedding_array),
			],
		)?;

		Ok(batch)
	}

	// Convert relationships to a RecordBatch for database storage
	async fn relationships_to_batch(&self, relationships: &[CodeRelationship]) -> Result<arrow::record_batch::RecordBatch> {
		// Create schema
		let schema = Arc::new(Schema::new(vec![
			Field::new("id", DataType::Utf8, false),
			Field::new("source", DataType::Utf8, false),
			Field::new("target", DataType::Utf8, false),
			Field::new("relation_type", DataType::Utf8, false),
			Field::new("description", DataType::Utf8, false),
			Field::new("confidence", DataType::Float32, false),
		]));

		// Generate unique IDs
		let ids: Vec<String> = relationships.iter().map(|_| uuid::Uuid::new_v4().to_string()).collect();
		let sources: Vec<&str> = relationships.iter().map(|r| r.source.as_str()).collect();
		let targets: Vec<&str> = relationships.iter().map(|r| r.target.as_str()).collect();
		let types: Vec<&str> = relationships.iter().map(|r| r.relation_type.as_str()).collect();
		let descriptions: Vec<&str> = relationships.iter().map(|r| r.description.as_str()).collect();
		let confidences: Vec<f32> = relationships.iter().map(|r| r.confidence).collect();

		// Create record batch
		let batch = arrow::record_batch::RecordBatch::try_new(
			schema,
			vec![
				Arc::new(arrow::array::StringArray::from(ids)),
				Arc::new(arrow::array::StringArray::from(sources)),
				Arc::new(arrow::array::StringArray::from(targets)),
				Arc::new(arrow::array::StringArray::from(types)),
				Arc::new(arrow::array::StringArray::from(descriptions)),
				Arc::new(arrow::array::Float32Array::from(confidences)),
			],
		)?;

		Ok(batch)
	}

	// Process a batch of code blocks and update the graph with batching and incremental saving
	pub async fn process_code_blocks(&self, code_blocks: &[CodeBlock]) -> Result<()> {
		// Create nodes for code blocks that are new or have changed
		let mut new_nodes = Vec::new();
		let mut processed_count = 0;
		let mut skipped_count = 0;

		// Constants for batch processing
		const BATCH_SIZE: usize = 20;  // Number of nodes to process before saving
		const DESC_BATCH_SIZE: usize = 5;  // Number of descriptions to generate in a single batch

		// Prepare for batch description generation
		let mut desc_batch = Vec::with_capacity(DESC_BATCH_SIZE);
		let mut desc_blocks = Vec::with_capacity(DESC_BATCH_SIZE);
		let mut desc_names = Vec::with_capacity(DESC_BATCH_SIZE);
		let mut desc_ids = Vec::with_capacity(DESC_BATCH_SIZE);

		// First pass: check which blocks need processing and collect blocks for description generation
		for block in code_blocks {
			if block.symbols.is_empty() {
				continue; // Skip blocks without symbols
			}

			// Generate a unique ID for this node
			let node_name = self.extract_node_name(block);
			let node_id = format!("{}/{}", block.path, node_name);

			// Check if we already have this node with the same hash
			let graph = self.graph.read().await;
			let needs_processing = match graph.nodes.get(&node_id) {
				Some(existing_node) if existing_node.hash == block.hash => {
					// Skip unchanged nodes
					skipped_count += 1;
					false
				},
				_ => true
			};
			drop(graph); // Release the lock

			if needs_processing {
				// Add to the description batch
				desc_batch.push(block.content.clone());
				desc_blocks.push(block.clone());
				desc_names.push(node_name);
				desc_ids.push(node_id);

				// Process batch if we've reached the batch size
				if desc_batch.len() >= DESC_BATCH_SIZE {
					let batch_nodes = self.process_description_batch(
						&desc_batch, &desc_blocks, &desc_names, &desc_ids
					).await?;
					new_nodes.extend(batch_nodes);

					// Clear batch buffers
					desc_batch.clear();
					desc_blocks.clear();
					desc_names.clear();
					desc_ids.clear();

					processed_count += DESC_BATCH_SIZE;

					// Process and save if we've reached BATCH_SIZE nodes
					if new_nodes.len() >= BATCH_SIZE {
						let batch_nodes = new_nodes.clone();

						// Discover relationships and save this batch
						let relationships = self.discover_relationships_batch(&batch_nodes).await?;
						if !relationships.is_empty() {
							let mut graph = self.graph.write().await;
							graph.relationships.extend(relationships.clone());
							drop(graph);

							// Save incrementally
							self.save_graph_incremental(&batch_nodes, &relationships).await?;
						} else {
							// Save just the nodes incrementally
							self.save_graph_incremental(&batch_nodes, &[]).await?;
						}

						// Clear the new_nodes for the next batch
						new_nodes.clear();
					}
				}
			}
		}

		// Process any remaining blocks in the description batch
		if !desc_batch.is_empty() {
			let batch_nodes = self.process_description_batch(
				&desc_batch, &desc_blocks, &desc_names, &desc_ids
			).await?;
			new_nodes.extend(batch_nodes);
			processed_count += desc_batch.len();
		}

		// Process any remaining nodes
		if !new_nodes.is_empty() {
			let batch_nodes = new_nodes.clone();

			// Discover relationships for the final batch
			let relationships = self.discover_relationships_batch(&batch_nodes).await?;
			if !relationships.is_empty() {
				let mut graph = self.graph.write().await;
				graph.relationships.extend(relationships.clone());
				drop(graph);

				// Save the final batch incrementally
				self.save_graph_incremental(&batch_nodes, &relationships).await?;
			} else {
				// Save just the nodes incrementally
				self.save_graph_incremental(&batch_nodes, &[]).await?;
			}
		}

		// Report total processing stats
		println!("GraphRAG: Completed processing {} new/changed nodes (skipped {} unchanged)", processed_count, skipped_count);

		Ok(())
	}

	// Process a batch of descriptions in one API call
	async fn process_description_batch(
		&self,
		content_batch: &[String],
		blocks: &[CodeBlock],
		names: &[String],
		ids: &[String]
	) -> Result<Vec<CodeNode>> {
		let mut new_nodes = Vec::new();

		// Generate descriptions in a batch
		let descriptions = self.extract_descriptions_batch(content_batch).await?;

		// Process each node with its description
		for i in 0..descriptions.len() {
			let block = &blocks[i];
			let node_name = &names[i];
			let node_id = &ids[i];
			let description = &descriptions[i];

			// Generate embedding for the node
			let embedding = self.generate_embedding(&format!("{} {}", node_name, description)).await?;

			// Create the node
			let node = CodeNode {
				id: node_id.clone(),
				name: node_name.clone(),
				kind: self.determine_node_kind(block),
				path: block.path.clone(),
				description: description.clone(),
				symbols: block.symbols.clone(),
				hash: block.hash.clone(),
				embedding,
			};

			// Add the node to the graph
			let mut graph = self.graph.write().await;
			graph.nodes.insert(node_id.clone(), node.clone());
			drop(graph);

			new_nodes.push(node);
		}

		Ok(new_nodes)
	}

	// Extract descriptions for a batch of code blocks in one API call
	async fn extract_descriptions_batch(&self, code_blocks: &[String]) -> Result<Vec<String>> {
		if code_blocks.is_empty() {
			return Ok(Vec::new());
		}

		// If there's just one block, use the single-block method
		if code_blocks.len() == 1 {
			let description = self.extract_description(&code_blocks[0]).await?;
			return Ok(vec![description]);
		}

		println!("Generating {} descriptions in a batch", code_blocks.len());

		// Prepare a batch prompt for multiple code blocks
		let mut batch_prompt = String::from("You are an expert code summarizer. Provide brief, clear descriptions of what each code block does. \n");
		batch_prompt.push_str("Limit each description to 2 sentences maximum, focusing only on the main functionality. \n");
		batch_prompt.push_str("Don't list parameters or mention \"this code\" or \"this function\". \n");
		batch_prompt.push_str("Don't use codeblocks or formatting. \n\n");
		batch_prompt.push_str("Respond with a JSON object that has a 'descriptions' field containing an array with one description per code block. \n\n");
		batch_prompt.push_str("Code blocks to describe: \n\n");

		// Add each code block
		for (i, code) in code_blocks.iter().enumerate() {
			// Truncate code if it's too long
			let truncated_code = if code.len() > 1200 {
				format!("{} [...]", &code[0..1200])
			} else {
				code.clone()
			};

			batch_prompt.push_str(&format!("Block #{}: ```\n{}\n```\n\n", i+1, truncated_code));
		}

		// Prepare the JSON schema for the response - using an object with array property
		// as required by OpenAI
		let schema = json!({
			"type": "object",
			"properties": {
				"descriptions": {
					"type": "array",
					"items": {
						"type": "string",
						"description": "A brief description of the code block"
					},
					"minItems": code_blocks.len(),
					"maxItems": code_blocks.len()
				}
			},
			"required": ["descriptions"],
			"additionalProperties": false
		});

		// Call the LLM with the batch prompt
		match self.call_llm(
			&self.config.graphrag.description_model,
			batch_prompt,
			Some(schema),
		).await {
			Ok(response) => {
				// Parse the JSON array response
				let descriptions: Vec<String> = match serde_json::from_str(&response) {
					Ok(descs) => descs,
					Err(e) => {
						eprintln!("Warning: Failed to parse descriptions batch: {}", e);
						// Fallback: generate placeholder descriptions
						code_blocks.iter().map(|code| {
							let first_line = code.lines().next().unwrap_or("").trim();
							if !first_line.is_empty() {
								format!("Code starting with: {}", first_line)
							} else {
								"Code block with no description available".to_string()
							}
						}).collect()
					}
				};

				// Cleanup and truncate descriptions
				let cleaned_descriptions: Vec<String> = descriptions.iter().map(|desc| {
					let description = desc.trim();
					if description.len() > 400 {
						format!("{} [...]", &description[0..400])
					} else {
						description.to_string()
					}
				}).collect();

				Ok(cleaned_descriptions)
			},
			Err(e) => {
				// Provide basic fallback descriptions
				eprintln!("Warning: Failed to generate batch descriptions: {}", e);
				Ok(code_blocks.iter().map(|code| {
					let first_line = code.lines().next().unwrap_or("").trim();
					if !first_line.is_empty() {
						format!("Code starting with: {}", first_line)
					} else {
						"Code block with no description available".to_string()
					}
				}).collect())
			}
		}
	}

	// Discover relationships in a batch and return them without modifying the graph
	async fn discover_relationships_batch(&self, new_nodes: &[CodeNode]) -> Result<Vec<CodeRelationship>> {
		if new_nodes.is_empty() {
			return Ok(Vec::new());
		}

		println!("Discovering relationships among {} nodes in batch", new_nodes.len());

		// Get a read lock on the graph to access existing nodes
		let nodes_from_graph = {
			let graph = self.graph.read().await;
			graph.nodes.values().cloned().collect::<Vec<CodeNode>>()
		}; // The lock is released when the block ends

		let mut relationship_candidates = Vec::new();

		// First pass: collect all potential relationship pairs to analyze
		for source_node in new_nodes {
			// Find similar nodes based on embeddings for efficiency
			let candidate_nodes = self.find_similar_nodes(source_node, &nodes_from_graph, 3)?;

			for target_node in candidate_nodes {
				// Skip self-relationships
				if source_node.id == target_node.id {
					continue;
				}

				// Add to candidates list
				relationship_candidates.push((source_node.clone(), target_node.clone()));
			}
		}

		// Process relationship candidates in batches
		// If no candidates, return empty result
		if relationship_candidates.is_empty() {
			return Ok(Vec::new());
		}

		// Use the optimized batch analysis if we have multiple candidates
		if relationship_candidates.len() >= 2 {
			let results = self.analyze_relationships_batch(&relationship_candidates).await?;
			println!("Found {} relationships from {} candidates using batch analysis", results.len(), relationship_candidates.len());
			return Ok(results);
		}

		// Fallback to individual analysis for small batches
		let mut new_relationships = Vec::new();
		let candidates_count = relationship_candidates.len();

		for (source, target) in relationship_candidates {
			if let Some(relationship) = self.analyze_relationship(&source, &target).await? {
				new_relationships.push(relationship);
			}
		}

		println!("Found {} relationships from {} candidates", new_relationships.len(), candidates_count);
		Ok(new_relationships)
	}

	// Analyze a batch of relationship candidates in a single API call
	async fn analyze_relationships_batch(&self, candidates: &[(CodeNode, CodeNode)]) -> Result<Vec<CodeRelationship>> {
		println!("Analyzing batch of {} potential relationships", candidates.len());

		// Prepare the prompt with multiple pairs
		let mut pairs_content = String::new();

		// Add each pair to the prompt
		for (i, (source, target)) in candidates.iter().enumerate() {
			pairs_content.push_str(&format!("Pair #{}:\n", i+1));
			pairs_content.push_str(&format!("Source ID: {}\n", source.id));
			pairs_content.push_str(&format!("Source Name: {}\n", source.name));
			pairs_content.push_str(&format!("Source Kind: {}\n", source.kind));
			pairs_content.push_str(&format!("Source Description: {}\n", source.description));
			pairs_content.push_str(&format!("Source Code: ```\n{}\n```\n\n", self.get_truncated_node_code(source)));

			pairs_content.push_str(&format!("Target ID: {}\n", target.id));
			pairs_content.push_str(&format!("Target Name: {}\n", target.name));
			pairs_content.push_str(&format!("Target Kind: {}\n", target.kind));
			pairs_content.push_str(&format!("Target Description: {}\n", target.description));
			pairs_content.push_str(&format!("Target Code: ```\n{}\n```\n\n", self.get_truncated_node_code(target)));
			pairs_content.push_str("-------------------\n\n");
		}

		// Create the final prompt
		let prompt = MULTI_REL_BATCH_PROMPT.replace("{pairs}", &pairs_content);

let schema = json!({
    "type": "object",
    "properties": {
        "relationships": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "source_id": {"type": "string"},
                    "target_id": {"type": "string"},
                    "relation_type": {"type": "string"},
                    "description": {"type": "string"},
                    "confidence": {"type": "number"},
                    "exists": {"type": "boolean"}
                },
                "required": ["source_id", "target_id", "relation_type", "description", "confidence", "exists"],
				"additionalProperties": false

            }
        }
    },
    "required": ["relationships"],
	"additionalProperties": false
});

		// Call the relationship detection model
		match self.call_llm(
			&self.config.graphrag.relationship_model,
			prompt,
			Some(schema),
		).await {
			Ok(response) => {
// Parse the JSON object response
let response_obj: serde_json::Value = match serde_json::from_str(&response) {
    Ok(obj) => obj,
    Err(e) => {
        eprintln!("Failed to parse batch relationship response: {} - Error: {}", response, e);
        return Ok(Vec::new());
    }
};

// Extract the relationships array
let batch_results: Vec<BatchRelationshipResult> = match response_obj.get("relationships").and_then(|r| r.as_array()) {
    Some(array) => {
        match serde_json::from_value(serde_json::Value::Array(array.clone())) {
            Ok(results) => results,
            Err(e) => {
                eprintln!("Failed to convert relationships array: {}", e);
                Vec::new()
            }
        }
    },
    None => {
        eprintln!("Warning: 'relationships' field not found or not an array");
        Vec::new()
    }
};
				// Convert batch results to CodeRelationships
				let relationships: Vec<CodeRelationship> = batch_results.into_iter()
					.filter(|r| r.exists) // Only include relationships that exist
					.map(|r| CodeRelationship {
						source: r.source_id,
						target: r.target_id,
						relation_type: r.relation_type,
						description: r.description,
						confidence: r.confidence,
					})
					.collect();

				Ok(relationships)
			},
			Err(e) => {
				// If API call fails, log the error and return empty list
				eprintln!("Warning: Failed to analyze batch relationships: {}", e);
				Ok(Vec::new())
			}
		}
	}

	// Extract the node name from a code block
	fn extract_node_name(&self, block: &CodeBlock) -> String {
		// Use the first non-underscore symbol as the name
		for symbol in &block.symbols {
			if !symbol.contains('_') {
				return symbol.clone();
			}
		}

		// Fallback to a generic name with line numbers
		format!("block_{}_{}", block.start_line, block.end_line)
	}

	// Determine the kind of node (function, class, etc.)
	fn determine_node_kind(&self, block: &CodeBlock) -> String {
		// Look for type indicators in the symbols
		for symbol in &block.symbols {
			if symbol.contains("_") {
				let parts: Vec<&str> = symbol.split('_').collect();
				if parts.len() > 1 {
					// Use the first part as the kind
					return parts[0].to_string();
				}
			}
		}

		// Default to "code_block"
		"code_block".to_string()
	}

	// Generate an embedding for node content
	async fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
		let embeddings = self.embedding_model.embed(vec![text], None)?;
		if embeddings.is_empty() {
			return Err(anyhow::anyhow!("Failed to generate embedding"));
		}
		Ok(embeddings[0].clone())
	}

	// Extract a description of the code block using a lightweight LLM
	async fn extract_description(&self, code: &str) -> Result<String> {
		// Truncate code if it's too long
		let truncated_code = if code.len() > 1200 {
			format!("{} [...]\n(code truncated due to length)", &code[0..1200])
		} else {
			code.to_string()
		};


		// Use an inexpensive LLM to generate the description
		match self.call_llm(
			&self.config.graphrag.description_model,
			DESCRIPTION_PROMPT.replace("{code}", &truncated_code),
			None,
		).await {
			Ok(response) => {
				// Cleanup and truncate the description
				let description = response.trim();
				if description.len() > 200 {
					Ok(format!("{} [...]", &description[0..197]))
				} else {
					Ok(description.to_string())
				}
			},
			Err(e) => {
				// Provide a basic fallback description without failing
				eprintln!("Warning: Failed to generate description: {}", e);

				// Create a basic description from the code
				let first_line = code.lines().next().unwrap_or("").trim();
				if !first_line.is_empty() {
					Ok(format!("Code starting with: {}", first_line))
				} else {
					Ok("Code block with no description available".to_string())
				}
			}
		}
	}

	// Discover relationships between nodes
	#[allow(dead_code)]
	async fn discover_relationships(&self, new_nodes: &[CodeNode]) -> Result<()> {
		println!("Discovering relationships among {} nodes", new_nodes.len());

		// Get a read lock on the graph
		let nodes_from_graph = {
			let graph = self.graph.read().await;
			graph.nodes.values().cloned().collect::<Vec<CodeNode>>()
		}; // The lock is released when the block ends

		let mut new_relationships = Vec::new();

		// For each new node, check for relationships with existing nodes
		for source_node in new_nodes {
			// First try to find relationships using embeddings for efficiency
			let candidate_nodes = self.find_similar_nodes(source_node, &nodes_from_graph, 5)?;

			for target_node in candidate_nodes {
				// Skip self-relationships
				if source_node.id == target_node.id {
					continue;
				}

				// Use an LLM to determine if there's a relationship
				let relationship = self.analyze_relationship(
					source_node,
					&target_node,
				).await?;

				// If a relationship was found, add it
				if relationship.is_some() {
					new_relationships.push(relationship.unwrap());
				}
			}
		}

		// Add the new relationships to the graph
		if !new_relationships.is_empty() {
			let mut graph = self.graph.write().await;
			graph.relationships.extend(new_relationships);
		}

		Ok(())
	}

	// Find nodes that are similar to the given node based on embeddings
	fn find_similar_nodes(&self, node: &CodeNode, all_nodes: &[CodeNode], limit: usize) -> Result<Vec<CodeNode>> {
		// Calculate cosine similarity between embeddings
		let mut similarities: Vec<(f32, CodeNode)> = all_nodes.iter()
			.filter(|n| n.id != node.id) // Skip self
			.map(|n| {
				let similarity = cosine_similarity(&node.embedding, &n.embedding);
				(similarity, n.clone())
			})
			.collect();

		// Sort by similarity (highest first)
		similarities.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

		// Take the top matches
		let result = similarities.into_iter()
			.take(limit)
			.map(|(_, node)| node)
			.collect();

		Ok(result)
	}

	// Analyze the relationship between two nodes using an LLM
	async fn analyze_relationship(&self, source: &CodeNode, target: &CodeNode) -> Result<Option<CodeRelationship>> {
		println!("Analyzing relationship between {} and {}", source.name, target.name);

		// Prepare the prompt with the node information
		let prompt = RELATIONSHIP_PROMPT
			.replace("{source_name}", &source.name)
			.replace("{source_kind}", &source.kind)
			.replace("{source_description}", &source.description)
			.replace("{source_code}", &self.get_truncated_node_code(source))
			.replace("{target_name}", &target.name)
			.replace("{target_kind}", &target.kind)
			.replace("{target_description}", &target.description)
			.replace("{target_code}", &self.get_truncated_node_code(target));

		let schema = json!({
			"type": "object",
			"properties": {
				"relation_type": {
					"type": "string",
					"description": "A simple relationship type like 'calls', 'imports', 'extends', 'implements', 'uses', 'defines', 'references', etc."
				},
				"description": {
					"type": "string",
					"description": "A brief description of this relationship (max 1 sentence)"
				},
				"confidence": {
					"type": "number",
					"minimum": 0.0,
					"maximum": 1.0,
					"description": "A number between 0.0 and 1.0 representing confidence in this relationship"
				},
				"exists": {
					"type": "boolean",
					"description": "Boolean indicating whether a relationship exists at all"
				}
			},
			"required": ["relation_type", "description", "confidence", "exists"],
			"additionalProperties": false
		});


		// Call the relationship detection model
		match self.call_llm(
			&self.config.graphrag.relationship_model,
			prompt,
			Some(schema),
		).await {
			Ok(response) => {
				// Parse the JSON response
				let result: RelationshipResult = match serde_json::from_str(&response) {
					Ok(result) => result,
					Err(e) => {
						// If we can't parse the response, log it and return None
						eprintln!("Failed to parse relationship response: {} - Error: {}", response, e);
						return Ok(None);
					}
				};

				// If the model didn't find a relationship, return None
				if !result.exists {
					return Ok(None);
				}

				// Create the relationship object
				let relationship = CodeRelationship {
					source: source.id.clone(),
					target: target.id.clone(),
					relation_type: result.relation_type,
					description: result.description,
					confidence: result.confidence,
				};

				Ok(Some(relationship))
			},
			Err(e) => {
				// If API call fails, log the error and return None without failing
				eprintln!("Warning: Failed to analyze relationship: {}", e);
				Ok(None)
			}
		}
	}

	// Get truncated code for a node to avoid token limits
	fn get_truncated_node_code(&self, node: &CodeNode) -> String {
		// Try to find the code for this node
		// This is a simplified approach - in a real implementation,
		// we would store the code content with the node
		let path = Path::new(&node.path);
		if !path.exists() {
			return "Code not available".to_string();
		}

		match fs::read_to_string(path) {
			Ok(content) => {
				// Truncate to 500 characters if longer
				if content.len() > 500 {
					format!("{} [...]", &content[0..497])
				} else {
					content
				}
			},
			Err(_) => "Failed to read code".to_string(),
		}
	}

	// Call an LLM with the given prompt
	async fn call_llm(&self, model_name: &str, prompt: String, json_schema: Option<serde_json::Value>) -> Result<String> {
		// Debug print for model name
		println!("Using model: {}", model_name);

		// Check if we have an API key configured
		let api_key = match &self.config.openrouter.api_key {
			Some(key) => key.clone(),
			None => return Err(anyhow::anyhow!("OpenRouter API key not configured")),
		};

		// Prepare request body
		let mut request_body = json!({
		"model": model_name,
		"messages": [{
			"role": "user",
			"content": prompt
		}],
			// "max_tokens": 200
		});

		// Only add response_format if schema is provided
		if let Some(schema_value) = json_schema {
			request_body["response_format"] = json!({
				"type": "json_schema",
				"json_schema": {
					"name": "relationship",
					"strict": true,
					"schema": schema_value
				}
			});
		}


		// Call OpenRouter API
		let response = self.client
			.post("https://openrouter.ai/api/v1/chat/completions")
			.header("Authorization", format!("Bearer {}", api_key))
			.header("HTTP-Referer", "https://github.com/muvon/octodev")
			.header("X-Title", "Octodev")
			.json(&request_body)
			.send()
		.await?;

		// Check if the API call was successful
		if !response.status().is_success() {
			let status = response.status();
			let error_text = response.text().await.unwrap_or_else(|_| "Unable to read error response".to_string());
			return Err(anyhow::anyhow!("API error: {} - {}", status, error_text));
		}

		// Parse the response
		let response_json = response.json::<serde_json::Value>().await?;

		// Extract the response text
		if let Some(content) = response_json["choices"][0]["message"]["content"].as_str() {
			Ok(content.to_string())
		} else {
			// Provide more detailed error information
			Err(anyhow::anyhow!("Failed to get response content: {:?}", response_json))
		}
	}

	// Get the full graph
	pub async fn get_graph(&self) -> Result<CodeGraph> {
		let graph = self.graph.read().await;
		Ok(graph.clone())
	}

	// Search the graph for nodes matching a query
	pub async fn search_nodes(&self, query: &str) -> Result<Vec<CodeNode>> {
		// First check if we have any nodes in memory
		let in_memory_nodes = {
			let graph = self.graph.read().await;
			!graph.nodes.is_empty()
		};

		if in_memory_nodes {
			// Use in-memory search if nodes are loaded
			return self.search_nodes_in_memory(query).await;
		} else {
			// Use database search if nodes are only in database
			return self.search_nodes_in_database(query).await;
		}
	}

	// Search for nodes in memory
	async fn search_nodes_in_memory(&self, query: &str) -> Result<Vec<CodeNode>> {
		// Generate an embedding for the query
		let query_embedding = self.generate_embedding(query).await?;

		// Find similar nodes
		let graph = self.graph.read().await;
		let nodes_array = graph.nodes.values().cloned().collect::<Vec<CodeNode>>();
		drop(graph);

		// Calculate similarity to each node
		let mut similarities: Vec<(f32, CodeNode)> = Vec::new();
		let query_lower = query.to_lowercase();

		for node in nodes_array {
			// Calculate semantic similarity
			let similarity = cosine_similarity(&query_embedding, &node.embedding);

			// Check if the query is a substring of various node fields
			// This handles specific cases like searching for "impl"
			let name_contains = node.name.to_lowercase().contains(&query_lower);
			let kind_contains = node.kind.to_lowercase().contains(&query_lower);
			let desc_contains = node.description.to_lowercase().contains(&query_lower);
			let symbols_contain = node.symbols.iter().any(|s| s.to_lowercase().contains(&query_lower));

			// Use a lower threshold for semantic similarity (0.5 instead of 0.6)
			// OR include if the query is a substring of any important field
			if similarity > 0.5 || name_contains || kind_contains || desc_contains || symbols_contain {
				// Boost similarity score for exact matches to ensure they appear at the top
				let boosted_similarity = if name_contains || kind_contains || symbols_contain {
					// Ensure exact matches get higher priority
					0.9_f32.max(similarity)
				} else {
					similarity
				};

				similarities.push((boosted_similarity, node));
			}
		}

		// Sort by similarity (highest first)
		similarities.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

		// Return the nodes (without the similarity scores)
		let results = similarities.into_iter()
			.map(|(_, node)| node)
			.collect();

		Ok(results)
	}

	// Search for nodes in database
	async fn search_nodes_in_database(&self, query: &str) -> Result<Vec<CodeNode>> {
		// Generate an embedding for the query
		let query_embedding = self.generate_embedding(query).await?;

		// Check if the tables exist
		if !self.store.tables_exist(&["graphrag_nodes"]).await? {
			return Ok(Vec::new()); // No nodes in database
		}

		// Search for nodes similar to the query (increased limit to 50)
		let node_batch = self.store.search_graph_nodes(&query_embedding, 50).await?;
		if node_batch.num_rows() == 0 {
			return Ok(Vec::new());
		}

		// Process the results into CodeNode objects
		let mut nodes = Vec::new();
		let query_lower = query.to_lowercase();

		// Extract columns from the batch
		let id_array = node_batch.column_by_name("id").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let name_array = node_batch.column_by_name("name").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let kind_array = node_batch.column_by_name("kind").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let path_array = node_batch.column_by_name("path").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let description_array = node_batch.column_by_name("description").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let symbols_array = node_batch.column_by_name("symbols").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
		let hash_array = node_batch.column_by_name("hash").unwrap().as_any().downcast_ref::<arrow::array::StringArray>().unwrap();

		// Get the embedding fixed size list array
		let embedding_array = node_batch.column_by_name("embedding").unwrap()
			.as_any()
			.downcast_ref::<arrow::array::FixedSizeListArray>()
			.unwrap();

		// Get the values of the embedding array
		let embedding_values = embedding_array.values()
			.as_any()
			.downcast_ref::<arrow::array::Float32Array>()
			.unwrap();

		// Process each row
		for i in 0..node_batch.num_rows() {
			let id = id_array.value(i).to_string();
			let name = name_array.value(i).to_string();
			let kind = kind_array.value(i).to_string();
			let path = path_array.value(i).to_string();
			let description = description_array.value(i).to_string();

			// Parse symbols JSON
			let symbols: Vec<String> = if symbols_array.is_null(i) {
				Vec::new()
			} else {
				serde_json::from_str(symbols_array.value(i)).unwrap_or_default()
			};

			let hash = hash_array.value(i).to_string();

			// Extract the embedding for this node
			let embedding_offset = i * embedding_array.value_length() as usize;
			let embedding_len = embedding_array.value_length() as usize;
			let mut embedding = Vec::with_capacity(embedding_len);

			for j in 0..embedding_len {
				embedding.push(embedding_values.value(embedding_offset + j));
			}

			// Calculate semantic similarity
			let similarity = cosine_similarity(&query_embedding, &embedding);

			// Check if the query is a substring of various node fields
			let name_contains = name.to_lowercase().contains(&query_lower);
			let kind_contains = kind.to_lowercase().contains(&query_lower);
			let desc_contains = description.to_lowercase().contains(&query_lower);
			let symbols_contain = symbols.iter().any(|s| s.to_lowercase().contains(&query_lower));

			// Use a lower threshold for semantic similarity (0.5 instead of 0.6)
			// OR include if the query is a substring of any important field
			if similarity > 0.5 || name_contains || kind_contains || desc_contains || symbols_contain {
				// Create the node
				let node = CodeNode {
					id,
					name,
					kind,
					path,
					description,
					symbols,
					hash,
					embedding,
				};

				// Add to results
				nodes.push(node);
			}
		}

		// Sort nodes by relevance (exact matches first, then by similarity)
		nodes.sort_by(|a, b| {
			let a_contains = a.name.to_lowercase().contains(&query_lower) ||
			               a.kind.to_lowercase().contains(&query_lower) ||
			               a.symbols.iter().any(|s| s.to_lowercase().contains(&query_lower));

			let b_contains = b.name.to_lowercase().contains(&query_lower) ||
			               b.kind.to_lowercase().contains(&query_lower) ||
			               b.symbols.iter().any(|s| s.to_lowercase().contains(&query_lower));

			if a_contains && !b_contains {
				return std::cmp::Ordering::Less;
			} else if !a_contains && b_contains {
				return std::cmp::Ordering::Greater;
			} else {
				// Both contain or both don't contain, sort by similarity
				let a_sim = cosine_similarity(&query_embedding, &a.embedding);
				let b_sim = cosine_similarity(&query_embedding, &b.embedding);
				return b_sim.partial_cmp(&a_sim).unwrap_or(std::cmp::Ordering::Equal);
			}
		});

		Ok(nodes)
	}

	// Find paths between nodes in the graph
	pub async fn find_paths(&self, source_id: &str, target_id: &str, max_depth: usize) -> Result<Vec<Vec<String>>> {
		let graph = self.graph.read().await;

		// Ensure both nodes exist
		if !graph.nodes.contains_key(source_id) || !graph.nodes.contains_key(target_id) {
			return Ok(Vec::new());
		}

		// Build an adjacency list for easier traversal
		let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
		for rel in &graph.relationships {
			adjacency_list.entry(rel.source.clone())
				.or_insert_with(Vec::new)
				.push(rel.target.clone());
		}

		// Use BFS to find paths
		let mut queue = Vec::new();
		queue.push(vec![source_id.to_string()]);

		let mut paths = Vec::new();

		while let Some(path) = queue.pop() {
			let current = path.last().unwrap();

			// Found a path to target
			if current == target_id {
				paths.push(path);
				continue;
			}

			// Stop if we've reached max depth
			if path.len() > max_depth {
				continue;
			}

			// Explore neighbors
			if let Some(neighbors) = adjacency_list.get(current) {
				for neighbor in neighbors {
					// Avoid cycles
					if !path.contains(neighbor) {
						let mut new_path = path.clone();
						new_path.push(neighbor.clone());
						queue.push(new_path);
					}
				}
			}
		}

		Ok(paths)
	}
}

// Helper struct for parsing relationship analysis results
#[derive(Debug, Serialize, Deserialize)]
struct RelationshipResult {
	relation_type: String,
	description: String,
	confidence: f32,
	exists: bool,
}

// Calculate cosine similarity between two vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
	if a.len() != b.len() {
		return 0.0;
	}

	let mut dot_product = 0.0;
	let mut a_norm = 0.0;
	let mut b_norm = 0.0;

	for i in 0..a.len() {
		dot_product += a[i] * b[i];
		a_norm += a[i] * a[i];
		b_norm += b[i] * b[i];
	}

	a_norm = a_norm.sqrt();
	b_norm = b_norm.sqrt();

	if a_norm == 0.0 || b_norm == 0.0 {
		return 0.0;
	}

	dot_product / (a_norm * b_norm)
}

// Render GraphRAG nodes to JSON format
pub fn render_graphrag_nodes_json(nodes: &[CodeNode]) -> Result<(), anyhow::Error> {
	let json = serde_json::to_string_pretty(nodes)?;
	println!("{}", json);
	Ok(())
}

// Render GraphRAG nodes to Markdown format
pub fn graphrag_nodes_to_markdown(nodes: &[CodeNode]) -> String {
	let mut markdown = String::new();

	if nodes.is_empty() {
		markdown.push_str("No matching nodes found.");
		return markdown;
	}

	markdown.push_str(&format!("# Found {} GraphRAG nodes\n\n", nodes.len()));

	// Group nodes by file path for better organization
	let mut nodes_by_file: std::collections::HashMap<String, Vec<&CodeNode>> = std::collections::HashMap::new();

	for node in nodes {
		nodes_by_file
			.entry(node.path.clone())
			.or_insert_with(|| Vec::new())
			.push(node);
	}

	// Print results organized by file
	for (file_path, file_nodes) in nodes_by_file.iter() {
		markdown.push_str(&format!("## File: {}\n\n", file_path));

		for node in file_nodes {
			markdown.push_str(&format!("### {} `{}`\n", node.kind, node.name));
			markdown.push_str(&format!("**ID:** {}  \n", node.id));
			markdown.push_str(&format!("**Description:** {}  \n", node.description));

			if !node.symbols.is_empty() {
				markdown.push_str("**Symbols:**  \n");
				// Display symbols
				let mut display_symbols = node.symbols.clone();
				display_symbols.sort();
				display_symbols.dedup();

				for symbol in display_symbols {
					// Only show non-type symbols to users
					if !symbol.contains("_") {
						markdown.push_str(&format!("- `{}`  \n", symbol));
					}
				}
			}

			markdown.push_str("\n");
		}

		markdown.push_str("---\n\n");
	}

	markdown
}
