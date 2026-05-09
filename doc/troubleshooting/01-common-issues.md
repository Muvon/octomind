# Common Issues

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
- `x86_64` / `amd64` -- Intel/AMD
- `arm64` / `aarch64` -- Apple Silicon / ARM

## API Keys

### Key Not Found

```
API key not found for provider 'openrouter'
```

Set the environment variable:
```bash
export OPENROUTER_API_KEY="your_key"
```

Add to shell profile for persistence. Or use a `.env` file in your project.

Check current config:
```bash
octomind config --show
```

### Invalid Model Format

```
Invalid model format. Expected 'provider:model'
```

Always use `provider:model` format:
```
openrouter:anthropic/claude-sonnet-4    # correct
anthropic/claude-sonnet-4               # wrong
claude-sonnet-4                         # wrong
```

## Configuration

### Config Validation Fails

```bash
octomind config --validate
```

Common issues:
- Missing required fields (check against `config-templates/default.toml`)
- Invalid TOML syntax (check brackets, quotes, commas)
- Invalid field values (temperature > 2.0, negative thresholds)

### Config Not Loading

Verify config location:
```bash
octomind config --show
```

Default: `~/.local/share/octomind/config/config.toml`

Override with:
```bash
export OCTOMIND_CONFIG_PATH="/custom/path/"
```

## MCP Tools

### Tool Not Found

Ensure the server is configured and the role has access:

```toml
# Server must be defined
[[mcp.servers]]
name = "filesystem"
type = "stdio"
command = "octofs"
timeout_seconds = 30

# Role must reference it
[[roles]]
name = "developer"
[roles.mcp]
server_refs = ["core", "filesystem"]
allowed_tools = ["core:*", "filesystem:*"]
```

### Server Not Responding

Check health:
```
/mcp health
```

Check logs:
```
/loglevel debug
```

For stdio servers, verify the command is on PATH:
```bash
which octofs
```

### Tool Permission Denied

Check `allowed_tools` in your role config. Patterns:
- `"core:*"` -- all core tools
- `"filesystem:view"` -- specific tool
- `[]` -- all tools (no restrictions)

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

### High Token Usage

Monitor with:
```
/info
```

Reduce context:
```
/done               # Force compression and start a fresh task boundary
/run reduce         # Custom reduce command (if configured)
```
Automatic compression also runs in the background once token thresholds are reached — see [Compression Guide](../usage/08-compression.md).

Enable automatic compression in config. See [Compression](../usage/08-compression.md).

### Spending Limit Hit

```
Session spending threshold exceeded
```

Adjust or disable:
```toml
max_session_spending_threshold = 10.0   # Increase limit
max_session_spending_threshold = 0.0    # Disable limit
```

## Platform-Specific

### Linux: Sandbox Not Working

Landlock requires kernel 5.13+:
```bash
uname -r  # Check kernel version
```

### macOS: Sandbox Permissions

Seatbelt may block certain operations. Check Console.app for sandbox violations.

### Windows: Path Issues

Use `%LOCALAPPDATA%/octomind/` for data directory. Ensure backslashes in paths are escaped in config.

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
