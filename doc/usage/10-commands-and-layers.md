# Custom Commands, Agents, and Prompt Templates

Add custom commands, ACP sub-agents, and prompt templates to Octomind: define them in a TOML file in your config directory, then start a new session.

## Get Started

The shipped `reduce` command compresses conversation history through the local `reduce` role. In an existing session:

```text
/run
/run reduce
```

Add custom definitions to a TOML file in your config directory, then start a new session to load them:

```bash
octomind run assistant
```

See [Configuration reference](../reference/03-config-reference.md) for config locations and loading rules.

## Extension Types

- **Commands** — ACP-backed steps triggered interactively with `/run <name>` (`[[commands]]`).
- **Agents** — installed or session-private tap roles delegated to through `orchestration:tap`.
- **Prompts** — reusable prompt templates queued with `/prompt <name>` (`[[prompts]]`).

All of these are user-defined (or provided by a tap). The default config ships one command (`reduce`) and one agent
(`context_gatherer`).

## Custom Commands

Commands run with `/run <name> [input]` and execute via ACP (Agent Client Protocol). Model, system prompt, and MCP tool
access live in `[[roles]]` config — commands reference roles via the `command` field.

### Configuration

The example below is an illustrative custom command — it is not shipped by default and requires a matching `analysis`
role in `[[roles]]` (see the role example below):

```toml
[[commands]]
name = "analysis"
description = "Analyze the previous response and current request"
command = "octomind acp analysis"
input_mode = "last"
output_mode = "append"
output_role = "assistant"
```

`input_mode`, `output_mode`, and `output_role` are **all mandatory** — they have no serde defaults, so omitting any of
them is a TOML parse error. Only `workdir` defaults (to `"."`).

### Input Modes

How the command receives conversation input:

| Mode | Description |
|------|-------------|
| `"last"` | Explicit input plus the last assistant response; with empty input, the last assistant response or last genuine user task |
| `"all"` | Chronological non-system transcript followed by the current task |
| `"summary"` | Concatenated assistant text truncated to 2000 characters, plus current input and a request to summarize |

### Output Modes

How the command's output affects the session:

| Mode | Description |
|------|-------------|
| `"none"` | Intermediate processing, doesn't modify session |
| `"append"` | Adds output as a new message to the session |
| `"replace"` | Keeps the first system message, rebuilds welcome/instructions, and inserts all command outputs |
| `"last"` | Append only the last response to session (ignore multiple outputs) |
| `"restart"` | Clears all messages and inserts only the last command output |

### Command Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Command name, typed after `/run` |
| `description` | string | yes | Human-readable purpose (shown in help) |
| `command` | string | yes | ACP command to execute: `octomind acp <role_name>` |
| `workdir` | string | no | Working directory (the only field with a default: `"."`). Relative paths resolve against the session's working directory. |
| `input_mode` | string | yes | `"last"`, `"all"`, or `"summary"` |
| `output_mode` | string | yes | `"none"`, `"append"`, `"replace"`, `"last"`, `"restart"` |
| `output_role` | string | yes | `"assistant"` or `"user"` — role for output messages. No default; must be set explicitly. |

The `command` field is split on whitespace into an executable and arguments; it is not evaluated by a shell. Do not
embed shell quoting, pipes, or environment assignments. For complex launch logic, point `command` at a wrapper executable that starts the ACP server. Model, system
prompt, and tool access are resolved by the ACP child role.

Example role definition (in config or from taps) that the `analysis` command above would target:
```toml
[[roles]]
name = "analysis"
system = "Analyze the supplied code or requirements. Identify assumptions, concrete risks, and next steps."
welcome = ""

[roles.model]
name = "openai:gpt-5.6-luna"

[roles.mcp]
server_refs = []
allowed_tools = []
```

### Usage

```text
/run
/run analysis Check the assumptions in this design
```

`/run` resolves commands through `Config::get_role_config`; current config merging supplies the global `[[commands]]`
set to roles. The parent uses the current role's resolved command set; the `command` executable selects the child role.
Additional words after the command name become its explicit input. Without them, `/run` supplies the last genuine user
task. Session and request spending checks can decline execution before the child starts.

## Agents

See [Delegate work to tap roles](../use-cases/05-multi-agent-delegation.md) for creating, running and saving a specialist.

## Prompt Templates

Reusable prompts sent into the session via `/prompt <name>`. The prompt text is queued into the session inbox and picked
up by the main loop as a normal **user message** on the next turn — so the AI responds to it as a fresh user turn, it is
not silently appended. The template is sent verbatim: prompt-template variable substitution (`{role}`, `{model}`, etc.)
is not currently implemented.

The `description` field is optional; the examples below set it, but it can be omitted.

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

```text
/prompt
/prompt review
```

## Common Questions

**Why is my command missing from `/run`?** Put it in `[[commands]]` and reload by starting a new session. `/run` lists
the loaded command names.

**Why does a command fail to start?** Confirm its executable is installed and its role resolves. Defining a command does not define a role.

```bash
octomind acp analysis --help
```

This checks the command-line syntax; it does not launch or validate the custom role. Start a session and invoke the
command with a small input to check execution:

```text
/run analysis Summarize the assumptions in the previous answer
```

**Why did replace remove my conversation?** `output_mode = "replace"` keeps the first system message, rebuilds
welcome/instructions, and replaces the old conversation with command outputs. Use `append` when you need to retain the
conversation.

## Source Reference

- [Shipped definitions](../../config-templates/default.toml)
- [Command fields and input preparation](../../src/session/layers/layer_trait.rs)
- [Command output modes](../../src/session/chat/command_executor.rs)
- [Agent execution](../../src/acp/driver.rs)
- [Prompt queueing](../../src/session/chat/session/commands/prompt.rs)

## See also

- [MCP tools](07-mcp-tools.md)
- [Context compression](08-compression.md)
- [Workflows](09-workflows.md)
- [Roles](06-roles.md)
- [Editor integration](12-editor-integration.md)
