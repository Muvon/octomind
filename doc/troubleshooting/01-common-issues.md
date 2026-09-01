# Common Issues

Use these checks to diagnose installation, authentication, configuration, MCP, session, sandbox, and transport failures.

## Installation

### Binary Not Found

```
octomind: command not found
```

Ensure the binary is on your PATH:
```bash
which octomind
# If not found, add to PATH or move binary:
sudo mv octomind /usr/local/bin/
```

### Permission Denied

```bash
chmod +x /usr/local/bin/octomind
```

### Wrong Architecture

Download the correct binary for your platform. Use `uname -m` to check:
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
to replace an existing sign-in.

### Key Not Found

If a provider has no credentials, the run fails before the first request. You may
see an error similar to (the exact wording comes from the provider layer):

```
missing API key for provider 'openrouter'
```

Set the matching environment variable. The general pattern is `<PROVIDER>_API_KEY`:
```bash
export OPENROUTER_API_KEY="your_key"   # openrouter
export OPENAI_API_KEY="your_key"       # openai
export ANTHROPIC_API_KEY="your_key"    # anthropic
export DEEPSEEK_API_KEY="your_key"     # deepseek
```

A few configured providers use non-`*_API_KEY` variables:
- Google: `GOOGLE_APPLICATION_CREDENTIALS`
- Amazon: `AWS_ACCESS_KEY_ID`
- Cloudflare: `CLOUDFLARE_API_TOKEN`

For the full per-provider list see [Environment Variables](../reference/04-environment-variables.md).
The `cli:` meta-provider (e.g. local CLI-backed models) needs no API key.

Add to your shell profile for persistence, or use a `.env` file in your project
(it is loaded from the current directory and overrides the process environment).

Check current config:
```bash
octomind config --show
```

### Invalid Model Format

Setting a model without a provider prefix is rejected, for example:

```
model must be in provider:model format
```

Always use `provider:model` format:
```
openrouter:anthropic/claude-sonnet-4-6  # correct explicit provider:model
anthropic/claude-sonnet-4-6             # wrong — slash is not the provider delimiter
claude-sonnet-4-6                       # wrong — missing provider prefix
```

## Configuration

### Config Validation Fails

```bash
octomind config --validate
```

Common issues:
- Missing required fields (check against `config-templates/default.toml`)
- Invalid TOML syntax (check brackets, quotes, commas)
- Out-of-range field values. The validator enforces:
  - role `temperature` between `0.0` and `2.0`
  - role `top_p` between `0.0` and `1.0`
  - every model profile's `top_k` between `0` and `1000` (`0` disables it)
  - `max_session_tokens_threshold` at most `2,000,000` (`0` disables)
  - `cache_keepalive_max_idle_seconds` at most `86400` (24h; `0` = unbounded)
  - each MCP/webhook hook `timeout` between `1` and `3600` seconds
  - every resolved model profile's `name` must resolve to a configured provider

### Config Not Loading

Verify config location:
```bash
octomind config --show
```

Default: `~/.local/share/octomind/config/config.toml`

Octomind merges `config.toml` with every other `*.toml` file in the same
directory (`mcp-*.toml` files are applied last as overrides), so a stray `.toml`
left in that directory can change what gets loaded.

Override the config file path (this is a full path to a `.toml` file, not a
directory — its parent directory is then used to merge sibling `*.toml` files):
```bash
export OCTOMIND_CONFIG_PATH="/custom/path/config.toml"
```

If the config was written by an older Octomind version, migrate it to the
current schema:
```bash
octomind config --upgrade
```

## MCP Tools

### Tool Not Found

A tool is only available if its MCP server is configured *and* the active role
references that server. The shipped config declares four built-in servers
(`core`, `orchestration`, `runtime`, `agent`); the `filesystem` server (the `octofs` companion)
is provided by the default tap, not by a hand-written block. To confirm what is
loaded, run `/mcp list` in a session.

To wire up your own stdio server and grant a role access:

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
server_refs = ["core", "orchestration", "runtime", "filesystem", "agent", "octocode"]
allowed_tools = ["core:*", "orchestration:*", "runtime:*", "filesystem:*", "agent:*", "octocode:*"]
```

`server_refs` controls which servers the role can see. An empty
`allowed_tools` list is unrestricted; when the list is non-empty, it must also
grant each desired tool or a matching server wildcard.

### Server Not Responding

Inspect the MCP layer. `/mcp` accepts these subcommands:
`info` (the default), `list`, `full`, `health`, `dump`, and `validate`.

```
/mcp health     # connection status per server
/mcp list       # confirm the server's tools are registered
/mcp validate   # check tool parameter schemas
```

Turn on detailed logging to see startup/handshake errors:
```
/loglevel debug
```

For stdio servers, verify the command is on PATH (substitute your server's
binary):
```bash
which octocode
```

### Tool Permission Denied

Check `allowed_tools` in your role config. Patterns:
- `"core:*"` — all tools from the `core` server
- `"filesystem:view"` — one specific tool
- `[]` — empty list means no filtering (every tool from the referenced servers is allowed)

## Taps and Agents

### Agent Not Found

Tap agents are addressed as `category:variant` (for example `developer:general`).
If a tag is not found, list the taps that are active and confirm the category
and variant exist:
```bash
octomind tap            # list active taps (no URL = list mode)
```
The built-in default tap (`muvon/tap`) is always present as the last fallback,
so `developer:general` resolves out of the box. Add or remove taps with:
```bash
octomind tap myorg/my-agents     # clones github.com/myorg/octomind-my-agents
octomind untap myorg/my-agents   # remove a tap
```

### Manifest Placeholder Prompts

The first time you run a tap agent, its manifest may prompt for `{{INPUT:KEY}}`
values. Answers are persisted to `~/.local/share/octomind/inputs.toml` and reused.
`{{ENV:KEY}}` placeholders read from the environment (with `.env` fallback), and
`{{CWD}}` is the runtime working directory.

## Sessions

### Session Not Resuming

Sessions are stored in `~/.local/share/octomind/sessions/`. Check:
```
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

### High Token Usage

Monitor with:
```
/info
```

Reduce context:
```
/done               # Force compression and start a fresh task boundary
/run reduce         # Built-in command: compress session history (ships in default.toml)
```
Automatic compression is checked at API-call and tool-result boundaries when its trigger is eligible — see [Compression Guide](../usage/08-compression.md).

Enable automatic compression in config. See [Compression](../usage/08-compression.md).

### Cannot Send to a Running Session

`octomind send -n NAME "message"` talks to a session over a Unix domain socket
(`$XDG_RUNTIME_DIR/octomind/<name>.sock`, or `<system tmp>/octomind-<uid>/<name>.sock`
when that variable is unset; a named pipe `\\.\pipe\octomind-<name>` on Windows). If you get an error like:

```
no running session named 'NAME' (socket not found ...)
```

the target session is not running, or you used the wrong name. The target must
be a session started with `octomind run --daemon` (or a named interactive
session). The daemon loop is non-interactive: from a terminal, select it with
`--format`; with piped stdin, `--format` is optional:
```bash
octomind run --daemon --format jsonl -n my-daemon
```

## Platform-Specific

The sandbox (`--sandbox` flag or `sandbox` in config) applies to the `run`,
`server`, and `acp` commands only. Linux permits writes to the current working
directory and `~/.local/share`; macOS additionally permits the device and
platform temp paths named in `src/sandbox/macos.rs`. Credential directories
such as `~/.ssh` and `~/.aws` are write-protected. There is a platform
difference in read behavior:
- **macOS** (Seatbelt): also *blocks reads* of credential dirs (`~/.ssh`,
  `~/.gnupg`, `~/.aws`, `~/.kube`, `~/.config/gcloud`, `~/.azure`, `~/.config/op`).
- **Linux** (Landlock): the whole filesystem stays *readable*; only writes are
  denied. True read isolation is not possible with Landlock v1-v3, so credential
  dirs are write-protected but not read-protected.

### Linux: Sandbox Not Working

Landlock requires kernel 5.13+:
```bash
uname -r  # Check kernel version
```
On older kernels the sandbox runs in best-effort mode and logs a warning instead
of failing.

### macOS: Sandbox Permissions

Seatbelt may block certain operations (including reads of the credential dirs
listed above). Check Console.app for sandbox violations.

### Windows: Path Issues

The data root is `%LOCALAPPDATA%\octomind` (falls back to
`%USERPROFILE%\AppData\Local\octomind` when `LOCALAPPDATA` is unset). The config
file therefore lives at `%LOCALAPPDATA%\octomind\config\config.toml`. Ensure
backslashes in paths are escaped in TOML strings (or use forward slashes).
The OS-level sandbox is not implemented on Windows; requesting it logs a warning
and continues without write restrictions.

## Debug Mode

Enable detailed logging:

```
/loglevel debug
```

Or in config:
```toml
log_level = "debug"
```

Log locations:
- CLI: printed to terminal
- WebSocket: `~/.local/share/octomind/logs/websocket-debug.log`
- ACP: `~/.local/share/octomind/logs/acp-debug.log`
- ACP errors: `~/.local/share/octomind/logs/acp-errors.jsonl`

## See Also

- [Environment Variables](../reference/04-environment-variables.md) — full per-provider API-key list
- [MCP Tools](../usage/07-mcp-tools.md) — configuring MCP servers and tools
- [Local Tools](../usage/17-local-tools.md) — the built-in filesystem (octofs) tools
- [Compression](../usage/08-compression.md) — how automatic context compression works
