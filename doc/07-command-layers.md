# Command Layers in Octomind

Command layers are a powerful feature that allows you to define specialized AI helpers that can be invoked without affecting your session history. They use the same flexible layer infrastructure as the main processing pipeline but operate as standalone utilities.

## Key Benefits

- **Non-intrusive**: Commands don't affect your conversation history
- **Specialized**: Each command can have its own model, system prompt, and configuration
- **Flexible**: Use the same layer configuration system as regular layers
- **Cost-effective**: Only run when needed, with isolated token usage
- **Context-aware**: Proper input_mode handling for accessing session context

## Usage

Use the `/run` command followed by the command name:

```bash
/run estimate
/run query_processor
/run review
```

## Configuration

Command layers are defined in the `[commands]` section of your configuration file. They can be defined at different levels:

1. **Role-specific**: `[developer.commands.estimate]` or `[assistant.commands.summarize]`
2. **Global**: `[commands.help]` (fallback for any role)

### Basic Configuration

```toml
[developer.commands.estimate]
name = "estimate"
model = "openrouter:openai/gpt-4.1-mini"  # Optional - uses session model if not specified
system_prompt = "You are a project estimation expert..."
temperature = 0.2
input_mode = "last"  # "last", "all", or "summary" (case-insensitive)

[developer.commands.estimate.mcp]
server_refs = ["developer", "filesystem"]  # Reference servers from registry
allowed_tools = []  # Empty means all tools from referenced servers
```

### Available Input Modes

- **`last`** (case-insensitive): Gets the last assistant response from session context - perfect for analyzing previous AI responses
- **`all`**: Uses the entire conversation history with proper formatting
- **`summary`**: Uses a summarized version of the conversation to save tokens

**Note**: Input modes are now case-insensitive, so `"Last"`, `"last"`, `"LAST"` all work the same way.

### Smart Context Processing

The input_mode system now properly handles session context:

- **With `input_mode = "last"`**: If no explicit input is provided, the command gets the last assistant response. If explicit input is provided, it's combined with the previous assistant context.
- **With `input_mode = "all"`**: The command receives the full conversation context formatted for analysis.
- **With `input_mode = "summary"`**: The command gets a concise summary of the conversation.

### MCP (Tool) Integration

Commands can have their own tool configurations using the new server registry system:

```toml
[developer.commands.review.mcp]
server_refs = ["developer", "filesystem"]  # Reference servers by name
allowed_tools = ["text_editor", "shell"]   # Limit to specific tools
```

## Example Commands

### 1. Project Estimation

```toml
[developer.commands.estimate]
name = "estimate"
model = "openrouter:openai/gpt-4.1-mini"
system_prompt = """You are a project estimation expert. Analyze the work done or discussed and provide:

1. Time required for completion
2. Complexity assessment (1-5)
3. Potential challenges
4. Suggested next steps

Be specific and practical."""
temperature = 0.2
input_mode = "last"  # Gets the last assistant response for analysis

[developer.commands.estimate.mcp]
server_refs = []  # No tools needed for estimation
```

Usage: `/run estimate`

### 2. Code Review

```toml
[developer.commands.review]
name = "review"
model = "openrouter:anthropic/claude-3.5-sonnet"
system_prompt = """You are a code review expert. Analyze recent work and provide:

1. Code quality assessment
2. Potential improvements
3. Best practices recommendations
4. Security considerations

Focus on constructive feedback."""
temperature = 0.1
input_mode = "all"  # Gets full conversation context

[developer.commands.review.mcp]
server_refs = ["developer", "filesystem"]  # Access to code files and shell
allowed_tools = ["text_editor", "list_files", "shell"]
```

Usage: `/run review`

### 3. Quick Summary

```toml
[developer.commands.summarize]
name = "summarize"
model = "openrouter:openai/gpt-4.1-nano"  # Fast, cheap model
system_prompt = "Provide a concise summary of the conversation and key points."
temperature = 0.2
input_mode = "summary"  # Uses pre-summarized content to save tokens

[developer.commands.summarize.mcp]
server_refs = []  # No tools needed
```

Usage: `/run summarize`

## Advanced Features

### Parameters and Placeholders

Command layers support custom parameters that can be used in system prompts:

```toml
[developer.commands.estimate]
name = "estimate"
system_prompt = "You estimate projects for %{team_size} person team with %{project_type} focus."

[developer.commands.estimate.parameters]
team_size = "3"
project_type = "web development"
```

### Multiple Models

Different commands can use different models optimized for their specific tasks:

```toml
[developer.commands.quick_check]
model = "openrouter:openai/gpt-4.1-nano"  # Fast, cheap model

[developer.commands.deep_analysis]
model = "openrouter:anthropic/claude-sonnet-4"  # Powerful model
```

## Command Discovery

- **List available commands**: `/run` (without parameters)
- **Help system**: `/help` shows available commands for your role
- **Configuration check**: Commands are validated at startup

## Differences from Regular Layers

| Feature | Regular Layers | Command Layers |
|---------|---------------|----------------|
| **Execution** | Automatic on first message | Manual via `/run` |
| **History** | Affects session context | Isolated execution |
| **Cost** | Part of main flow | Separate, tracked |
| **Usage** | Pipeline processing | On-demand helpers |
| **Configuration** | `[[layers]]` section | `[commands]` section |

## Best Practices

1. **Keep commands focused**: Each command should have a specific purpose
2. **Use appropriate models**: Match model capabilities to command complexity
3. **Optimize input modes**: Use "Last" for quick commands, "All" for analysis
4. **Enable tools selectively**: Only give commands the tools they need
5. **Document your commands**: Use clear names and system prompts

## Troubleshooting

### Command Not Found
```
Command 'estimate' not found in configuration
```
- Check that the command is defined in your role's commands section
- Verify proper TOML syntax in your configuration
- Use `/run` without parameters to see available commands

### Tool Execution Errors
```
Tool execution failed: Unknown tool 'list_files'. Available tools: search_code, memorize, remember, forget
```
- **Tool routing issue**: The tool is being sent to the wrong server
- **Solution**: Check your MCP server configuration and ensure proper server references
- **Verify**: Tools are being routed to the correct server type (filesystem tools → filesystem server)

### Input Mode Issues
```
Unknown input mode: 'Last'. Valid options: last, all, summary
```
- **Case sensitivity**: Input modes are now case-insensitive but should use lowercase
- **Solution**: Use `input_mode = "last"` instead of `input_mode = "Last"`
- **Valid values**: `"last"`, `"all"`, `"summary"` (any case)

### No Commands Available
```
No command layers configured for this role.
```
- Add command definitions to your configuration file
- Use `/run` without parameters to see configuration examples
- Check role-specific command sections (e.g., `[developer.commands.estimate]`)

### MCP Server Configuration Issues
```
Failed to execute tool 'shell': No servers available to process tool
```
- **Missing server references**: Check `server_refs` in your command's MCP configuration
- **Server configuration**: Ensure servers are defined in `[[mcp.servers]]` array
- **Tool mapping**: Verify tools are available on the referenced servers

### Context Processing Problems
```
No previous messages found
```
- **Input mode mismatch**: Using `input_mode = "last"` but no assistant messages in session
- **Solution**: Either provide explicit input or use `input_mode = "all"`
- **Check**: Session history has the expected message types

## Migration from /done

The existing `/done` command continues to work as before. Command layers provide additional functionality without replacing the context reduction system.

## Complete Example

See `doc/examples/command_layers_config.toml` for a comprehensive configuration example with multiple command types.
