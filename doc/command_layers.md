# Command Layers in Octodev

Command layers are a powerful feature that allows you to define specialized AI helpers that can be invoked without affecting your session history. They use the same flexible layer infrastructure as the main processing pipeline but operate as standalone utilities.

## Key Benefits

- **Non-intrusive**: Commands don't affect your conversation history
- **Specialized**: Each command can have its own model, system prompt, and configuration
- **Flexible**: Use the same layer configuration system as regular layers
- **Cost-effective**: Only run when needed, with isolated token usage

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
enabled = true
model = "openrouter:openai/gpt-4.1-mini"  # Optional - uses session model if not specified
system_prompt = "You are a project estimation expert..."
temperature = 0.2
input_mode = "Last"  # "Last", "All", or "Summary"

[developer.commands.estimate.mcp]
enabled = false  # Can enable tools for specific commands
```

### Available Input Modes

- **`Last`**: Uses the last user message as input
- **`All`**: Uses the entire conversation history
- **`Summary`**: Uses a summarized version of the conversation

### MCP (Tool) Integration

Commands can have their own tool configurations:

```toml
[developer.commands.review.mcp]
enabled = true
servers = ["core"]
allowed_tools = ["text_editor", "semantic_code"]
```

## Example Commands

### 1. Project Estimation

```toml
[developer.commands.estimate]
name = "estimate"
enabled = true
model = "openrouter:openai/gpt-4.1-mini"
system_prompt = """You are a project estimation expert. Analyze the work done or discussed and provide:

1. Time required for completion
2. Complexity assessment (1-5)
3. Potential challenges
4. Suggested next steps

Be specific and practical."""
temperature = 0.2
input_mode = "Last"
```

Usage: `/run estimate`

### 2. Code Review

```toml
[developer.commands.review]
name = "review"
enabled = true
model = "openrouter:anthropic/claude-3.5-sonnet"
system_prompt = """You are a code review expert. Analyze recent work and provide:

1. Code quality assessment
2. Potential improvements
3. Best practices recommendations
4. Security considerations

Focus on constructive feedback."""
temperature = 0.1
input_mode = "All"

[developer.commands.review.mcp]
enabled = true
servers = ["core"]
allowed_tools = ["text_editor", "semantic_code"]
```

Usage: `/run review`

### 3. Query Processor (System Layer Alternative)

```toml
[developer.commands.query_processor]
name = "query_processor"
enabled = true
# No model specified - uses session model
system_prompt = "You are a query processor that improves and clarifies user requests."
temperature = 0.2
input_mode = "Last"
```

Usage: `/run query_processor`

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
- Verify the command is `enabled = true`
- Ensure proper TOML syntax

### No Commands Available
```
No command layers configured for this role.
```
- Add command definitions to your configuration file
- Use `/run` without parameters to see configuration examples

### Permission Errors
```
Tool 'text_editor' is not allowed for this layer
```
- Check the `allowed_tools` list in the command's MCP configuration
- Ensure the required MCP servers are enabled

## Migration from /done

The existing `/done` command continues to work as before. Command layers provide additional functionality without replacing the context reduction system.

## Complete Example

See `doc/examples/command_layers_config.toml` for a comprehensive configuration example with multiple command types.