// GraphRAG module for OctoDev
// Handles code relationship extraction and graph generation

use crate::config::Config;
use crate::store::CodeBlock;
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

// A prompt template for extracting relationships
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

// Manages the creation and storage of the code graph
pub struct GraphBuilder {
    config: Config,
    graph: Arc<RwLock<CodeGraph>>,
    client: Client,
    embedding_model: Arc<TextEmbedding>,
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
        
        // Load existing graph if available
        let graph = Arc::new(RwLock::new(Self::load_graph().await?));
        
        Ok(Self {
            config,
            graph,
            client: Client::new(),
            embedding_model: Arc::new(model),
        })
    }
    
    // Load the existing graph from disk or create a new one
    async fn load_graph() -> Result<CodeGraph> {
        let graph_path = Path::new(".octodev/graph.json");
        if graph_path.exists() {
            let graph_data = fs::read_to_string(graph_path)?;
            let graph = serde_json::from_str(&graph_data)?;
            Ok(graph)
        } else {
            Ok(CodeGraph::default())
        }
    }
    
    // Save the graph to disk
    async fn save_graph(&self) -> Result<()> {
        let graph = self.graph.read().await;
        let graph_path = Path::new(".octodev/graph.json");
        let graph_data = serde_json::to_string_pretty(&*graph)?;
        fs::write(graph_path, graph_data)?;
        Ok(())
    }

    // Process a batch of code blocks and update the graph
    pub async fn process_code_blocks(&self, code_blocks: &[CodeBlock]) -> Result<()> {
        // Create nodes for all code blocks
        let mut new_nodes = Vec::new();
        
        for block in code_blocks {
            if block.symbols.is_empty() {
                continue; // Skip blocks without symbols
            }

            // Generate a unique ID for this node
            let node_name = self.extract_node_name(block);
            let node_id = format!("{}/{}", block.path, node_name);
            
            // Check if we already have this node with the same hash
            let mut graph = self.graph.write().await;
            if let Some(existing_node) = graph.nodes.get(&node_id) {
                if existing_node.hash == block.hash {
                    continue; // Skip unchanged nodes
                }
            }
            
            // Extract a description using an LLM
            let description = self.extract_description(&block.content).await?;
            
            // Generate embedding for the node
            let embedding = self.generate_embedding(&format!("{} {}", node_name, description)).await?;
            
            // Create the node
            let node = CodeNode {
                id: node_id.clone(),
                name: node_name,
                kind: self.determine_node_kind(block),
                path: block.path.clone(),
                description,
                symbols: block.symbols.clone(),
                hash: block.hash.clone(),
                embedding,
            };
            
            // Add the node to the graph
            graph.nodes.insert(node_id, node.clone());
            new_nodes.push(node);
        }
        
        // Drop the write lock before discovering relationships
        drop(self.graph.write().await);
        
        // Discover relationships between nodes
        if !new_nodes.is_empty() {
            self.discover_relationships(&new_nodes).await?;
        }
        
        // Save the updated graph
        self.save_graph().await?;
        
        Ok(())
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
        let truncated_code = if code.len() > 1000 {
            format!("{} [...]\n(code truncated due to length)", &code[0..1000])
        } else {
            code.to_string()
        };
        
        // Use an inexpensive LLM to generate the description
        match self.call_llm(
            &self.config.graphrag.description_model,
            DESCRIPTION_PROMPT.replace("{code}", &truncated_code),
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
    async fn discover_relationships(&self, new_nodes: &[CodeNode]) -> Result<()> {
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
        
        // Call the relationship detection model
        match self.call_llm(
            &self.config.graphrag.relationship_model,
            prompt,
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
    async fn call_llm(&self, model_name: &str, prompt: String) -> Result<String> {
        // Check if we have an API key configured
        let api_key = match &self.config.openrouter.api_key {
            Some(key) => key.clone(),
            None => return Err(anyhow::anyhow!("OpenRouter API key not configured")),
        };
        
        // Call OpenRouter API
        let response = self.client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&json!({
                "model": model_name,
                "messages": [{
                    "role": "user",
                    "content": prompt
                }],
                "max_tokens": 200
            }))
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
        // Generate an embedding for the query
        let query_embedding = self.generate_embedding(query).await?;
        
        // Find similar nodes
        let graph = self.graph.read().await;
        let nodes_array = graph.nodes.values().cloned().collect::<Vec<CodeNode>>();
        drop(graph);
        
        // Calculate similarity to each node
        let mut similarities: Vec<(f32, CodeNode)> = Vec::new();
        for node in nodes_array {
            let similarity = cosine_similarity(&query_embedding, &node.embedding);
            // Only include reasonably similar nodes (threshold 0.6)
            if similarity > 0.6 {
                similarities.push((similarity, node));
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