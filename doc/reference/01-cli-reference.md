# CLI Reference

Complete reference for all `octomind` CLI commands and flags.

## Synopsis

```bash
octomind <COMMAND> [OPTIONS]
```

A subcommand is **required** — running bare `octomind` prints a usage error. There is no default action.

| Command | Purpose |
|---------|---------|
| `run` | Start an interactive or non-interactive AI session (the main command). |
| `server` | Start a WebSocket server for remote sessions. See [WebSocket Server](../integration/01-websocket-server.md). |
| `acp` | Run as an Agent Client Protocol agent over stdio for editor integration. See [ACP Protocol](../integration/02-acp-protocol.md). |
| `config` | Create, validate, display, or upgrade configuration. See [Config Reference](03-config-reference.md). |
| `tap` | Add a registry tap (agent source) or list active taps. |
| `untap` | Remove a previously added tap. |
| `vars` | Show placeholder variables and their resolved values. |
| `send` | Inject a message into a running named session. |
| `workflow` | Run a multi-step workflow defined in a TOML file. See [Workflows](../usage/09-workflows.md). |
| `completion` | Generate shell completion scripts. |

The global config file lives at `~/.local/share/octomind/config/config.toml` on macOS and Linux
(`%LOCALAPPDATA%\octomind\config\config.toml` on Windows). Override the path with `OCTOMIND_CONFIG_PATH`.

### TAG resolution

`run`, `server`, and `acp` take an optional `TAG`:

- A **role name** (e.g. `developer`) — matched against `[[roles]]` in your config.
- A **registry agent tag** in `category:variant` form (e.g. `developer:general`) — resolved through your installed [taps](../integration/04-tap-system.md).
- Omitted — uses the default role from config.

Model selection priority, highest first: `--model` (CLI) > the role/agent `model` field > root `config.model`. For a `role:tag` registry agent, a `[taps]` entry overrides the `config.model` tier.

## `octomind run [TAG]`

Start an interactive or non-interactive AI session.

| Flag | Short | Description |
|------|-------|-------------|
| `TAG` | | Role name (e.g. `developer`) or registry agent tag `category:variant` (e.g. `developer:general`). Uses the default role from config if omitted. |
| `--name` | `-n` | Named session identifier |
| `--resume` | `-r` | Resume a specific session by name |
| `--resume-recent` | | Resume the most recent session for the current directory |
| `--format` | | Output format: `plain` or `jsonl`. Unset by default — see note below. |
| `--model` | `-m` | Override model (`provider:model` format) |
| `--daemon` | | Keep the session alive for injected messages. Implies non-interactive mode — pair with `--format` (e.g. `--format jsonl`) and deliver messages with `octomind send`. |
| `--sandbox` | | Restrict filesystem writes to the working directory. See [Sandbox](#sandbox). |
| `--hook` | | Activate webhook hook(s) by name (defined in `[[hooks]]` config). Repeatable. See [Daemon & Hooks](../integration/03-daemon-and-hooks.md). |

**Interactivity and `--format`:** `--format` is unset by default. If it is omitted and stdin is a TTY, the
session runs **interactively**. If `--format` is given (`plain` or `jsonl`) **or** stdin is piped, the session
runs **non-interactively**, reading the input from stdin. Internally, an unset format resolves to `plain`.

**Daemon mode:** `--daemon` is non-interactive and effectively requires `--format` — when a daemon is launched
attached to a terminal, the piped input is forced empty. While the session is alive, inject further messages with
[`octomind send --name <name>`](#octomind-send). See [Daemon & Hooks](../integration/03-daemon-and-hooks.md).

**Examples:**
```bash
# Interactive session with default role
octomind run

# Interactive with specific role
octomind run developer

# Registry agent (category:variant)
octomind run developer:general

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
| `TAG` | | Role name or registry agent tag `category:variant` |
| `--host` | | Bind address (default: `127.0.0.1`) |
| `--port` | `-p` | Port (default: `8080`) |
| `--sandbox` | | Restrict filesystem writes to the working directory. See [Sandbox](#sandbox). |

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
| `TAG` | | Role name or registry agent tag `category:variant` |
| `--name` | `-n` | Session name (used when client creates a session) |
| `--resume` | `-r` | Resume a specific session by name |
| `--resume-recent` | | Resume the most recent session |
| `--model` | `-m` | Override model (`provider:model` format) |
| `--sandbox` | | Restrict filesystem writes to the working directory. See [Sandbox](#sandbox). |
| `--hook` | | Activate webhook hook(s) by name. Repeatable. |

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
| `--preview` | `-p` | Show a short preview (up to 3 lines) of each placeholder value |
| `--expand` | `-e` | Show full expanded values for placeholders |

With no flag, `vars` runs in **list** mode (names + descriptions). If both flags are given, `--expand`
takes precedence over `--preview`.

```bash
octomind vars
octomind vars --preview
octomind vars --expand
```

Displays the placeholder set `octomind vars` reports: `{{DATE}}`, `{{SHELL}}`, `{{OS}}`, `{{BINARIES}}`, `{{CWD}}`,
`{{HOME}}`, `{{SYSTEM}}` (complete system info), `{{CONTEXT}}` (README + git status + git tree),
`{{GIT_STATUS}}`, `{{GIT_TREE}}`, and `{{README}}`. (`{{ROLE}}` is substituted in role prompts but is **not** among the values `vars` lists.)

## `octomind send`

Inject a message into a running named session.

Works against any running session that has started its inject listener (typically a session launched with
`--daemon`, but not exclusively). The message reaches the session over a per-OS transport:

- **Unix:** a Unix domain socket at `<run_dir>/<name>.sock` (run dir is `~/.local/share/octomind/run/`).
- **Windows:** a named pipe `\\.\pipe\octomind-<name>`.

The session replies `ok` on successful delivery; any other reply is treated as an error and reported.

| Flag | Short | Description |
|------|-------|-------------|
| `--name` | `-n` | Name of the running session to send to (required) |
| `MESSAGE` | | Message text. If omitted, reads from stdin. |

```bash
echo "Check build status" | octomind send --name ci-watcher
octomind send --name ci-watcher "Check build status"
```

## `octomind config [OPTIONS]`

Create, validate, display, or upgrade configuration. With no flags, see the example note below.

**Mutating flags** (apply changes, then save the config file):

| Flag | Description |
|------|-------------|
| `--model` | Set root-level model (`provider:model` format). |
| `--log-level <none\|info\|debug>` | Set the log level (case-insensitive); any other value errors. |
| `--mcp-providers <a,b,c>` | **Replace** the MCP server list: clears all configured servers, then adds each named one as a `builtin` server (timeout 30s). |
| `--mcp-server <name,key=value,...>` | Add or update one MCP server. See [format](#--mcp-server-format) below. |
| `--system` | Set a custom system prompt, or pass `default` to reset to the built-in prompt. |
| `--markdown-enable` | Enable or disable markdown rendering. |
| `--markdown-theme` | Set the markdown theme (must be one of the themes from `--list-themes`). |

**Inspect / maintenance flags** (no save):

| Flag | Description |
|------|-------------|
| `--show` | Display the current configuration with all defaults filled in. |
| `--validate` | Validate the configuration without making changes. |
| `--list-themes` | List the available markdown themes. |
| `--upgrade` | Upgrade the config file to the latest schema version. |

**Note on `--api-key`:** the parser accepts an `--api-key provider:key` argument, but it is **always rejected** at
runtime — API keys can never be stored in the config file for security reasons. Set the provider's environment
variable instead, e.g. `export OPENROUTER_API_KEY=...`. See the [Config Reference](03-config-reference.md) for the
full list of provider environment variables.

#### `--mcp-server` format

`--mcp-server name,key=value,...` — the first comma-separated token is the server name; the rest are `key=value`
pairs:

| Key | Meaning |
|-----|---------|
| `type` | `http`, `stdio`, or `builtin` (default `http`). |
| `url` | Endpoint URL — **required** for `http`. |
| `command` | Executable to launch — **required** for `stdio`. |
| `args` | Space-separated arguments for a `stdio` command. |
| `timeout` / `timeout_seconds` | Request timeout in seconds (default `30`). |

```bash
# HTTP server
octomind config --mcp-server "search,url=http://localhost:9000,timeout=60"

# stdio server
octomind config --mcp-server "files,type=stdio,command=octofs,args=--root ."
```

**Examples:**
```bash
# Create a default config (only if none exists; otherwise reports
# the current state with no changes — it does NOT regenerate).
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
| `--dry-run` | | Validate and print the execution plan without running any steps |

Reads input from stdin. **All** output — per-step assistant responses, progress, cost, and token stats — is
written to **stderr**. **stdout** receives output only for `--dry-run` (the execution plan); the workflow itself
produces no stdout result stream. To capture a step's text, consume stderr (e.g. `2>flow.log`). See
[Workflows](../usage/09-workflows.md).

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

Dynamic agent-tag completion for `octomind run <TAB>` is injected only into the **bash**, **zsh**, and **fish**
scripts (they call an internal `octomind complete` helper at completion time). PowerShell and Elvish scripts are
emitted as-is and provide static completion only.

## Sandbox

When enabled, the sandbox restricts all filesystem writes to the current working directory (Landlock on Linux,
Seatbelt on macOS). It is active if **either** the config `sandbox` setting **or** the `--sandbox` flag is set, and
it applies only to `run`, `server`, and `acp` — all other subcommands ignore both.

## Global Flags

| Flag | Short | Description |
|------|-------|-------------|
| `--help` | `-h` | Show help |
| `--version` | `-V` | Show version |
