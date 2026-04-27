# Commands, Layers, Agents, and Prompts

Octomind provides four mechanisms for extending AI capabilities beyond the base session.

## Layers

Layers execute via ACP (Agent Client Protocol). Model, system prompt, and MCP tool access live in `[[roles]]` config — layers reference roles via the `command` field. Layers are workflow building blocks; commands are interactive layer shortcuts.

### Configuration

```toml
[[layers]]
name = "task_refiner"
description = "Refines and clarifies user requests for better processing"
command = "octomind acp task_refiner"
input_mode = "last"
output_mode = "none"
output_role = "assistant"
```

### Input Modes

How the layer receives conversation input:

| Mode | Description |
|------|-------------|
| `"last"` | Only the last assistant/message from the session (default for workflow stages) |
| `"all"` | Entire conversation history from the session |
| `"summary"` | A summarized version of the conversation history |

### Output Modes

How the layer's output affects the session:

| Mode | Description |
|------|-------------|
| `"none"` | Intermediate processing, doesn't modify session (for workflow stages like task_refiner) |
| `"append"` | Adds output as a new message to the session |
| `"replace"` | Replaces entire session content with layer output (reducer functionality) |
| `"last"` | Append only the last response to session (ignore multiple outputs) |
| `"restart"` | Replace session with only the last response (fresh start with last message) |
### Built-in Layers

### Built-in Layers

The default configuration includes layers that reference built-in roles:

| Layer | Command | Purpose |
|-------|---------|---------|
| `task_refiner` | `octomind acp task_refiner` | Query cleanup and initial file guessing |
| `task_researcher` | `octomind acp task_researcher` | Context gathering via code analysis |

### Layer Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Layer identifier |
| `description` | string | yes | Human-readable purpose (shown in help) |
| `command` | string | yes | ACP command to execute: `octomind acp <role_name>` |
| `workdir` | string | no | Working directory (default: `"."`) |
| `input_mode` | string | yes | `"last"`, `"all"`, or `"summary"` |
| `output_mode` | string | yes | `"none"`, `"append"`, `"replace"`, `"last"`, `"restart"` |
| `output_role` | string | no | `"assistant"` (default) or `"user"` — role for output messages |

**Key Architecture**: Layers don't contain model/system/mcp config. Those live in `[[roles]]`. The `command` field references which role to spawn via ACP.

Example role definition (in config or from taps):
```toml
[[roles]]
name = "task_refiner"
model = "openrouter:openai/gpt-4.1-mini"
system = "You are a query processor..."
temperature = 0.3

[roles.mcp]
server_refs = []
allowed_tools = []
```
## Custom Commands

Commands are layers triggered interactively via `/run <name>`. Same configuration as layers.

```toml
[[commands]]
name = "reduce"
description = "Compress session history for cost optimization during ongoing work"
command = "octomind acp reduce"
input_mode = "all"
output_mode = "replace"
output_role = "assistant"
```

### Usage

```
/run              # List available commands
/run reduce       # Execute the reduce command
```

### Layers vs Commands

| Feature | Layer | Command |
|---------|-------|---------|
| Triggered by | Workflows, code | User via `/run` |
| Config section | `[[layers]]` | `[[commands]]` |
| Interactive | No | Yes |
| Typical use | Pipeline stages | User-initiated actions |

Both use identical configuration fields.

## Agents

Agents are specialized AI instances that run as separate processes via ACP (Agent Client Protocol). Each agent becomes an MCP tool.

### Configuration

```toml
[[agents]]
name = "context_gatherer"
description = "Gather detailed context from files and codebase."
command = "octomind acp context_gatherer"
workdir = "."
```

### How Agents Work

1. Define agent in `[[agents]]` with `name`, `description`, and `command`
2. Agent becomes MCP tool `agent_<name>` (e.g., `agent_context_gatherer`)
3. When called, Octomind spawns the command as a child process
4. Communication happens via JSON-RPC over stdio (ACP protocol)
5. Agent's final response is returned as the tool result

### Agent Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique ID. Tool becomes `agent_<name>`. |
| `description` | string | yes | MCP tool description shown to AI |
| `command` | string | yes | Shell command starting ACP server over stdio |
| `workdir` | string | no | Working directory (default: `"."`) |

### Agent Tool Parameters

Each agent tool accepts:
- `task` (string, required): Task description in human language
- `async` (boolean, default: false): Run asynchronously

### Async Agents

`async: true` returns immediately. Result appears as a user message when complete.

Use async when:
- Task takes 30+ seconds
- You can continue other work
- You don't need the result immediately

Max concurrent async jobs is configurable. Jobs cancelled on session exit.

### Dynamic Agents

Create agents at runtime using the `agent` MCP tool:

```json
{"action": "add", "name": "reviewer", "description": "Code reviewer", "system": "You review code..."}
{"action": "enable", "name": "reviewer"}
```

See [MCP Tools Reference](07-mcp-tools.md#agent----dynamic-agent-management).

## Prompt Templates

Reusable prompts injected into the session via `/prompt <name>`.

```toml
[[prompts]]
name = "review"
description = "Request code review with focus on best practices"
prompt = """Please review the code above focusing on:
- Code quality and best practices
- Security considerations
- Performance implications"""

[[prompts]]
name = "explain"
description = "Ask for detailed explanation"
prompt = "Please provide a detailed explanation of the code/concept above."

[[prompts]]
name = "test"
description = "Request test cases"
prompt = """Please help create comprehensive tests:
- Unit test cases
- Edge cases and error conditions
- Integration test considerations"""
```

### Usage

```
/prompt              # List available prompts
/prompt review       # Inject the review prompt
```
