# Quickstart

Get from zero to a productive AI session in 5 minutes.

## 1. Install

```bash
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash
```

## 2. Set API Key

```bash
export OPENROUTER_API_KEY="your_key"
```

Get a key at [openrouter.ai](https://openrouter.ai/). OpenRouter gives you access to many AI providers through a single key.

## 3. Generate Config

```bash
octomind config
```

Creates `~/.local/share/octomind/config/config.toml` with sensible defaults.

## 4. Start a Session

```bash
octomind run
```

You're now in an interactive AI session with full tool access. The AI can read files, edit code, run commands, and manage your project.

With no tag, `octomind run` uses the configured default tag `assistant:concierge` -- a tap agent from the built-in default tap `muvon/tap` -- on the default model `openrouter:anthropic/claude-sonnet-4`. The first run clones that tap (a one-time `git` fetch over the network), so the very first session needs connectivity.

## What You Can Do

**Ask questions about your code:**
```
How does the authentication system work?
```

**Make changes:**
```
Add error handling to the login function
```

**Run commands:**
```
Run the test suite and fix any failures
```

**Attach images for analysis:**
```
/image screenshot.png
```

## Essential Commands

| Command | Description |
|---------|-------------|
| `/help` | Show all commands |
| `/info` | Token usage and costs |
| `/image <path>` | Attach image for AI analysis |
| `/done` | Finalize the current task: compress context, run learning extraction, and produce a summary |
| `/clear` | Clear the terminal screen |
| `/copy` | Copy the last response to the clipboard |
| `/exit` | Exit session (or press `Ctrl+D`) |

`/done` is the most useful command for a productive first session: when you finish a task, it summarizes what happened and compresses the context so the next task starts clean.

## Session Modes

The tag you pass to `octomind run` can be one of two things:

- **A plain name** (e.g. `assistant`) -- a local role defined in your `config.toml`. Resolved offline, no network access.
- **A `category:variant` tag** (e.g. `developer:general`) -- a tap agent fetched from the registry. The first use clones the tap over the network.

```bash
# Default: interactive with the assistant:concierge tap agent
octomind run

# Built-in local "assistant" role (full tool access -- reads, edits, runs commands)
octomind run assistant

# Named session (resume later)
octomind run --name my-feature

# Resume a session
octomind run --resume my-feature

# Tap agent (fetches a specialized config from the registry)
octomind run developer:general
```

The built-in `assistant` role is NOT chat-only -- it ships with full MCP tool access (core, runtime, filesystem, agent). If you want a chat-only experience, use the tap agent `octomind:assistant` instead (see [Roles](06-roles.md)). For more on tap agents and the registry, see the [Tap System](../integration/04-tap-system.md).

## Non-Interactive Mode

`--format` is available only on `octomind run`. Passing any `--format` value forces non-interactive mode: the prompt is read from stdin and the result is printed to stdout. The only accepted values are `plain` and `jsonl`.

Pipe a message via stdin and get a plain-text response:

```bash
echo "Explain the auth module" | octomind run developer:general --format plain
```

Structured line-delimited JSON output for pipelines:

```bash
echo "List TODO items" | octomind run developer:general --format jsonl
```

> Use a resolvable tag here. A bare `developer` is treated as a local role name; since the default config has no such role, it logs `Unknown role` and silently falls back to another role -- so always use the tap form `developer:general` (or a real local role like `assistant`).

## Next Steps

- [Configuration](03-configuration.md) -- customize settings
- [Providers](04-providers.md) -- set up AI providers
- [Sessions](05-sessions.md) -- session management
- [MCP Tools](07-mcp-tools.md) -- available tools
