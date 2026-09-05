# Structured Output

Use this guide to consume Octomind activity as JSONL and request schema-shaped model answers. It is for scripts and
clients that need machine-readable session output.

The activity stream and the model answer are separate surfaces: `--format jsonl` structures session events; `--schema`
attaches a JSON Schema to model requests. WebSocket and ACP expose activity through their own protocols.

## Get Started with JSONL

The `run` command takes a `--format` string. The runtime implements two output modes:

- `plain` — human-formatted terminal output (the default).
- `jsonl` — one JSON object per line (JSON Lines) on stdout.

Clap does not constrain the string to an enum, but any value other than `jsonl` resolves to the plain output path; use
only the two documented values.

Setting `--format jsonl` switches Octomind into non-interactive mode: it reads the prompt from **stdin** and streams the
session as JSONL.

```bash
echo "Summarize recent changes" | octomind run --format jsonl
```

Omitting the tag uses the configured default (shipped as `assistant:concierge`). You can also select it explicitly:

```bash
echo "Summarize recent changes" | octomind run assistant:concierge --format jsonl
```

Notes:

- Among session-serving subcommands, `--format` belongs to `run`; `server` and `acp` use their protocols instead. The
  separate `workflow` command also accepts `--format`, where only `jsonl` produces stdout events.
- For ordinary one-shot runs, `--format` requires piped stdin. Without `--format`, redirected stdin also starts a
  non-interactive run, using plain output. An empty pipe is rejected. `--format jsonl --daemon` can start with no
  initial prompt when stdin is a terminal.
- The default tag is `assistant:concierge` (a tap agent from the built-in default tap `muvon/tap`); the stock config
  also ships the local roles `assistant`, `task_refiner`, `task_researcher`, and `reduce`. (See [CLI
  Reference](../reference/01-cli-reference.md) for the full flag set and [Roles](06-roles.md) for tags.)

## What the JSONL Stream Contains

Each line is a single JSON object with a `"type"` field that tells you which kind of event it is. These are the same
`ServerMessage` variants the WebSocket server emits, serialized one-per-line. The variants are:

| `type` | Meaning | Key fields |
|--------|---------|-----------|
| `assistant` | Assistant response text | `content`, `session_id` |
| `thinking` | Model reasoning/thinking content (separate from the answer) | `content`, `session_id` |
| `tool_use` | The agent is about to call a tool | `tool`, `tool_id`, `server`, `params`, `session_id` |
| `tool_result` | Result of a tool call | `tool`, `tool_id`, `server`, `content`, `success`, `session_id` |
| `cost` | Token/cost accounting | `session_tokens`, `session_cost`, `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `reasoning_tokens`, `session_id` |
| `status` | Non-critical status/info (also carries command results in `data`) | `message`, `session_id?`, `data?` |
| `error` | Error message | `message`, `request_id?` |
| `mcp_notification` | Notification forwarded from an MCP server | `server`, `method`, `params`, `tool_id?` |
| `skill` | Skill lifecycle event (`activate` / `use` / `forget`) | `action`, `name`, `trigger?`, `session_id` |
| `evolution` | Generated behavior lifecycle event | `action`, `id`, `name`, `kind`, `state`, `scope`, `session_id` |
| `injected` | A non-user message injected into the loop (schedule, monitor, background agent/job, tap run, skill, webhook, guardrail, …) | `source_kind`, `source_label`, `content`, `session_id` |

The WebSocket transport additionally sends `ack` for a valid client frame, including request correlation and session
identifiers. A piped `run --format jsonl` has no incoming WebSocket frame to acknowledge.

Illustrative events from a `jsonl` run (one object per physical line):

```jsonl
{"type":"status","message":"Session created: my-session","session_id":"my-session"}
{"type":"tool_use","tool":"mcp","tool_id":"call_abc","server":"runtime","params":{"action":"list"},"session_id":"my-session"}
{"type":"tool_result","tool":"mcp","tool_id":"call_abc","server":"runtime","content":"No MCP servers configured or registered.","success":true,"session_id":"my-session"}
{"type":"assistant","content":"No additional MCP servers were found.","session_id":"my-session"}
{"type":"cost","session_tokens":1234,"session_cost":0.0,"input_tokens":1000,"output_tokens":200,"cache_read_tokens":30,"cache_write_tokens":4,"reasoning_tokens":0,"session_id":"my-session"}
```

To stream every assistant message, filter with `jq` (install `jq` separately):

```bash
echo "Summarize recent changes" | octomind run --format jsonl \
  | jq -r 'select(.type == "assistant") | .content'
```

For only the final assistant message from a completed one-shot run, collect the events and select the last:

```bash
printf '%s\n' 'Summarize recent changes' | octomind run --format jsonl \
  | jq -sr '[.[] | select(.type == "assistant")][-1].content // empty'
```

This waits for EOF, so use the streaming filter for daemon sessions. Check the producer's exit status as well as any
`error` events; failures before session initialization may appear on stderr.

## Schema Enforcement (`--schema`)

Save this schema as `todos.schema.json`:

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["todos"],
  "properties": {
    "todos": {"type": "array", "items": {"type": "string"}}
  }
}
```

Pass the file to `octomind run` to request answers matching that shape:

```bash
printf '%s\n' 'List three follow-up tasks for a completed deployment' | octomind run --format jsonl --schema todos.schema.json
```

- The schema is attached to model requests throughout this process, including follow-ups after tools and daemon turns.
  Activity events keep their usual outer shape; an `assistant.content` string carries the model's answer.
- The resolved provider must report `supports_structured_output(model)` at startup, or the run fails. That capability is
  supplied by the installed provider library; it does not establish hard schema enforcement for every route.
- Like `--model`, the schema is a **runtime override** — it is not persisted with the session, so pass it again when
  resuming.
- The file must parse as JSON and have an object root; Octomind does not compile the full schema at load time. A
  ready-to-use example ships at [`config-templates/todos.schema.json`](../../config-templates/todos.schema.json).
- `--schema` exists only on `octomind run` — the WebSocket and ACP session-init messages do not accept a schema.

A successful schema-shaped answer is still wrapped as an event:

```json
{"type":"assistant","content":"{\"todos\":[\"Check logs\",\"Verify health\",\"Record the release\"]}","session_id":"deployment"}
```

Decode the answer separately from its event envelope:

```bash
printf '%s\n' 'List deployment follow-up tasks' | octomind run --format jsonl --schema todos.schema.json \
  | jq -sr '[.[] | select(.type == "assistant")][-1].content | fromjson'
```

When resuming, supply the schema again:

```bash
printf '%s\n' 'List remaining tasks' | octomind run --name deployment --format jsonl --schema todos.schema.json
```

Validate consumed answers against your schema in the calling application. The CLI capability check and JSON parsing are
not an independent guarantee that every returned answer satisfies the schema.

Compression uses a separate capability check, `enforces_response_schema(model)`: it sends a strict JSON schema when that
guarantee is advertised, and otherwise uses an XML prompt with local structural parsing. See [Context
compression](08-compression.md) for configuration.

## Bidirectional Clients

Start either server mode for a client that needs to send additional turns:

```bash
octomind server --host 127.0.0.1 --port 8080
```

Or let an ACP client launch:

```bash
octomind acp
```

WebSocket uses the same event family as JSONL. Send a session frame, wait for its status, then send a user message:

```jsonl
{"type":"session","request_id":"create-1","session_id":"deployment"}
{"type":"message","request_id":"turn-1","session_id":"deployment","content":"Summarize the deployment status"}
```

An `ack` confirms acceptance of a valid frame, not completion of its work. Use `request_id` for acknowledgement/error
correlation and `session_id` for session events. Browser connections require a matching `--allow-origin`:

```bash
octomind server --allow-origin http://localhost:3000
```

ACP maps activity into native session updates and supports `_octomind/command` requests. See [Editor
integration](12-editor-integration.md) for a complete command payload. Neither transport accepts a schema on session
creation.

## Common Questions

**Why is the answer still a string?** `--schema` shapes the answer inside `assistant.content`; parse that string as
JSON. The event envelope is unchanged.

**Why does startup reject my schema?** Check that the file is readable, valid JSON, and an object at the top level. If
the model reports no structured-output support, select a compatible model using `--model`.

**Why did a pipeline appear successful after Octomind failed?** In Bash, enable `pipefail` so a downstream filter does
not hide a failing producer:

```bash
set -o pipefail
printf '%s\n' 'List deployment follow-up tasks' | octomind run --format jsonl --schema todos.schema.json \
  | jq -c 'select(.type == "assistant")'
```

## Source Reference

- [CLI and stdin](../../src/commands/run.rs)
- [Event payloads](../../src/websocket/protocol.rs)
- [Schema loading and capability gate](../../src/session/completion.rs)
- [Compression JSON/XML selection](../../src/session/chat/conversation_compression/ai.rs)

## See also

- [CLI reference](../reference/01-cli-reference.md)
- [Workflows](09-workflows.md)
- [Context compression](08-compression.md)
- [Editor integration](12-editor-integration.md)
- [WebSocket server](../integration/01-websocket-server.md)
