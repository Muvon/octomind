# Structured Output

Octomind can emit its session activity as machine-readable JSON instead of human-formatted terminal text. This is what you use for automation, CI/CD pipelines, and any program that needs to parse what the agent did.

> **Heads up:** "structured output" covers two different features here. To make the model's *answer* conform to a JSON Schema, pass `--schema <file>` to `octomind run` — see [Schema Enforcement](#schema-enforcement---schema) below. Independent of that, the session's *activity* is available as a structured **event stream** (`--format jsonl` and the WebSocket/ACP servers), described next. Note the WebSocket and ACP protocols do not accept a schema — `--schema` exists only on `octomind run`.

## The Automation Surface: `--format jsonl`

The `run` command takes a `--format` flag. It accepts exactly two values:

- `plain` — human-formatted terminal output (the default).
- `jsonl` — one JSON object per line (JSON Lines) on stdout.

Setting `--format jsonl` switches Octomind into non-interactive mode: it reads the prompt from **stdin** and streams the session as JSONL.

```bash
echo "Summarize recent changes" | octomind run --format jsonl
```

Omitting the tag uses the default agent. You can also target a real default role, for example:

```bash
echo "Summarize recent changes" | octomind run assistant --format jsonl
```

Notes:

- `--format` only exists on the `run` subcommand. The `server` and `acp` subcommands stream structured output by their own protocols (see below); they have no `--format` flag.
- When `--format` is set, input always comes from stdin — there is no interactive prompt.
- The default tag is `assistant:concierge` (a tap agent from the built-in default tap `muvon/tap`); the stock config also ships the local roles `assistant`, `task_refiner`, `task_researcher`, and `reduce`. (See [CLI Reference](../reference/01-cli-reference.md) for the full flag set and [Roles](06-roles.md) for tags.)

## What the JSONL Stream Contains

Each line is a single JSON object with a `"type"` field that tells you which kind of event it is. These are the same `ServerMessage` variants the WebSocket server emits, serialized one-per-line. The variants are:

| `type` | Meaning | Key fields |
|--------|---------|-----------|
| `assistant` | Assistant response text | `content`, `session_id` |
| `thinking` | Model reasoning/thinking content (separate from the answer) | `content`, `session_id` |
| `tool_use` | The agent is about to call a tool | `tool`, `tool_id`, `server`, `params`, `session_id` |
| `tool_result` | Result of a tool call | `tool`, `tool_id`, `server`, `content`, `success`, `session_id` |
| `cost` | Token/cost accounting | `session_tokens`, `session_cost`, `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `reasoning_tokens`, `session_id` |
| `status` | Non-critical status/info (also carries command results in `data`) | `message`, `session_id?`, `data?` |
| `error` | Error message | `message` |
| `mcp_notification` | Notification forwarded from an MCP server | `server`, `method`, `params` |
| `skill` | Skill lifecycle event (`activate` / `use` / `forget`) | `action`, `name`, `trigger?`, `session_id` |
| `injected` | A non-user message injected into the loop (schedule, background agent, skill, webhook, …) | `source_kind`, `source_label`, `content`, `session_id` |

Example of a few lines from a `jsonl` run (one object per physical line):

```json
{"type":"status","message":"Session created: my-session","session_id":"my-session"}
{"type":"tool_use","tool":"view","tool_id":"call_abc","server":"filesystem","params":{"path":"src/"},"session_id":"my-session"}
{"type":"tool_result","tool":"view","tool_id":"call_abc","server":"filesystem","content":"src/main.rs\nsrc/lib.rs","success":true,"session_id":"my-session"}
{"type":"assistant","content":"Recent changes refactored the session loop...","session_id":"my-session"}
{"type":"cost","session_tokens":1234,"session_cost":0.0025,"input_tokens":1000,"output_tokens":200,"cache_read_tokens":30,"cache_write_tokens":4,"reasoning_tokens":0,"session_id":"my-session"}
```

To get just the final answer text, filter for `assistant` lines, e.g. with `jq`:

```bash
echo "Summarize recent changes" | octomind run --format jsonl \
  | jq -r 'select(.type == "assistant") | .content'
```

## Streaming Programmatically (WebSocket & ACP)

If you want a live, bidirectional stream instead of a one-shot pipe, use one of the server modes:

- **WebSocket server** (`octomind server`) — emits the same `ServerMessage` event stream over a WebSocket. See [WebSocket Server](../integration/01-websocket-server.md) for the message protocol. Note that the session-init message (`session`) only carries an optional `session_id`; it does **not** accept a schema.
- **ACP protocol** (`octomind acp`) — the Agent Client Protocol integration for editors/clients. See [ACP Protocol](../integration/02-acp-protocol.md).

Both stream the structured events listed above; neither accepts a schema on session creation.

## Provider Compatibility (Structured Output Capability)

Whether a provider *can* be asked for native structured output is exposed by each provider's `supports_structured_output(model)`. This capability gates the `--schema` flag on `octomind run` (see [Schema Enforcement](#schema-enforcement---schema)) and the internal compression decision call. For reference, against the active `octolib 0.21.6`:

| Provider | `supports_structured_output` |
|----------|------------------------------|
| OpenAI | Yes (all models) |
| Google (Vertex) | Yes |
| Amazon (Bedrock) | Yes |
| Cloudflare | Yes |
| DeepSeek | Yes |
| OpenRouter | Per model's reference capabilities, else Yes |
| Anthropic | Trait default — per model's reference capabilities, else No |

When a schema is requested from a provider that returns `false` for the given model, Octomind fails fast:

```
Provider 'anthropic' does not support structured output for model '<model-without-reference-capabilities>'. Remove the schema parameter or use a compatible provider.
```

## Schema Enforcement (`--schema`)

Pass a JSON Schema **object** file to `octomind run` to constrain the assistant's replies:

```bash
echo "List the top 3 TODOs" | octomind run developer:general --format jsonl --schema todos.schema.json
```

- The schema applies to **every assistant reply** for the session's lifetime — across multi-turn sessions, resumes, and daemon mode. Tool calls still flow normally underneath; only the final text is constrained.
- The resolved model must support structured output (see the provider table above), or the run fails fast with the error shown there.
- Like `--model`, the schema is a **runtime override** — it is not persisted with the session, so pass it again when resuming.
- The file must contain a JSON Schema object; it is loaded and validated before session init. A ready-to-use example ships at [`config-templates/todos.schema.json`](../../config-templates/todos.schema.json).
- `--schema` exists only on `octomind run` — the WebSocket and ACP session-init messages do not accept a schema.

The same mechanism is also used **internally** by conversation compression: it checks the resolved `[compression.model]` provider via `supports_structured_output()`; if true, it sends a generated compression schema in strict mode, otherwise it falls back to an XML-style prompt. Omitting `[compression.model]` inherits the main profile. See [Context Compression](08-compression.md).

## Summary

- For machine-readable output, use `--format jsonl` on `octomind run` (or the WebSocket/ACP servers for live streaming).
- The JSONL/WebSocket/ACP streams emit typed `ServerMessage` events (`assistant`, `tool_use`, `tool_result`, `cost`, `status`, `error`, `mcp_notification`, `skill`, `injected`, `thinking`).
- To enforce a JSON Schema on the assistant's answer, pass `--schema <file>` to `octomind run` (structured-output-capable models only). The compression decision call uses the same mechanism internally.
