# Advanced Features Guide

## Overview

OctoDev includes several advanced features that provide deep insights into your codebase and enable sophisticated AI assistance. These features include GraphRAG for code relationship analysis, MCP protocol for tool integration, and advanced layered architecture for complex reasoning.

## GraphRAG (Graph-based Retrieval Augmented Generation)

### What is GraphRAG?

GraphRAG creates a knowledge graph of your codebase by:
1. **Analyzing code relationships** using AI
2. **Creating nodes** for functions, classes, and modules
3. **Establishing relationships** between code entities
4. **Providing graph-based search** and traversal

### Enabling GraphRAG

```toml
[graphrag]
enabled = true
description_model = "openrouter:openai/gpt-4.1-nano"
relationship_model = "openrouter:openai/gpt-4.1-nano"
```

```bash
# Enable GraphRAG
octodev config --graphrag-enable true

# Index with GraphRAG
octodev index
```

### GraphRAG Commands

```bash
# Search for nodes
octodev graphrag search "authentication functions"

# Get node details
octodev graphrag node "UserAuth::login"

# Find relationships
octodev graphrag relationships "UserAuth::login"

# Find paths between nodes
octodev graphrag path "UserAuth::login" "Database::connect"

# Graph overview
octodev graphrag overview
```

### Using GraphRAG in Sessions

GraphRAG is available as a tool in interactive sessions:

```bash
> "Show me the relationships around the authentication system"

AI uses: graphrag
Parameters: {"operation": "search", "query": "authentication system"}

> "How is the login function connected to the database?"

AI uses: graphrag  
Parameters: {"operation": "path", "source_id": "login_func", "target_id": "db_connect"}
```

### GraphRAG Data Structure

#### Nodes
```json
{
  "id": "user_auth_login",
  "name": "login",
  "kind": "function", 
  "path": "src/auth.rs",
  "description": "Authenticates user credentials and returns session token",
  "embedding": [0.1, 0.2, ...]
}
```

#### Relationships
```json
{
  "source": "user_auth_login",
  "target": "database_query",
  "type": "calls",
  "description": "Login function calls database query to verify credentials"
}
```

### Relationship Types

GraphRAG automatically detects various relationship types:
- **calls**: Function A calls function B
- **inherits**: Class A inherits from class B
- **implements**: Class implements interface
- **uses**: Module/function uses another component
- **depends_on**: Component depends on another
- **contains**: Module contains function/class

## MCP (Model-Centric Programming) Protocol

### What is MCP?

MCP enables AI models to use external tools and services through a standardized protocol. OctoDev supports both local tools and external MCP servers.

### Core MCP Tools

#### Development Tools
- **shell**: Execute terminal commands
- **text_editor**: Read, write, and edit files
- **list_files**: Browse directory structures
- **semantic_code**: Search and analyze code

#### Search Tools
- **code_search**: Semantic search in code blocks
- **docs_search**: Search documentation
- **text_search**: Search text files
- **graphrag**: Query the code knowledge graph

### MCP Configuration

#### Basic Configuration
```toml
[mcp]
enabled = true
providers = ["core"]
servers = []
```

#### Advanced Configuration
```toml
[mcp]
enabled = true
providers = ["core", "filesystem"]

# External MCP server (remote)
[[mcp.servers]]
enabled = true
name = "WebSearch"
url = "https://mcp.so/server/webSearch-Tools"
auth_token = "optional_token"
tools = []  # Empty = all tools enabled

# Local MCP server
[[mcp.servers]]
enabled = true
name = "LocalTools"
command = "python"
args = ["-m", "my_mcp_server", "--port", "8008"]
mode = "http"
timeout = 30
```

### Creating Custom MCP Servers

#### Simple Python MCP Server
```python
#!/usr/bin/env python3
"""
Simple MCP server example
"""
import json
import sys
from typing import Dict, Any

def handle_list_tools():
    """Return available tools"""
    return {
        "tools": [
            {
                "name": "custom_search",
                "description": "Custom search functionality",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "scope": {"type": "string", "enum": ["local", "web"]}
                    },
                    "required": ["query"]
                }
            }
        ]
    }

def handle_call_tool(name: str, arguments: Dict[str, Any]):
    """Handle tool execution"""
    if name == "custom_search":
        query = arguments.get("query", "")
        scope = arguments.get("scope", "local")
        
        # Implement your custom search logic
        results = f"Search results for '{query}' in {scope} scope"
        
        return {
            "content": [
                {
                    "type": "text",
                    "text": results
                }
            ]
        }
    
    return {"error": f"Unknown tool: {name}"}

def main():
    """Main server loop"""
    for line in sys.stdin:
        try:
            request = json.loads(line.strip())
            method = request.get("method")
            
            if method == "tools/list":
                response = handle_list_tools()
            elif method == "tools/call":
                params = request.get("params", {})
                name = params.get("name")
                arguments = params.get("arguments", {})
                response = handle_call_tool(name, arguments)
            else:
                response = {"error": f"Unknown method: {method}"}
            
            # Send response
            response["id"] = request.get("id")
            print(json.dumps(response))
            
        except Exception as e:
            error_response = {
                "id": request.get("id"),
                "error": str(e)
            }
            print(json.dumps(error_response))

if __name__ == "__main__":
    main()
```

#### Registering Custom Server
```toml
[[mcp.servers]]
enabled = true
name = "CustomSearch"
command = "python"
args = ["/path/to/custom_mcp_server.py"]
mode = "stdin"
```

### Tool Error Handling

MCP includes sophisticated error handling:
- **Retry logic**: Automatic retries for transient errors
- **Error tracking**: Per-tool error counters
- **Fallback mechanisms**: Alternative tools when primary fails
- **User warnings**: Notifications for repeated failures

## Layered Architecture Deep Dive

### Architecture Philosophy

The layered architecture breaks complex AI tasks into specialized stages:

```mermaid
graph TB
    A[User Input] --> B[Query Processor]
    B --> C[Context Generator] 
    C --> D[Developer]
    D --> E[Final Response]
    
    F[/done Command] --> G[Reducer]
    G --> H[Optimized Context]
    H --> I[Next Interaction]
```

### Layer Responsibilities

#### Query Processor Layer
- **Purpose**: Analyze and improve user requests
- **Tools**: None (pure analysis)
- **Output**: Clarified, actionable instructions

```toml
[[layers]]
name = "query_processor"
model = "openrouter:openai/gpt-4.1-nano"
enable_tools = false
temperature = 0.1
system_prompt = "You analyze user requests and make them clearer and more actionable."
```

#### Context Generator Layer
- **Purpose**: Gather necessary information using tools
- **Tools**: Limited set (search, file reading)
- **Output**: Relevant code, documentation, and context

```toml
[[layers]]
name = "context_generator"
model = "openrouter:google/gemini-1.5-flash"
enable_tools = true
allowed_tools = ["core", "text_editor", "semantic_code"]
temperature = 0.2
```

#### Developer Layer
- **Purpose**: Execute development tasks and provide solutions
- **Tools**: Full access to all available tools
- **Output**: Complete response with code changes, explanations

```toml
[[layers]]
name = "developer"
model = "openrouter:anthropic/claude-sonnet-4"
enable_tools = true
temperature = 0.3
input_mode = "All"  # Uses context from all previous layers
```

#### Reducer Layer (Optional)
- **Purpose**: Optimize context for future interactions
- **Tools**: None (pure optimization)
- **Triggered**: Manually with `/done` command

### Input Modes Explained

#### Last Mode
```toml
input_mode = "Last"
```
- Only receives output from the immediately previous layer
- Keeps context focused and manageable
- Best for sequential processing

#### All Mode
```toml
input_mode = "All"
```
- Receives all context from previous layers
- Provides complete picture
- Used by Developer layer for comprehensive understanding

#### Summary Mode
```toml
input_mode = "Summary"
```
- Receives summarized version of all previous context
- Balances completeness with token efficiency
- Useful for final processing stages

### Custom Layer Development

#### Creating Specialized Layers
```toml
[[layers]]
name = "security_analyzer"
enabled = true
model = "openrouter:anthropic/claude-3.5-sonnet"
temperature = 0.1
enable_tools = true
allowed_tools = ["semantic_code", "text_editor"]
input_mode = "All"
system_prompt = """You are a security expert. Analyze code for:
1. Security vulnerabilities
2. Authentication issues  
3. Data validation problems
4. Injection attack vectors"""

[[layers]]
name = "performance_optimizer"
enabled = true
model = "openrouter:openai/gpt-4o"
temperature = 0.2
enable_tools = true
allowed_tools = ["semantic_code", "shell"]
input_mode = "Last"
system_prompt = """You optimize code performance by:
1. Identifying bottlenecks
2. Suggesting algorithmic improvements
3. Recommending profiling tools
4. Analyzing resource usage"""
```

## Advanced Configuration Patterns

### Multi-Provider Layer Setup
```toml
# Use different providers for different layers
[[layers]]
name = "query_processor"
model = "google:gemini-1.5-flash"  # Fast and cheap

[[layers]]
name = "context_generator"  
model = "openai:gpt-4o-mini"  # Good balance

[[layers]]
name = "developer"
model = "anthropic:claude-3-5-sonnet"  # Best reasoning
```

### Environment-Specific Configuration
```toml
# Development environment
[dev.openrouter]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true

# Production environment  
[prod.openrouter]
model = "openrouter:anthropic/claude-3.5-haiku"
enable_layers = false
```

### Dynamic Tool Access
```toml
# Tools based on project type
[web_project.mcp]
providers = ["core", "web_tools", "testing"]

[ml_project.mcp]
providers = ["core", "data_tools", "notebook"]

[systems_project.mcp]
providers = ["core", "system_tools", "monitoring"]
```

## Performance Optimization

### Layer Performance Tuning

#### Model Selection Strategy
1. **Fast models** for simple processing (Query Processor)
2. **Balanced models** for information gathering (Context Generator)
3. **Powerful models** for complex reasoning (Developer)

#### Token Optimization
```toml
[openrouter]
# Automatic context management
cache_tokens_pct_threshold = 40
max_request_tokens_threshold = 50000
enable_auto_truncation = true

# Layer-specific token limits
query_processor_max_tokens = 1000
context_generator_max_tokens = 5000
developer_max_tokens = 20000
```

### GraphRAG Performance

#### Batch Processing
- **Node creation**: 5 nodes per API call
- **Relationship analysis**: 3 pairs per call
- **Incremental updates**: Only process changed files

#### Embedding Optimization
```toml
[graphrag]
# Use faster models for large codebases
description_model = "openrouter:openai/gpt-4o-mini"
relationship_model = "openrouter:openai/gpt-4o-mini"

# Batch sizes
node_batch_size = 5
relationship_batch_size = 3
```

## Troubleshooting Advanced Features

### GraphRAG Issues

#### Large Memory Usage
```bash
# Monitor GraphRAG memory usage
octodev graphrag overview

# Clear and rebuild graph
octodev clear
octodev index
```

#### Poor Relationship Quality
```toml
# Use better models for relationship analysis
[graphrag]
relationship_model = "openrouter:anthropic/claude-3.5-sonnet"
```

### MCP Issues

#### Tool Timeout
```toml
[[mcp.servers]]
timeout = 60  # Increase timeout for slow tools
```

#### Server Connection Issues
```bash
# Test MCP server directly
curl -X POST http://localhost:8008/tools/list

# Check server logs
tail -f .octodev/logs/mcp_server.log
```

### Layer Issues

#### Token Limit Exceeded
```bash
# Use /done to optimize context
/done

# Enable auto-truncation
/truncate 30000
```

#### Layer Performance
```bash
# Monitor layer performance
/info

# Disable expensive layers temporarily
/layers
```

## Best Practices

### GraphRAG
1. **Enable for large codebases** where relationships are complex
2. **Use efficient models** for cost control
3. **Regular rebuilds** for accuracy
4. **Monitor storage usage** as graphs can be large

### MCP Protocol
1. **Start with core tools** then add specialized ones
2. **Test custom servers** thoroughly before deployment
3. **Monitor tool performance** and error rates
4. **Use appropriate timeouts** for different tool types

### Layered Architecture
1. **Design layers** with clear responsibilities
2. **Use appropriate models** for each layer's complexity
3. **Monitor token usage** across layers
4. **Optimize input modes** for efficiency
5. **Test layer interactions** to ensure smooth flow