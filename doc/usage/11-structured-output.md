# Structured Output

Octomind supports enforcing structured JSON output via schemas. Useful for automation, CI/CD pipelines, and machine-readable responses.

> **Note:** Schema-based structured output is available programmatically via the WebSocket server and ACP protocol. It is not exposed as a CLI `--schema` flag.

## How It Works

When a schema is provided (via WebSocket/ACP protocol):
1. Schema is loaded and validated as JSON
2. Schema is passed to the provider using its native structured output API
3. AI is constrained to respond with JSON matching the schema
4. Response is returned as raw JSON (markdown rendering disabled)
5. Strict mode is always enabled: non-conforming responses are rejected

## Usage

### Define a Schema

```json
{
  "type": "object",
  "properties": {
    "summary": { "type": "string" },
    "issues": {
      "type": "array",
      "items": { "type": "string" }
    },
    "severity": { "type": "string", "enum": ["low", "medium", "high"] }
  },
  "required": ["summary", "issues", "severity"],
  "additionalProperties": false
}
```

### Non-Interactive Mode (WebSocket/ACP)

```bash
# Via WebSocket protocol — send schema in the session initialization message
# See WebSocket server documentation for the protocol details
```

### Interactive Session

Schema can be set programmatically via the WebSocket or ACP protocol during session initialization. It is not available as a CLI flag.

### Pipeline Integration

```bash
# Use --format jsonl for structured JSON output in pipelines
echo "Summarize recent changes" | octomind run developer --format jsonl
```

## Provider Compatibility

Not all providers support structured output. Octomind fails fast with a clear error if unsupported:

```
Provider 'cloudflare' does not support structured output for model 'llama-3.1-8b-instruct'.
Remove the schema parameter or use a compatible provider.
```

| Provider | Support | Notes |
|----------|---------|-------|
| OpenAI | Yes | `gpt-4o`, `gpt-4o-mini`, recent models |
| Anthropic | Yes | Tool-based JSON mode |
| OpenRouter | Varies | Depends on underlying model |
| Google | No | |
| Amazon | No | |
| Cloudflare | No | |
| DeepSeek | No | |

## Notes

- Schema is used only for the top-level session response
- Layers and compression always use `schema: None`
- When structured output is present, it takes precedence over `content` for display
- The `structured_output` field in provider response carries the parsed JSON value
