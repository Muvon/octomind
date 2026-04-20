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
| `/exit` | Exit session |

## Session Modes

```bash
# Default: interactive with tools
octomind run

# Chat-only (assistant role)
octomind run assistant

# Named session (resume later)
octomind run --name my-feature

# Resume a session
octomind run --resume my-feature

# Tap agent (fetches specialized config)
octomind run octomind:developer
```

## Non-Interactive Mode

Send a single message and get a response:

```bash
octomind run developer "Explain the auth module" --format plain
```

Structured JSON output for pipelines:

```bash
octomind run developer "List TODO items" --schema todos.json --format jsonl
```

## Next Steps

- [Configuration](03-configuration.md) -- customize settings
- [Providers](04-providers.md) -- set up AI providers
- [Sessions](05-sessions.md) -- session management
- [MCP Tools](07-mcp-tools.md) -- available tools
