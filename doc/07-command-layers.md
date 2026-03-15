# Command Layers and Workflows in Octomind

Octomind provides two powerful systems for specialized AI processing:

1. **Workflows** - Brain-inspired planning system for complex multi-step processing
2. **Command Layers** - Standalone utilities that don't affect session history

## Workflows: Brain-Inspired Planning

Workflows are Octomind's planning system that enables complex, multi-step AI processing with validation, feedback loops, and conditional branching. See [doc/10-workflows.md](./10-workflows.md) for comprehensive documentation.

### Workflow Execution

Use the `/workflow` command to execute workflows:

```bash
# List available workflows
/workflow

# Execute a specific workflow
/workflow simple_workflow

# Execute workflow with custom input
/workflow feedback_loop "Your custom input here"
```

### Workflow Step Types

Workflows support five control flow primitives:

1. **Once** - Execute a layer once
2. **Loop** - Repeat until exit condition or max iterations
3. **Foreach** - Iterate over parsed items
4. **Conditional** - Branch based on pattern matching
5. **Parallel** - Execute layers in parallel

For complete workflow documentation, see [doc/10-workflows.md](./10-workflows.md).

## Command Layers: Standalone Utilities

Command layers are a powerful feature that allows you to define specialized AI helpers that can be invoked without affecting your session history. They use the same flexible layer infrastructure as the main processing pipeline but operate as standalone utilities.

### Key Benefits

- **Non-intrusive**: Commands don't affect your conversation history
- **Specialized**: Each command can have its own model, system prompt, and configuration
- **Flexible**: Use the same layer configuration system as regular layers
- **Cost-effective**: Only run when needed, with isolated token usage
- **Context-aware**: Proper input_mode handling for accessing session context

### Usage

Use the `/run` command followed by the command name:

```bash
/run estimate
/run task_refiner
/run review
```

## Layer System Architecture

Octomind's layer system provides a flexible AI processing pipeline that can be configured for different roles and use cases:

### Built-in Layers

**Task Refiner** (`task_refiner`):
- Refines and clarifies user requests for better processing
- Uses lightweight models for cost efficiency
- Provides initial file guessing and query structuring

**Task Researcher** (`task_researcher`):
- Comprehensive analysis and research for complex tasks
- Full MCP tool access for investigation
- Detailed context gathering and solution planning

### Layer Configuration

Layers are configured in `config-templates/default.toml` with the following structure:

```toml
[[layers]]
name = "layer_name"
description = "Layer description for agents and documentation"
model = "provider:model"
max_tokens = 2048
temperature = 0.7
input_mode = "last"   # last, all, summary
output_mode = "none"  # none, append, replace, last, restart
system_prompt = "Layer-specific system prompt"

[layers.mcp]
server_refs = ["filesystem", "core"]
allowed_tools = ["filesystem:*", "core:*"]
```

### Custom Commands

Commands use the same layer infrastructure but operate as standalone utilities:

```toml
[[commands]]
name = "estimate"
description = "Provides project estimation"
model = "provider:model"
system_prompt = "You are an estimation expert..."

[commands.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view"]
```

Commands are configured similarly to layers but in the `[[commands]]` array.

### Available Input Modes

Layers and commands support different input modes that control how data is passed between processing stages:

- **`last`** (case-insensitive): Gets the last assistant response from session context - perfect for analyzing previous AI responses
- **`all`**: Uses the entire conversation history with proper formatting
- **`summary`**: Uses a summarized version of the conversation to save tokens

**Note**: Input modes are case-insensitive, so `"Last"`, `"last"`, `"LAST"` all work the same way.

### Available Output Modes

Output modes control how the layer's output affects the session:

- **`none`**: Don't modify session (intermediate layer like task_refiner) - layer output is returned directly
- **`append`**: Add output as new message to session
- **`replace`**: Replace entire session with output (reducer functionality)
- **`last`**: Append only the last response to session (ignore multiple outputs)
- **`restart`**: Replace session with only the last response (fresh start with last message)

### Smart Context Processing

The input_mode system handles session context:

- **With `input_mode = "last"`**: If no explicit input is provided, the command gets the last assistant response. If explicit input is provided, it's combined with the previous assistant context.
- **With `input_mode = "all"`**: The command receives the full conversation context formatted for analysis.
- **With `input_mode = "summary"`**: The command gets a concise summary of the conversation.

### Visual Feedback

Command layers display the same smart animation as main sessions during API processing:

- **Initial API call**: Shows animated spinner with context token count and cost
- **Follow-up calls**: Animation continues during recursive tool processing
- **Cancellation**: Animation stops immediately when operation is cancelled
- **Context tracking**: Displays current tokens, cost, and threshold information

This provides consistent visual feedback across both main sessions and command layer execution.

### MCP (Tool) Integration

Commands can have their own tool configurations using the new server registry system:

```toml
[developer.commands.review.mcp]
server_refs = ["core", "filesystem", "agent"]  # Reference servers by name
allowed_tools = ["core:*", "filesystem:*", "agent:*"]   # Limit to specific tools
```

## Example Commands

### 1. Project Estimation

```toml
[[commands]]
name = "estimate"
description = "Provides project estimation and time analysis for development tasks"
model = "openrouter:openai/gpt-4.1-mini"
system_prompt = """You are a project estimation expert. Analyze the work done or discussed and provide:

1. Time required for completion
2. Complexity assessment (1-5)
3. Potential challenges
4. Suggested next steps

Be specific and practical."""
temperature = 0.2
input_mode = "last"  # Gets the last assistant response for analysis

[commands.mcp]
server_refs = []  # No tools needed for estimation
```

Usage: `/run estimate`

### 2. Code Review

```toml
[[commands]]
name = "review"
model = "openrouter:anthropic/claude-sonnet-4"
system_prompt = """You are a code review expert. Analyze recent work and provide:

1. Code quality assessment
2. Potential improvements
3. Best practices recommendations
4. Security considerations

Focus on constructive feedback."""
temperature = 0.1
input_mode = "all"  # Gets full conversation context

[commands.mcp]
server_refs = ["core", "filesystem"]  # Access to code files and shell
allowed_tools = ["filesystem:text_editor", "filesystem:view", "filesystem:shell"]
```

Usage: `/run review`


### 3. Quick Summary

```toml
[[commands]]
name = "summarize"
model = "openrouter:openai/gpt-4o-mini"  # Fast, cheap model
system_prompt = "Provide a concise summary of the conversation and key points."
temperature = 0.2
input_mode = "summary"  # Uses pre-summarized content to save tokens

[commands.mcp]
server_refs = []  # No tools needed
```

Usage: `/run summarize`

## Advanced Features

### Parameters and Placeholders

Command layers support custom parameters that can be used in system prompts using `{{var}}` syntax:

```toml
[[commands]]
name = "estimate"
system_prompt = "You estimate projects for {{team_size}} person team with {{project_type}} focus."

[commands.parameters]
team_size = "3"
project_type = "web development"
```

### Multiple Models

Different commands can use different models optimized for their specific tasks:

```toml
[[commands]]
name = "quick_check"
model = "openrouter:openai/gpt-4o-mini"  # Fast, cheap model

[[commands]]
name = "deep_analysis"
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
| **Configuration** | `[[layers]]` section | `[[commands]]` section |

## Best Practices

1. **Keep commands focused**: Each command should have a specific purpose
2. **Use appropriate models**: Match model capabilities to command complexity
3. **Optimize input modes**: Use "Last" for quick commands, "All" for analysis
4. **Enable tools selectively**: Only give commands the tools they need
5. **Document your commands**: Use clear names and system prompts

### Command Not Found
```
Command 'estimate' not found in configuration
```
- Check that the command is defined in your `[[commands]]` section
- Verify proper TOML syntax in your configuration
- Use `/run` without parameters to see available commands

### Tool Execution Errors
```
Tool execution failed: Unknown tool 'view'. Available tools: view, text_editor, batch_edit, extract_lines, shell, workdir, ast_grep, plan, mcp, agent
```
- **Tool routing issue**: The tool is being sent to the wrong server
- **Solution**: Check your MCP server configuration and ensure proper server references
- **Verify**: Tools are being routed to the correct server type (filesystem tools → filesystem server)

### Input Mode Issues
```
Unknown input mode: 'Last'. Valid options: last, all, summary
```
- **Case sensitivity**: Input modes are now case-insensitive but should use lowercase
- **Solution**: Use `input_mode = \"last\"` instead of `input_mode = \"Last\"`
- **Valid values**: `\"last\"`, `\"all\"`, `\"summary\"` (any case)

### No Commands Available
```
No command layers configured.
```
- Add command definitions to your configuration file
- Use `/run` without parameters to see configuration examples
- Check `[[commands]]` section in your config

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

The existing `/done` command continues to work as before, but now uses the intelligent compression system instead of simple context reduction. Command layers provide additional functionality without replacing the core session finalization flow.

## Complete Example

See `doc/examples/command_layers_config.toml` for a comprehensive configuration example with multiple command types.
