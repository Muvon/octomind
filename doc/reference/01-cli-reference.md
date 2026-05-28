# CLI Reference

Complete reference for all `octomind` CLI commands and flags.

## `octomind run [TAG] [MESSAGE]`

Start an interactive or non-interactive AI session.

| Flag | Short | Description |
|------|-------|-------------|
| `TAG` | | Agent tag or role name (e.g., `developer`, `octomind:assistant`). Uses `default` from config if omitted. |
| `--name` | `-n` | Named session identifier |
| `--resume` | `-r` | Resume a specific session by name |
| `--resume-recent` | | Resume the most recent session |
| `--format` | | Output format: `plain` (default for pipe), `jsonl` (structured JSON Lines) |
| `--model` | `-m` | Override model (`provider:model` format) |
| `--daemon` | | Daemon mode: stay alive after message, accept injected messages |
| `--sandbox` | | Restrict filesystem writes to working directory |
| `--hook` | | Activate webhook hook(s) by name. Can be specified multiple times. |

**Examples:**
```bash
# Interactive session with default role
octomind run

# Interactive with specific role
octomind run developer

# Tap agent
octomind run octomind:developer

# Non-interactive: pipe message via stdin
echo "Explain the auth module" | octomind run developer --format plain

# Named session
octomind run --name feature-auth

# Resume session
octomind run --resume feature-auth
octomind run --resume-recent

# Daemon mode with webhook
octomind run --name ci-watcher --daemon --format jsonl --hook github-push

# Model override
octomind run -m anthropic:claude-sonnet-4
```

## `octomind server [TAG]`

Start a WebSocket server for remote AI sessions.

| Flag | Short | Description |
|------|-------|-------------|
| `TAG` | | Agent tag or role name |
| `--host` | | Bind address (default: `127.0.0.1`) |
| `--port` | `-p` | Port (default: `8080`) |
| `--sandbox` | | Restrict filesystem writes to working directory |

**Examples:**
```bash
octomind server
octomind server --host 0.0.0.0 --port 9090
octomind server developer --sandbox
```

## `octomind acp [TAG]`

Run as Agent Client Protocol agent over stdio (for editor integration).

| Flag | Short | Description |
|------|-------|-------------|
| `TAG` | | Agent tag or role name |
| `--name` | `-n` | Session name (used when client creates a session) |
| `--resume` | `-r` | Resume a specific session by name |
| `--resume-recent` | | Resume the most recent session |
| `--model` | `-m` | Override model (`provider:model` format) |
| `--sandbox` | | Restrict filesystem writes to working directory |
| `--hook` | | Activate webhook hook(s) by name. Can be specified multiple times. |

**Examples:**
```bash
octomind acp developer
octomind acp context_gatherer --sandbox
octomind acp developer:general -m openai:gpt-4o
```

## `octomind tap [TAP] [PATH]`

Add or list registry taps (Homebrew-style agent sources).

| Argument | Description |
|----------|-------------|
| `TAP` | Tap identifier (`user/repo` format). Omit to list all taps. |
| `PATH` | Optional local path. If provided, symlinks instead of cloning from GitHub. |

**Examples:**
```bash
# List all taps
octomind tap

# Add tap from GitHub
octomind tap myorg/my-tap

# Add local tap (symlink)
octomind tap myorg/my-tap /path/to/local/tap
```

## `octomind untap <TAP>`

Remove a previously added tap.

```bash
octomind untap myorg/my-tap
```

## `octomind vars`

Show all placeholder variables and their current values.

| Flag | Short | Description |
|------|-------|-------------|
| `--preview` | `-p` | Show preview of placeholder values (3 lines) |
| `--expand` | `-e` | Show full expanded values for placeholders |

```bash
octomind vars
octomind vars --preview
octomind vars --expand
```

Displays: `{{CWD}}`, `{{ROLE}}`, `{{DATE}}`, `{{SHELL}}`, `{{OS}}`, `{{BINARIES}}`, `{{GIT_STATUS}}`, `{{README}}`, etc.

## `octomind send`

Send a message to a running daemon session by name.

| Flag | Short | Description |
|------|-------|-------------|
| `--name` | `-n` | Name of the running session to send to (required) |
| `MESSAGE` | | Message text. If omitted, reads from stdin. |

```bash
echo "Check build status" | octomind send --name ci-watcher
octomind send --name ci-watcher "Check build status"
```

## `octomind config [OPTIONS]`

Generate, validate, or display configuration.

| Flag | Description |
|------|-------------|
| `--model` | Set root-level model (`provider:model` format) |
| `--api-key` | Set API key (`provider:key` format) |
| `--log-level` | Set log level |
| `--mcp-providers` | Set MCP providers |
| `--mcp-server` | Add/configure MCP server |
| `--system` | Set custom system prompt (or `default` to reset) |
| `--markdown-enable` | Enable/disable markdown rendering |
| `--markdown-theme` | Set markdown theme |
| `--list-themes` | List available markdown themes |
| `--show` | Display current configuration with defaults |
| `--validate` | Validate configuration without making changes |
| `--upgrade` | Upgrade config file to latest version |

**Examples:**
```bash
# Generate default config
octomind config

# Show current settings
octomind config --show

# Validate config
octomind config --validate

# List themes
octomind config --list-themes
```

## `octomind workflow <FILE>`

Run a multi-step workflow defined in a TOML file.

| Flag | Short | Description |
|------|-------|-------------|
| `FILE` | | Path to workflow TOML file (required) |
| `--dry-run` | | Validate and print execution plan without spawning processes |

Reads input from stdin. Per-step progress, cost, and token stats go to stderr. The final step's output goes to stdout.

```bash
echo "build a JSON-to-CSV CLI in Rust" | octomind workflow myflow.toml
octomind workflow myflow.toml --dry-run
```

## `octomind completion <SHELL>`

Generate shell completion scripts.

| Argument | Description |
|----------|-------------|
| `SHELL` | Target shell: `bash`, `zsh`, `fish`, `powershell`, `elvish` |

| Shell | Command |
|-------|---------|
| Bash | `octomind completion bash > ~/.local/share/bash-completion/completions/octomind` |
| Zsh | `octomind completion zsh > ~/.zfunc/_octomind` |
| Fish | `octomind completion fish > ~/.config/fish/completions/octomind.fish` |
| PowerShell | `octomind completion powershell > octomind.ps1` |

## Global Flags

| Flag | Description |
|------|-------------|
| `--help` | Show help |
| `--version` | Show version |
