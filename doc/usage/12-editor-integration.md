# Editor Integration

Use this guide to connect an ACP-capable editor or client to Octomind. It covers the launch command, session setup, MCP
injection, supported prompt content, and troubleshooting.

## Get Started

Octomind runs as an ACP agent over stdio using JSON-RPC:

```bash
octomind acp assistant
```

Configure your client with executable `octomind` and arguments `acp`, `assistant`. The editor launches this as a
subprocess and communicates via JSON-RPC on stdio. Normal ACP tracing goes to files in the data directory's `logs/`
subdirectory. Keep stdout reserved for JSON-RPC; check the process exit status and stderr for failures before ACP
logging initializes.

The positional role/tag is optional. When omitted, the agent uses the default role from your config (the shipped default
is `assistant:concierge`). `TAG` can be:

- A **local role name** from your config (e.g. `assistant`), or
- A **tap agent** addressed as `category:variant` (e.g. `developer:general`).

For a tap role, launch:

```bash
octomind acp developer:general
```

Omitting the tag uses `assistant:concierge` in the shipped config; it does not select the local `assistant` role. Tap
tags must resolve from installed/fetched manifests. The built-in default tap is `muvon/tap`.

Each ACP session also spawns a background inbox monitor. It processes internally queued schedules, monitors, tap runs,
detached jobs, skills, guardrail feedback, and background-agent results without waiting for a user prompt; these arrive
in the editor as user-side message chunks. ACP does not start the `octomind send` or webhook listeners owned by
`octomind run`.

## Configure the Launch Command

| Flag | Description |
|------|-------------|
| `TAG` | Agent tag (e.g. `developer:general`) or local role name. Omit for the config default. |
| `--name`, `-n` | Preferred session name for the next `new_session` request |
| `--resume`, `-r` | Resume a specific session by name on the next `new_session` |
| `--resume-recent` | Resume the most recent session for the current working directory |
| `--model`, `-m` | Override the model name for sessions started by this agent (runtime > role > tap > main `[model]`) |
| `--sandbox` | Apply the platform sandbox using the launch working directory; see the configuration reference |
| `--hook` | Parsed and carried into ACP session options, but ACP does not currently start webhook listeners |

For a named session or a resume target, pass the corresponding flags in your editor's argument list:

```bash
octomind acp assistant --name editor-review
octomind acp --resume editor-review
octomind acp --resume-recent
```

To override the model or request sandboxing:

```bash
octomind acp assistant --model octohub:auto --sandbox
```

`--name`, `--resume`, and `--resume-recent` are consumed by the first `session/new` request. Model options apply to
sessions created or loaded for the lifetime of the ACP process. Start the process in the project directory;
`session/new` also supplies a session working directory.

Editor plugin configuration formats are not implemented in this repository. In your client's external-agent setup, use
the executable and argument list above; the protocol examples below define the Octomind side of the connection.

## Create and Resume Sessions

An ACP client initializes the connection, then creates a session. These JSON-RPC requests are sent as separate lines;
wait for each response before sending the next request. Replace `/absolute/project` with your actual project path:

```jsonl
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}
{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/absolute/project","mcpServers":[]}}
```

Use the returned `sessionId` for subsequent requests; `session-id-from-new` below means that returned value:

```json
{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"session-id-from-new","prompt":[{"type":"text","text":"Explain the project structure."}]}}
```

The agent advertises `loadSession` support. A client can restore a saved ID directly:

```json
{"jsonrpc":"2.0","id":4,"method":"session/load","params":{"sessionId":"session-id-from-new","cwd":"/absolute/project","mcpServers":[]}}
```

## MCP Server Injection

Editors can inject additional MCP servers into the Octomind session through the ACP `session/new` and `session/load`
requests. Behavior:

- **Per-session scope**: injected servers are merged into a per-session config snapshot and added to the role's
  `server_refs` for that session only. Your base config is never mutated.
- **Supported transports**: `stdio` and `HTTP` only. The agent advertises HTTP MCP support (`mcp_capabilities.http =
  true`) during initialization, so clients offer HTTP servers.
- **Unsupported transports**: `SSE` and any unknown transport are skipped (logged, not connected).
- **Timeout**: injected servers use 30 seconds per operation; tool progress resets the idle deadline, with an absolute
  cap of 20 times that value.
- **Credentials**: the current converter copies stdio command/args and HTTP name/URL, but ignores ACP stdio `env`
  entries and HTTP `headers`. Configure credentials in Octomind config or the parent process environment.
- **Name collisions**: an existing config server wins; injection does not replace a same-named server. Role tool filters
  still apply, so adding a server reference alone does not guarantee its tools are visible.

For a local HTTP server already listening on port 3000:

```json
{"jsonrpc":"2.0","id":5,"method":"session/new","params":{"cwd":"/absolute/project","mcpServers":[{"type":"http","name":"local_api","url":"http://localhost:3000/mcp","headers":[]}]}}
```

Inspect the resulting tools with `/mcp full`. See [MCP tools](07-mcp-tools.md) for authenticated TOML server examples.

## Attach Images and Video

ACP accepts text blocks, inline base64 image blocks, and embedded blob resources whose MIME type starts with `video/`.
Only the first image and first video in a prompt are attached. Audio, resource links, and other resource content are
ignored by this prompt handler, even though initialization advertises embedded context support.

For a concrete image example, this JSON-RPC prompt attaches a one-pixel PNG:

```json
{"jsonrpc":"2.0","id":6,"method":"session/prompt","params":{"sessionId":"session-id-from-new","prompt":[{"type":"text","text":"Describe this image."},{"type":"image","mimeType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4//8/AAX+Av4N70a4AAAAAElFTkSuQmCC"}]}}
```

For video, construct a resource block with your file's base64 bytes:

```json
{"type":"resource","resource":{"uri":"file:///absolute/project/clip.mp4","mimeType":"video/mp4","blob":"BASE64_VIDEO_BYTES"}}
```

Replace `BASE64_VIDEO_BYTES` with encoded video data and place the block in `session/prompt.params.prompt` alongside
text. The URI labels the resource; the handler consumes `blob`. The selected model must support the media you send.

## Available Slash Commands

The ACP agent currently advertises **26 command names** during the session. Names are sent **without the leading `/`** —
the client prepends it when displaying:

`help`, `role`, `model`, `done`, `info`, `clear`, `copy`, `context`, `list`, `session`, `run`, `workflow`, `mcp`,
`plan`, `prompt`, `image`, `video`, `loglevel`, `report`, `skill`, `effort`, `schedule`, `agents`, `usage`, `login`,
`exit`

Notes:

- The menu omits some registered commands and also includes three unsupported names. Commands such as `/learning`,
  `/share`, `/analyze`, `/rename`, and `/status` are not advertised over ACP.
- `/done` is handled specially in ACP: it compresses the conversation and reports the result. If you pass trailing
  instructions (`/done <instructions>`), the agent compresses first, sends the compression status, then processes the
  instructions as a normal prompt.
- Three advertised names are not wired into the shared slash-command dispatcher: `session`, `workflow`, and `agents`.
  ACP reports them as unsupported if invoked. Use ACP `session/new`/`session/load` for client session management, the
  external workflow CLI for workflows, and `/status agents` for activity.
- `/effort` accepts `low`, `medium`, `high`, `xhigh`, or `max` (the advertised input hint only shows the first three).
- Editors that support arbitrary slash input may send other registered commands even when they are absent from the menu;
  unknown slash commands receive an unsupported-command response rather than reaching the model.

For clients accepting arbitrary slash input:

```text
/info
/mcp full
/status agents
/effort high
/done Review the next requirement.
```

The advertised `/done` description mentions auto-commit, but its handler compresses and runs learning; it does not
commit changes. `/done` with trailing instructions is special to the prompt path.

### Programmatic command execution

Beyond the slash-command menu, clients can invoke commands programmatically through the ACP extension wire method
`_octomind/command` (dispatched internally as `octomind/command`). The request carries `{ session_id, command, args }`
and the response returns `{ success, output, error }` with structured JSON output. The `command` value must include its
leading slash:

```json
{"jsonrpc":"2.0","id":7,"method":"_octomind/command","params":{"session_id":"session-id-from-new","command":"/mcp","args":["list"]}}
```

Inspect `output.command_type` and its command-specific data as well as the outer `success`: a handled command can return
an error in its structured output. The extension method does not apply the prompt path's `/done <instructions>`
follow-up behavior.

## Cost and Usage Reporting

As a session runs, the agent emits a `SessionInfoUpdate` notification carrying a `_meta["octomind.usage"]` payload on
the session notification with `session_tokens`, `session_cost`, `input_tokens`, `output_tokens`, `cache_read_tokens`,
`cache_write_tokens`, and `reasoning_tokens`. Clients that pass `_meta` through can display live cost and token usage.
The usage block has this shape (values are illustrative):

```json
{"octomind.usage":{"session_tokens":1200,"session_cost":0.01,"input_tokens":1000,"output_tokens":200,"cache_read_tokens":0,"cache_write_tokens":0,"reasoning_tokens":0}}
```

## Role Tool Selection

The selected role controls requested MCP servers and tool filters; this is configuration, not a standalone security
boundary. Define local roles with `[[roles]]` and exact `name` values. See [Roles](06-roles.md) for configuration and
[Configuration reference](../reference/03-config-reference.md#root-level-settings) for process restrictions.

## Troubleshooting

**Agent not found:** Ensure the editor can find the executable and that the requested role resolves. Check CLI
availability:

```bash
command -v octomind
octomind acp --help
```

**No response / hangs:**

- For the shipped `octohub:auto` profile, run `octomind login`; for a direct provider model, ensure its credential
  variable reaches the editor process
- Editor may need to inherit shell environment variables
- Check `~/.local/share/octomind/logs/acp-debug.log` for runtime errors

**Tools not available:**

- Verify the role has correct `server_refs` and `allowed_tools`
- Check `~/.local/share/octomind/logs/acp-errors.jsonl` for structured error details

**Agent fails to start at all:**

- ACP tracing/error-sink initialization failures use `acp-init-errors.log`. Earlier config/role failures can still reach
  stderr; capture the child exit status and stderr in your client.

The paths above are macOS/Linux defaults. Windows uses `%LOCALAPPDATA%/octomind/logs/`; `OCTOMIND_DATA_DIR` relocates
the whole data tree. For a shell using the default location:

```bash
octomind login
tail -n 50 "$HOME/.local/share/octomind/logs/acp-debug.log"
tail -n 20 "$HOME/.local/share/octomind/logs/acp-errors.jsonl"
```

**Why does a request wait while a background result is processed?** Prompts, extension commands, and inbox work share
per-session exclusion locks. They run serially for a session.

## Source Reference

- [Launch flags](../../src/commands/acp.rs)
- [Sessions, prompt content, menu, and MCP injection](../../src/acp/agent.rs)
- [Command extension](../../src/acp/commands.rs)
- [Logging initialization](../../src/acp/mod.rs)
- [Data paths](../../src/directories.rs)

## See also

- [ACP Protocol](../integration/02-acp-protocol.md) — full handshake, capabilities, and session lifecycle
- [WebSocket Server](../integration/01-websocket-server.md) — alternative integration transport
- [CLI Reference](../reference/01-cli-reference.md) — complete `octomind` command and flag reference
- [Session Commands](../reference/02-session-commands.md) — all interactive session commands
