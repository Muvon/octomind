# Dynamic MCP Servers

Use the built-in `mcp` tool to connect and manage tool servers during a live session. This guide is for users who need
additional tools mid-task and want to decide which connections to keep for future sessions.

## Get Started

The `mcp` tool lives in the `runtime` builtin MCP server. Your role must include that server and allow `runtime:mcp` or
`runtime:*`; the template's `assistant` role allows `runtime:*`.

```bash
octomind run assistant
```

Ask the AI to inspect its servers before adding one:

```text
List your MCP servers and tell me which are enabled.
```

The AI calls `mcp` with this JSON argument object. The JSON examples below are tool arguments, not shell or slash
commands.

```json
{
  "action": "list"
}
```

The result separates `Configured servers:` from `Dynamic servers:`. Configured rows include a server type and an
`active` label; dynamic rows show enabled/disabled state. Both include the stored tool filter and persistence marker.
The configured `active` label is not a live health probe; use `/mcp health` to diagnose connections. Actual inventories
depend on your role, taps, and configuration.

## Connect a Server

`add` registers a server without connecting; `enable` connects, discovers its tools, and exposes them to the AI. Dynamic
registrations belong to the current session.

### Stdio Servers (Local Tools)

With `octocode` installed and available on the Octomind process's PATH, ask:

```text
Add a stdio MCP server named octocode using command octocode, args ["mcp", "--path=."],
and timeout_seconds 240. Then enable it and report the tool names returned by the server.
```

The corresponding `mcp` calls are:

```json
{
  "action": "add",
  "name": "octocode",
  "server_type": "stdio",
  "command": "octocode",
  "args": [
    "mcp",
    "--path=."
  ],
  "timeout_seconds": 240
}
```

```json
{
  "action": "enable",
  "name": "octocode"
}
```

This uses the Octocode launch arguments shown in the default config template. Do not assume a fixed number or set of
tools: use the names returned by `enable` or inspect `/mcp list`.

`timeout_seconds` is optional and defaults to **30**. For tool calls it is an idle deadline that resets on MCP progress.
Raise it only when a server cannot report progress or return a task for bounded long-running work.

### HTTP Servers

For an MCP server you already run at `http://localhost:3000/mcp`, use these arguments. Replace the URL with your
server's actual MCP endpoint; a regular REST API URL is not sufficient.

```json
{
  "action": "add",
  "name": "my_http_server",
  "server_type": "http",
  "url": "http://localhost:3000/mcp"
}
```

```json
{
  "action": "enable",
  "name": "my_http_server"
}
```

The dynamic tool does not accept `headers`, `env`, or `cwd`. If the server needs custom values for those fields, define
it in TOML instead. For example, place this in a config-directory `.toml` file and restart with the matching role.
Supply `MY_MCP_TOKEN` in the environment before starting Octomind.

```toml
[[mcp.servers]]
name = "my_http_server"
type = "http"
url = "http://localhost:3000/mcp"
headers = { Authorization = "Bearer {{ENV:MY_MCP_TOKEN}}" }
timeout_seconds = 30
tools = []
auto_bind = ["assistant"]
```

`auto_bind` matches the full role name exactly: `developer` does not match `developer:general`. See [Configuration
Reference](../reference/03-config-reference.md) for static server configuration.

### Tool Filtering

Pass the desired filter to `enable`, using actual names from the server's inventory. For example, if the server reports
`semantic_search`, this exposes only that tool:

```json
{
  "action": "enable",
  "name": "octocode",
  "tools": [
    "semantic_search"
  ]
}
```

A trailing `*` matches a name prefix; `tools: []` exposes the full inventory:

```json
{
  "action": "enable",
  "name": "octocode",
  "tools": [
    "semantic_*"
  ]
}
```

```json
{
  "action": "enable",
  "name": "octocode",
  "tools": []
}
```

The current dynamic connection path applies the filter supplied to `enable`. A `tools` value supplied to `add` is stored
in the server configuration but is not applied by dynamic `enable` on its own. Also, an `enable` filter is not written
back into that stored configuration: `persist` saves the registered configuration, not the temporary filter. For a
persistent restriction, include `tools` when you first register the server, and pass the same filter to `enable`. For
example, use this `add` in place of the unrestricted one above:

```json
{
  "action": "add",
  "name": "octocode",
  "server_type": "stdio",
  "command": "octocode",
  "args": [
    "mcp",
    "--path=."
  ],
  "timeout_seconds": 240,
  "tools": [
    "semantic_search"
  ]
}
```

Verify the next session's inventory after persisting.

## Persist or Disconnect a Server

If a server proves useful, persist it:

```json
{
  "action": "persist",
  "name": "octocode"
}
```

This writes `mcp-octocode.toml` in Octomind's config directory. It also works for config-loaded servers.

| State at persist time | Saved `auto_bind` |
|---|---|
| Enabled, with a current role | The exact current role name |
| Disabled | Cleared; explicit role `server_refs` can still select the server |

The config directory is `~/.local/share/octomind/config` on macOS/Linux and `%LOCALAPPDATA%/octomind/config` on Windows.
If `OCTOMIND_DATA_DIR` is set, it is `$OCTOMIND_DATA_DIR/config`. `persist` always writes there, even when
`OCTOMIND_CONFIG_PATH` makes startup load a different directory. Use the normal data directory when testing persistence:


```bash
OCTOMIND_DATA_DIR="$PWD/.octomind-data" octomind run assistant
```

Persisted `mcp-*.toml` files load after regular config files and override same-named servers. Disabling a server only
affects the live tool surface; it does not rewrite saved configuration:

```json
{
  "action": "disable",
  "name": "octocode"
}
```

To stop using the server and remove the saved override, call these separately:

```json
{
  "action": "unpersist",
  "name": "octocode"
}
```

```json
{
  "action": "remove",
  "name": "octocode"
}
```

`unpersist` deletes only `mcp-octocode.toml`; it does not disconnect the live server or remove declarations from other
config files. `remove` unregisters a dynamic entry and cleans up its process, but does not delete saved TOML.

## Common Questions

**Why can't I type `/mcp add`?** The `mcp` tool manages servers. The human `/mcp` command inspects them and supports
only `info` (the default), `list`, `full`, `health`, `dump`, and `validate`:

```text
/mcp
/mcp list
/mcp full
/mcp health
/mcp dump
/mcp validate
```

**Why did enabling fail or expose no tools?** Check the executable and arguments for stdio, or the real MCP endpoint and
authentication for HTTP. Use `/mcp health` and `/mcp full`, and compare filter strings with the returned tool names. If
`mcp` itself is missing, check the role's `server_refs` and `allowed_tools` for `runtime` access.

**Why did the server return next session?** `disable` and `remove` do not change saved config. Remove the persisted
override with `unpersist`, and check for other declarations or explicit role references.

## MCP Management Reference

| Action | Arguments beyond `action` | Effect |
|---|---|---|
| `list` | None | Show configured and dynamic servers with status |
| `add` | `name`, `server_type`; `command` for stdio, `url` for HTTP | Register without connecting; optional `args`, `tools`, `timeout_seconds` |
| `enable` | `name`, optional `tools` | Connect and activate discovered tools with the supplied filter |
| `disable` | `name` | Deactivate tools while keeping the registration |
| `remove` | `name` | Remove a dynamic registration; does not edit config files |
| `persist` | `name` | Save registered config, setting or clearing `auto_bind` |
| `unpersist` | `name` | Delete the corresponding `mcp-<name>.toml` override |

Dynamic agents have their own `agent` management tool; see [Multi-Agent Delegation](05-multi-agent-delegation.md) for
agent setup and execution.

Implementation: [runtime server management](../../src/mcp/runtime/dynamic.rs), [tool inventory and
filtering](../../src/mcp/server.rs), and [role/server selection](../../src/config/mcp.rs).

## See also

- [Configuration Reference](../reference/03-config-reference.md)
- [Session Commands](../reference/02-session-commands.md)
- [Multi-Agent Delegation](05-multi-agent-delegation.md)
