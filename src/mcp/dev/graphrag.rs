// GraphRAG functionality for the Developer MCP provider

use serde_json::{json, Value};
use anyhow::{Result, anyhow};
use super::super::{McpToolCall, McpToolResult, McpFunction};

// Define the GraphRAG function
pub fn get_graphrag_function() -> McpFunction {
	McpFunction {
		name: "graphrag".to_string(),
		description: "Query and explore the code relationship graph (GraphRAG) built during indexing.

This tool allows you to explore the code knowledge graph that was built during indexing, which contains
code entities and their relationships. This semantic graph helps in understanding complex codebases and
finding connections between different parts of the code.

Operations:
- `search`: Find code nodes that match a semantic query
- Use with `task_focused: true` for an optimized, token-efficient view focused on your specific task
- `get_node`: Get detailed information about a specific node by ID
- `get_relationships`: Find relationships involving a specific node
- `find_path`: Find paths between two nodes in the graph
- `overview`: Get an overview of the entire graph structure

Use this tool to understand how different parts of the code are related and to explore the codebase
from a structural perspective.".to_string(),
		parameters: json!({
			"type": "object",
			"required": ["operation"],
			"properties": {
				"operation": {
					"type": "string",
					"enum": ["search", "get_node", "get_relationships", "find_path", "overview"],
					"description": "The GraphRAG operation to perform"
				},
				"query": {
					"type": "string",
					"description": "[For search operation] The semantic query to search for"
				},
				"task_focused": {
					"type": "boolean",
					"description": "[For search operation] Whether to use task-focused optimization to provide a more concise, relevant view",
					"default": false
				},
				"node_id": {
					"type": "string",
					"description": "[For get_node/get_relationships operations] The ID of the node to get information about"
				},
				"source_id": {
					"type": "string",
					"description": "[For find_path operation] The ID of the source node"
				},
				"target_id": {
					"type": "string",
					"description": "[For find_path operation] The ID of the target node"
				},
				"max_depth": {
					"type": "integer",
					"description": "[For find_path operation] The maximum path length to consider",
					"default": 3
				}
			}
		}),
	}
}

// Execute GraphRAG operations
pub async fn execute_graphrag(call: &McpToolCall, config: &crate::config::Config) -> Result<McpToolResult> {
	// Extract operation parameter
	let operation = match call.parameters.get("operation") {
		Some(Value::String(op)) => op.as_str(),
		_ => return Err(anyhow!("Missing or invalid 'operation' parameter")),
	};

	// Check if GraphRAG is enabled
	if !config.graphrag.enabled {
		return Ok(McpToolResult {
			tool_name: "graphrag".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"success": false,
				"error": "GraphRAG is not enabled. Enable it in your configuration and run 'octodev index' to build the knowledge graph.",
				"message": "To enable GraphRAG, add the following to your .octodev/config.toml file:\n\n[graphrag]\nenabled = true\n\nThen run 'octodev index' to build the knowledge graph."
			}),
		});
	}

	// Initialize the GraphBuilder
	let graph_builder = match crate::indexer::GraphBuilder::new(config.clone()).await {
		Ok(builder) => builder,
		Err(e) => return Err(anyhow!("Failed to initialize GraphBuilder: {}", e)),
	};

	// Execute the requested operation
	match operation {
		"search" => execute_graphrag_search(call, &graph_builder).await,
		"get_node" => execute_graphrag_get_node(call, &graph_builder).await,
		"get_relationships" => execute_graphrag_get_relationships(call, &graph_builder).await,
		"find_path" => execute_graphrag_find_path(call, &graph_builder).await,
		"overview" => execute_graphrag_overview(call, &graph_builder).await,
		_ => Err(anyhow!("Invalid operation: {}", operation)),
	}
}

// Search for nodes in the graph
async fn execute_graphrag_search(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract query parameter
	let query = match call.parameters.get("query") {
		Some(Value::String(q)) => q.clone(),
		_ => return Err(anyhow!("Missing or invalid 'query' parameter for search operation")),
	};

	// Check for task-focused flag
	let task_focused = call.parameters.get("task_focused")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	if task_focused {
		// Use the graph optimizer for task-focused search
		let store = crate::store::Store::new().await?;
		let config = crate::config::Config::load().unwrap_or_default();

		// Get the full graph
		let full_graph = graph_builder.get_graph().await?;

		// Generate embeddings for the query
		let query_embedding = crate::indexer::generate_embeddings(&query, false, &config).await?;

		// Create optimizer with token budget
		let optimizer = crate::indexer::graph_optimization::GraphOptimizer::new(2000);

		// Get all code blocks
		let code_blocks = store.get_code_blocks(query_embedding.clone()).await?;

		// Generate a task-focused view
		let task_view = optimizer.generate_task_focused_view(
			&query,
			&query_embedding,
			&full_graph,
			&code_blocks
		).await?;

		// Return the optimized view
		return Ok(McpToolResult {
			tool_name: "graphrag".to_string(),
			tool_id: call.tool_id.clone(),
			result: json!({
				"success": true,
				"output": task_view,
				"task_focused": true,
				"parameters": {
					"operation": "search",
					"query": query,
					"task_focused": true
				}
			}),
		});
	}

	// Traditional node search (without task focusing)
	let nodes = graph_builder.search_nodes(&query).await?;

	// Format the results as markdown using the official formatter
	let markdown = crate::indexer::graphrag::graphrag_nodes_to_markdown(&nodes);

	// Return the results
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"count": nodes.len(),
			"nodes": nodes,
			"parameters": {
				"operation": "search",
				"query": query
			}
		}),
	})
}

// Get details about a specific node
async fn execute_graphrag_get_node(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract node_id parameter
	let node_id = match call.parameters.get("node_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'node_id' parameter for get_node operation")),
	};

	// Get the graph
	let graph = graph_builder.get_graph().await?;

	// Check if the node exists
	let node = match graph.nodes.get(&node_id) {
		Some(node) => node.clone(),
		None => return Err(anyhow!("Node not found: {}", node_id)),
	};

	// Format the result as markdown
	let mut markdown = format!("# Node: {}\n\n", node.name);
	markdown.push_str(&format!("**ID**: {}\n", node.id));
	markdown.push_str(&format!("**Kind**: {}\n", node.kind));
	markdown.push_str(&format!("**Path**: {}\n", node.path));
	markdown.push_str(&format!("**Description**: {}\n\n", node.description));

	// Add symbols if any
	if !node.symbols.is_empty() {
		markdown.push_str("## Symbols\n\n");
		for symbol in &node.symbols {
			markdown.push_str(&format!("- `{}`\n", symbol));
		}
		markdown.push('\n');
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"node": node,
			"parameters": {
				"operation": "get_node",
				"node_id": node_id
			}
		}),
	})
}

// Get relationships involving a specific node
async fn execute_graphrag_get_relationships(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract node_id parameter
	let node_id = match call.parameters.get("node_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'node_id' parameter for get_relationships operation")),
	};

	// Get the graph
	let graph = graph_builder.get_graph().await?;

	// Check if the node exists
	if !graph.nodes.contains_key(&node_id) {
		return Err(anyhow!("Node not found: {}", node_id));
	}

	// Find relationships where this node is either source or target
	let relationships: Vec<_> = graph.relationships.iter()
		.filter(|rel| rel.source == node_id || rel.target == node_id)
		.cloned()
		.collect();

	// Format the result as markdown
	let mut markdown = format!("# Relationships for Node: {}\n\n", node_id);

	if relationships.is_empty() {
		markdown.push_str("No relationships found for this node.\n");
	} else {
		markdown.push_str(&format!("Found {} relationships:\n\n", relationships.len()));

		// Outgoing relationships
		let outgoing: Vec<_> = relationships.iter()
			.filter(|rel| rel.source == node_id)
			.collect();

		if !outgoing.is_empty() {
			markdown.push_str("## Outgoing Relationships\n\n");
			for rel in outgoing {
				let target_name = graph.nodes.get(&rel.target)
					.map(|n| n.name.clone())
					.unwrap_or_else(|| rel.target.clone());

				markdown.push_str(&format!("- **{}** → {} ({}): {}\n",
					rel.relation_type,
					target_name,
					rel.target,
					rel.description));
			}
			markdown.push('\n');
		}

		// Incoming relationships
		let incoming: Vec<_> = relationships.iter()
			.filter(|rel| rel.target == node_id)
			.collect();

		if !incoming.is_empty() {
			markdown.push_str("## Incoming Relationships\n\n");
			for rel in incoming {
				let source_name = graph.nodes.get(&rel.source)
					.map(|n| n.name.clone())
					.unwrap_or_else(|| rel.source.clone());

				markdown.push_str(&format!("- **{}** ← {} ({}): {}\n",
					rel.relation_type,
					source_name,
					rel.source,
					rel.description));
			}
			markdown.push('\n');
		}
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"relationships": relationships,
			"count": relationships.len(),
			"parameters": {
				"operation": "get_relationships",
				"node_id": node_id
			}
		}),
	})
}

// Find paths between two nodes
async fn execute_graphrag_find_path(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Extract parameters
	let source_id = match call.parameters.get("source_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'source_id' parameter for find_path operation")),
	};

	let target_id = match call.parameters.get("target_id") {
		Some(Value::String(id)) => id.clone(),
		_ => return Err(anyhow!("Missing or invalid 'target_id' parameter for find_path operation")),
	};

	let max_depth = match call.parameters.get("max_depth") {
		Some(Value::Number(n)) => n.as_u64().unwrap_or(3) as usize,
		_ => 3, // Default to depth 3
	};

	// Find paths
	let paths = graph_builder.find_paths(&source_id, &target_id, max_depth).await?;

	// Get the graph for node name lookup
	let graph = graph_builder.get_graph().await?;

	// Format the result as markdown
	let mut markdown = format!("# Paths from '{}' to '{}'\n\n", source_id, target_id);

	if paths.is_empty() {
		markdown.push_str("No paths found between these nodes within the specified depth.\n");
	} else {
		markdown.push_str(&format!("Found {} paths with max depth {}:\n\n", paths.len(), max_depth));

		for (i, path) in paths.iter().enumerate() {
			markdown.push_str(&format!("## Path {}\n\n", i + 1));

			// Display each node in the path
			for (j, node_id) in path.iter().enumerate() {
				let node_name = graph.nodes.get(node_id)
					.map(|n| n.name.clone())
					.unwrap_or_else(|| node_id.clone());

				if j > 0 {
					// Look up the relationship
					let prev_id = &path[j-1];
					let rel = graph.relationships.iter()
						.find(|r| r.source == *prev_id && r.target == *node_id);

					if let Some(rel) = rel {
						markdown.push_str(&format!("→ **{}** → ", rel.relation_type));
					} else {
						markdown.push_str("→ ");
					}
				}

				markdown.push_str(&format!("`{}` ({})", node_name, node_id));
			}
			markdown.push_str("\n\n");
		}
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"paths": paths,
			"count": paths.len(),
			"parameters": {
				"operation": "find_path",
				"source_id": source_id,
				"target_id": target_id,
				"max_depth": max_depth
			}
		}),
	})
}

// Get an overview of the graph
async fn execute_graphrag_overview(call: &McpToolCall, graph_builder: &crate::indexer::GraphBuilder) -> Result<McpToolResult> {
	// Get the graph
	let graph = graph_builder.get_graph().await?;

	// Get statistics
	let node_count = graph.nodes.len();
	let relationship_count = graph.relationships.len();

	// Count node types
	let mut node_types = std::collections::HashMap::new();
	for node in graph.nodes.values() {
		*node_types.entry(node.kind.clone()).or_insert(0) += 1;
	}

	// Count relationship types
	let mut rel_types = std::collections::HashMap::new();
	for rel in &graph.relationships {
		*rel_types.entry(rel.relation_type.clone()).or_insert(0) += 1;
	}

	// Format the result as markdown
	let mut markdown = String::from("# GraphRAG Knowledge Graph Overview\n\n");
	markdown.push_str(&format!("The knowledge graph contains {} nodes and {} relationships.\n\n", node_count, relationship_count));

	// Node type statistics
	markdown.push_str("## Node Types\n\n");
	for (kind, count) in node_types.iter() {
		markdown.push_str(&format!("- {}: {} nodes\n", kind, count));
	}
	markdown.push('\n');

	// Relationship type statistics
	markdown.push_str("## Relationship Types\n\n");
	for (rel_type, count) in rel_types.iter() {
		markdown.push_str(&format!("- {}: {} relationships\n", rel_type, count));
	}

	// Return the result
	Ok(McpToolResult {
		tool_name: "graphrag".to_string(),
		tool_id: call.tool_id.clone(),
		result: json!({
			"success": true,
			"output": markdown,
			"node_count": node_count,
			"relationship_count": relationship_count,
			"node_types": node_types,
			"relationship_types": rel_types,
			"parameters": {
				"operation": "overview"
			}
		}),
	})
}