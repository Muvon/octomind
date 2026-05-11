# Session Commands Reference

All commands available within an interactive Octomind session. Type the command at the prompt.

## Session Management

### `/help`
Show all available commands with descriptions.

### `/exit` / `/quit`
Exit the current session.

### `/list [PAGE]`
List all saved sessions. Optional page number for pagination.

### `/session [NAME]`
Switch to a different session by name. Without argument, shows current session info.

### `/clear`
Clear the terminal screen.

## Information & Monitoring

### `/info`
Display comprehensive session statistics:
- Token usage (input, output, cached, reasoning)
- Cost breakdown (per-request and cumulative)
- Cache savings (tokens and USD)
- Compression statistics (if compression has occurred)
- Model information

### `/report`
Generate a detailed usage report for the session with per-request breakdown.

### `/model [MODEL]`
Show or change the current model. Without argument, displays current model. With argument, switches to specified model in `provider:model` format.

```
/model openai:gpt-4o
/model anthropic:claude-sonnet-4
```

### `/role [ROLE]`
Show or change the current role. Without argument, displays current role.

```
/role assistant
/role developer
```
### `/effort [LEVEL]`
Show or change the reasoning effort level. Without argument, displays current level. With argument, sets the effort to one of: `low`, `medium`, `high`, `xhigh`, `max`.

```
/effort high
/effort max
```

### `/loglevel [LEVEL]`
Change the log level. Options: `none`, `info`, `debug`.

```
/loglevel debug
```

## Context Management

### `/context [FILTER]`
View session context (message history). Filters:
- `all` -- Show all messages
- `assistant` -- Only assistant messages
- `user` -- Only user messages
- `tool` -- Only tool calls and results
- `large` -- Only messages exceeding a token threshold

```
/context
/context tool
/context large
```

### `/done`
Mark the current task as complete. Triggers:
- Plan completion (if a plan is active)
- Force context compression (preserves only env-loaded skills, drops manually activated ones)
- Lesson extraction and storage (when `[learning]` is enabled — see [Learning Guide](../usage/13-learning.md))

## Media

### `/image <PATH>`
Attach an image for AI analysis. Supports PNG, JPEG, GIF, WebP.

```
/image screenshot.png
/image /path/to/diagram.jpg
```

### `/video <PATH>`
Attach a video for AI analysis.

```
/video demo.mp4
```

### `/copy`
Copy the last assistant response to the clipboard.

## MCP & Tools

### `/mcp [ACTION] [ARGS]`
Manage MCP servers at runtime.

| Action | Description |
|--------|-------------|
| `/mcp` or `/mcp list` | List all MCP servers with status |
| `/mcp info` | Detailed server information |
| `/mcp health` | Force health check on all servers |
| `/mcp validate` | Validate MCP configuration |
| `/mcp add <name> <url>` | Add HTTP server dynamically |
| `/mcp enable <name>` | Enable a registered server |
| `/mcp disable <name>` | Disable a server |
| `/mcp remove <name>` | Remove a server |

## Commands & Workflows

### `/run [COMMAND]`
Execute a custom command defined in `[[commands]]` config section. Without argument, lists available commands.

```
/run reduce
/run estimate
```

### `/workflow [NAME]`
Execute a workflow. Without argument, lists available workflows.

```
/workflow developer_workflow
```

### `/prompt [NAME]`
Inject a prompt template defined in `[[prompts]]` config section. Without argument, lists available prompts.

```
/prompt review
/prompt explain
```

### `/plan [ACTION]`

Manage the structured task plan.

| Usage | Description |
|-------|-------------|
| `/plan` or `/plan show` | Show current plan with progress |

**Note**: The `/plan` slash command only displays the current plan. To create, modify, or clear a plan, use the `plan` MCP tool directly with these commands:

- `plan(command="start", title="...", tasks=[...])` — Create a new plan
- `plan(command="next", content="...")` — Mark current task complete, advance to next
- `plan(command="step", content="...")` — Add progress note to current task
- `plan(command="reset")` — **Clear/reset the current plan**
- `plan(command="done", content="...")` — Complete the plan with final summary
### `/skill [NAME|PAGE|PATTERN]`
Manage skills from taps. Skills are reusable instruction packs that inject domain knowledge into context.

| Usage | Description |
|-------|-------------|
| `/skill` | List all skills (active first, then alphabetical) |
| `/skill <name>` | Toggle a skill on/off |
| `/skill <page>` | Show page N of the skill list |
| `/skill *pattern*` | Filter skills by glob pattern |

### `/schedule [SUBCOMMAND] [ARGS]`
Direct control over the built-in `schedule` MCP tool — schedule a message to be injected as a user message at a future time. Same operations as the MCP tool, but driven from chat input. See [Scheduled Tasks](../use-cases/07-scheduled-tasks.md) for the broader use case.

| Usage | Description |
|-------|-------------|
| `/schedule` or `/schedule list` | List all pending entries with IDs, trigger times, and countdown |
| `/schedule remove <id>` | Cancel a scheduled entry |
| `/schedule add when="<when>" message="<text>" [every="<interval>"] [description="<label>"]` | Schedule a new entry |
| `/schedule edit <id> [when="..."] [message="..."] [every="..."] [description="..."]` | Update an existing entry (use `every="none"` to clear a repeat) |
| `/schedule help` | Show inline usage |

Key=value tokens accept shell-style quoting so multi-word values work: `when="in 1h 30m"`, `message='hello world'`. Supported `when` formats: `now` (fires immediately), relative (`in 5m`, `in 1h30m`, `in 90s`), time-of-day (`15:30`, `3:30pm`, `9am` — tomorrow if past), or absolute (`2026-03-30 15:30`). `every` accepts the same duration syntax as relative `when` (`10m`, `1h`, `1h30m`).

Examples:
```
/schedule add when="now" message="say the date" every="5m"
/schedule add when="in 5m" message="check the build"
/schedule add when="9am" message="standup reminder" every="1h" description="daily"
/schedule edit abc12345 when="in 1h"
/schedule remove abc12345
```
