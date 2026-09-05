# Common Issues

Use these checks when installation, authentication, configuration, MCP tools, or sessions fail.
This guide also covers sandbox restrictions and ACP/WebSocket startup problems.

## Installation

### Binary Not Found

```text
octomind: command not found
```

Ensure the binary is on your PATH:

```bash
command -v octomind
# If you installed the binary in ~/.local/bin:
export PATH="$HOME/.local/bin:$PATH"
octomind --version
```

### Permission Denied

For a downloaded binary in `~/.local/bin`, make it executable:

```bash
chmod +x "$HOME/.local/bin/octomind"
```

### Wrong Architecture

Download the correct binary for your platform. On Linux/macOS, check the architecture:

```bash
uname -m
```

- `x86_64` / `amd64` — Intel/AMD
- `arm64` / `aarch64` — Apple Silicon / ARM

## API Keys

### Default OctoHub Sign-In

The shipped main, supervisor, and compression model profiles all default to
`octohub:auto`. Authenticate them with the device flow:

```bash
octomind login
```

The successful login stores the minted model-gateway credential as
`OCTOHUB_API_KEY` in Octomind's user-scope `.env`. Use `octomind login --force`
to replace an existing sign-in. On a machine without a browser, print the confirmation URL:

```bash
octomind login --force --no-browser
```

### Key Not Found

For a provider authentication error, check the variables named in the shipped configuration:

| Provider | Credential variable |
|---|---|
| OctoHub | `OCTOHUB_API_KEY` (written by `octomind login`) |
| OpenRouter | `OPENROUTER_API_KEY` |
| OpenAI | `OPENAI_API_KEY` |
| Anthropic | `ANTHROPIC_API_KEY` |
| DeepSeek | `DEEPSEEK_API_KEY` |
| Google | `GOOGLE_APPLICATION_CREDENTIALS` (credentials JSON path) |
| Amazon | `AWS_ACCESS_KEY_ID` |
| Cloudflare | `CLOUDFLARE_API_TOKEN` |

For example, substitute your provider-issued key:

```bash
export OPENROUTER_API_KEY="your-provider-issued-key"
```

See [Environment Variables](../reference/04-environment-variables.md) for provider-specific setup.
The load order is process environment → `<data-root>/config/.env` → launch-directory `.env`; later values win.
`OCTOMIND_CONFIG_PATH` does not relocate the user-scope `.env`. If a fresh login still appears ineffective,
check whether the project `.env` overrides `OCTOHUB_API_KEY`.

Check current config:

```bash
octomind config --show
```

### Invalid Model Format

Setting a model without a provider prefix is rejected, for example:

```text
model must be in provider:model format
```

Use a provider-qualified name when setting the model:

```bash
octomind config --model octohub:auto
```

For the current TOML profile shape and inheritance rules, see
[Model Profile and Provider Format](02-migration-guide.md#model-profile-and-provider-format).

## Configuration

### Config Validation Fails

```bash
octomind config --validate
```

Common causes are missing required fields, invalid TOML, or out-of-range values. Compare your file with
[the default template](../../config-templates/default.toml). Validation includes:

| Field | Accepted values |
|---|---|
| Every resolved model profile's `temperature` | `0.0`–`2.0` |
| Every resolved model profile's `top_p` | `0.0`–`1.0` |
| Every resolved model profile's `top_k` | `0`–`1000`; `0` disables it |
| Every resolved model profile's `name` | A name accepted by the provider factory |
| `max_session_tokens_threshold` | At most `2000000`; `0` disables the configured limit |
| `cache_keepalive_max_idle_seconds` | At most `86400`; `0` is unbounded |
| MCP server `timeout_seconds` | `1`–`3600` seconds |
| Webhook `timeout` | `1`–`3600` seconds |

### Config Not Loading

Inspect the loaded settings:

```bash
octomind config --show
```

On Linux/macOS the default is `~/.local/share/octomind/config/config.toml`.
See [Paths and logs](#paths-and-logs) for Windows and data-root overrides.
The path heading in `config --show` always shows the data-root default; it does not confirm a custom
`OCTOMIND_CONFIG_PATH`. To verify a custom selection, set the path explicitly when validating it as shown below.

Octomind merges `config.toml` with every other `*.toml` file in the same
directory: `config.toml` first, other files alphabetically, then `mcp-*.toml` overrides alphabetically.
A stray `.toml` backup can therefore change the loaded settings. Same-named array entries such as roles and
servers are replaced by the last entry, not merged field by field.

Override the config file path (this is a full path to a `.toml` file, not a
directory — its parent directory is then used to merge sibling `*.toml` files):

```bash
OCTOMIND_CONFIG_PATH="$HOME/.local/share/octomind/config/config.toml" octomind config --validate
```

Older configs are upgraded during startup before validation. For backups, manual upgrades, and recovery when loading
fails, follow the [Migration Guide](02-migration-guide.md).

## MCP Tools

### Tool Not Found

Select static servers through `server_refs` or an exact `auto_bind` match.
Interactive CLI sessions also receive `schedule` and `monitor`; that implicit grant does not add `tap`.
Runtime MCP tools and capabilities can add tools during a session. The shipped config declares four built-in
servers: `core`, `orchestration`, `runtime`, and `agent`. `filesystem` is an external companion server, not a builtin;
a role reference alone does not configure it. Check the resolved tool surface:

```text
/mcp list
/mcp full
```

With `octocode` installed, add this server and custom role to a sibling `reviewer.toml` in your config directory:

```toml
# Define the server (this stdio example is from default.toml)
[[mcp.servers]]
name = "octocode"
type = "stdio"
command = "octocode"
args = ["mcp", "--path=."]
timeout_seconds = 240
tools = []

# Reference it from a complete custom role
[[roles]]
name = "octocode-reviewer"
system = "Use the configured tools to inspect and review this project."
welcome = ""
[roles.mcp]
server_refs = ["octocode"]
allowed_tools = ["octocode:*"]
```

Start the custom role:

```bash
octomind run octocode-reviewer
```

Alternatively, add `auto_bind = ["developer:general"]` inside the server block to attach it to that exact role tag.
`"developer"` does not match `"developer:general"`. Auto-bound servers receive a server wildcard when the role's
allowlist is non-empty.

`server_refs` controls which static servers the role can see. An empty
`allowed_tools` list is unrestricted; when the list is non-empty, it must also
grant each desired tool or a matching server wildcard.

### Server Not Responding

Inspect the MCP layer:

| Command | Use |
|---|---|
| `/mcp` or `/mcp info` | Server overview |
| `/mcp list` | Tool names |
| `/mcp full` | Tool details and schemas |
| `/mcp health` | Server health checks |
| `/mcp dump` | Tool definitions for inspection |
| `/mcp validate` | Tool parameter-schema validation |

```text
/mcp health
/mcp list
/mcp validate
```

For startup/handshake errors, enable [debug logging](#debug-mode) before restarting the session.

For stdio servers, verify the command is on PATH (substitute your server's
binary):

```bash
which octocode
```

### Tool Permission Denied

Check `allowed_tools` in your role config:

| Pattern | Meaning |
|---|---|
| `"core:*"` | All available tools from `core` |
| `"filesystem:view"` | Only `view` from the configured `filesystem` server |
| `[]` | No allowlist filtering of selected servers |

Use the custom-role example above to grant `octocode:*`. To inspect which schemas actually reached the session:

```text
/mcp full
```

## Taps and Agents

### Agent Not Found

Tap agents are addressed as `category:variant` (for example `developer:general`).
If a tag is not found, list the taps that are active and confirm the category
and variant exist:

```bash
octomind tap            # list active taps (no URL = list mode)
```
The built-in default tap (`muvon/tap`) is always present as the last fallback,
but resolving a tag still requires its manifest and dependencies to be available.
For a repository you own, substitute its real owner and tap name:

```bash
octomind tap myorg/my-agents     # clones github.com/myorg/octomind-my-agents
octomind untap myorg/my-agents   # remove a tap
```

### Manifest Placeholder Prompts

The first time you run a tap agent, its manifest may prompt for `{{INPUT:KEY}}`
values. Answers are persisted to `<data-root>/inputs.toml` and reused.
`{{ENV:KEY}}` reads the environment; if unset, it prompts and saves the answer to the launch-directory `.env`.
An explicitly empty environment value is accepted. `{{CWD}}` is the runtime working directory.

If a delegated run reports missing input, run that role interactively once to supply its manifest values:

```bash
octomind run developer:general
```

Use the exact tag from the error if it names a different role.

## Sessions

### Session Not Resuming

Sessions are stored in `<data-root>/sessions/`. From an interactive session, check:

```text
/list
```

Resume by name:

```bash
octomind run --resume my-session
```

If you do not remember the name, resume the most recent session for the current
working directory:

```bash
octomind run --resume-recent
```

Bare `--resume` opens a session picker in a terminal; piped runs require a session name.
Without an explicit role tag, resume restores the saved role:

```bash
octomind run --resume
```

### Piped Input Fails

`run` takes an optional role/tag, not a message argument. Pipe the prompt; `--format` accepts `plain` or `jsonl`:

```bash
printf '%s\n' 'Explain the project structure.' | octomind run --format plain
```

An empty pipe fails with `No input provided via stdin`. With terminal stdin, `--format` requires piped input
unless you also use `--daemon`.

### High Token Usage

Monitor with:

```text
/info
```

Use `/done` to force compression across a task boundary. The shipped `/run reduce` command is an alternative
that replaces session history with its ACP role's output:

```text
/done
```

```text
/run reduce
```

Automatic compression is already enabled in the template. To restore its shipped trigger, set this key in the
existing `[compression]` table, before nested tables such as `[compression.model]`:

```toml
[compression]
threshold = 70000
```

`0` disables automatic compression; `/done` still bypasses its threshold/cooldown guards. Automatic checks run at
API-call and tool-result boundaries; eligibility does not guarantee an immediate fold. See
[Compression](../usage/08-compression.md) for the full behavior.

### Cannot Send to a Running Session

`send` requires a live session, not merely a saved session file. Start a named daemon from a terminal:

```bash
octomind run --daemon --format jsonl -n my-daemon
```

Then, in another terminal on the same host:

```bash
octomind send -n my-daemon "Summarize your current status."
```

`--daemon` alone stays interactive when stdin is a terminal. With piped stdin, `--format` is optional but the
initial prompt must be non-empty:

```bash
printf '%s\n' 'Wait for my next request.' | octomind run --daemon --format jsonl -n my-daemon
```

Interactive CLI sessions also start a message listener. If the socket is missing or connection fails, check the
session name, that the process is still running, and that both terminals use the same runtime directory.
On Unix, sockets live under `$XDG_RUNTIME_DIR/octomind` (if non-empty) or `<system tmp>/octomind-<uid>`.
Long session names are shortened and hashed to fit the socket path limit; use `send` rather than constructing a path.
Windows uses `\\.\pipe\octomind-<name>`.

## Transports

### Browser WebSocket Connection Returns 403

A handshake with an `Origin` header must match an allowed origin exactly. Native clients without that header
are exempt. For a browser app served at `http://localhost:3000`:

```bash
octomind server --host 127.0.0.1 --port 8080 --allow-origin http://localhost:3000
```

Repeat `--allow-origin` for additional origins. See [WebSocket Server](../integration/01-websocket-server.md)
for the client protocol.

### ACP Does Not Show an Interactive Prompt

ACP serves a client protocol over stdio; start an interactive CLI session with `run` instead. For an ACP client,
configure this executable invocation:

```bash
octomind acp
```

If the client fails to initialize, enable debug logging before launching it and inspect the ACP log files below.
See [ACP Protocol](../integration/02-acp-protocol.md) for protocol messages.

## Platform and Sandbox Troubleshooting

The sandbox (`--sandbox` or root `sandbox = true`) applies to `run`, `server`, and `acp`:

```bash
octomind run --sandbox
```

Run from the project directory you intend to modify.

| Platform | Filesystem policy |
|---|---|
| Linux (Landlock) | Reads remain unrestricted; writes are granted to cwd and existing `~/.local/share`. |
| macOS (Seatbelt) | Writes: cwd, `~/.local/share`, `/dev`, `/tmp`, `/private/tmp`, `/private/var/folders`. |
| Windows | Sandbox requests log a warning and continue without OS write restrictions. |

macOS also denies reads of `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.kube`, `~/.config/gcloud`, `~/.azure`, and
`~/.config/op`. On Linux these paths are only write-protected when outside the granted directories; launching
from your home directory grants writes throughout that directory. A custom `OCTOMIND_DATA_DIR` outside the
allowed paths does not automatically receive a write grant.

### Linux: Sandbox Not Working

Landlock requires kernel 5.13+:

```bash
uname -r  # Check kernel version
```
On older kernels the sandbox runs in best-effort mode and logs a warning instead
of failing.

### macOS: Sandbox Permissions

Seatbelt may block certain operations (including reads of the credential dirs
listed above). Check the configured file path and move writable project output beneath cwd.
The sandbox invocation above reproduces the same restrictions for child MCP processes.

### Windows: Path Issues

The default data root comes from Windows Local AppData, with a fallback to
`%USERPROFILE%\AppData\Local\octomind`. Ensure backslashes in TOML paths are escaped, or use literal strings.
For example, within an existing stdio MCP server block:

```toml
command = 'C:\Tools\octocode.exe'
```

## Debug Mode

Enable detailed logging:

```text
/loglevel debug
```

For startup diagnostics, set the root field in your existing config before restarting the CLI, ACP client, or server:

```toml
log_level = "debug"
```

### Paths and logs

`<data-root>` is `~/.local/share/octomind` on Linux/macOS and the Windows Local AppData `octomind` directory
on Windows. `OCTOMIND_DATA_DIR` overrides this root. Runtime sockets remain in the host runtime/temp directory.

| State | Location |
|---|---|
| Config | `<data-root>/config/config.toml`; `OCTOMIND_CONFIG_PATH` selects another file and its sibling directory |
| Shared credentials | `<data-root>/config/.env` |
| Sessions | `<data-root>/sessions/` |
| Manifest inputs | `<data-root>/inputs.toml` |
| CLI logs | Terminal output |
| WebSocket logs | `<data-root>/logs/websocket-debug.log` |
| ACP logs | `<data-root>/logs/acp-debug.log` |
| ACP errors | `<data-root>/logs/acp-errors.jsonl` |

For a separate data root, set it before launching Octomind:

```bash
export OCTOMIND_DATA_DIR="$HOME/octomind-state"
octomind config --show
```

Read ACP logs on Linux/macOS with:

```bash
tail -n 100 "${OCTOMIND_DATA_DIR:-$HOME/.local/share/octomind}/logs/acp-debug.log"
```

## See also

- [Environment Variables](../reference/04-environment-variables.md) — full per-provider API-key list
- [MCP Tools](../usage/07-mcp-tools.md) — configuring MCP servers and tools
- [Local Tools](../usage/17-local-tools.md) — executable project tools from `.agents/tools/`
- [Compression](../usage/08-compression.md) — how automatic context compression works
- [Migration Guide](02-migration-guide.md) — upgrades, backups, and recovery
