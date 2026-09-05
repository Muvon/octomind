# Local Tools

Use local tools to expose project scripts as MCP actions without adding server configuration. This guide covers the
script header, inputs and outputs, discovery, and failure handling.

## Quickstart

```bash
mkdir -p .agents/tools
cat > .agents/tools/echo <<'EOF'
#!/usr/bin/env bash
# @description Echo a message back, optionally uppercased.
# @param *message string The text to echo
# @param uppercase boolean Uppercase the output

set -euo pipefail
: "${OCTOMIND_PARAM_MESSAGE:?message is required}"

if [[ "${OCTOMIND_PARAM_UPPERCASE:-false}" == "true" ]]; then
  printf '%s\n' "$OCTOMIND_PARAM_MESSAGE" | tr '[:lower:]' '[:upper:]'
else
  printf '%s\n' "$OCTOMIND_PARAM_MESSAGE"
fi
EOF
chmod +x .agents/tools/echo

octomind run developer:general
```

In the session, the model now sees `echo` under server `local`. Run `/mcp list` (or `/mcp full` for the parameter
schemas) to confirm it was discovered:

```text
/mcp full
Call the echo tool with message "hello" and uppercase true.
```

The `echo` tool’s arguments are:

```json
{"message":"hello","uppercase":true}
```

Its successful result is `HELLO`. To check the script directly without a model:

```bash
printf '%s' '{"message":"hello","uppercase":true}' | \
  OCTOMIND_PARAM_MESSAGE=hello OCTOMIND_PARAM_UPPERCASE=true .agents/tools/echo
```

## File Contract

| Aspect | Rule |
|---|---|
| **Path** | `<workdir>/.agents/tools/<tool-name>` (no extension) |
| **Tool name** | The filename. Must match `[A-Za-z0-9_-]+`. Names beginning with `-` or `.` are also skipped, along with any other invalid characters (e.g. a `.` extension). |
| **Executable** | On Unix, must be `chmod +x`; non-executable files are skipped (logged at debug). On non-Unix platforms (e.g. Windows) the executable bit check is bypassed — any existing file is treated as runnable and the OS decides whether it can run. |
| **Shebang** | Line 1 may be a `#!...` shebang. Skipped during header parsing. Use an interpreter shebang for Unix text scripts; Windows direct spawning does not supply a Bash/shebang adapter. |
| **Header** | Leading comment block. Comment prefixes `#`, `//`, `--` are recognized. Parsing stops at code, at the first blank line after comments begin, or after 80 lines. |

Use a language with a supported comment prefix and an installed interpreter. The examples below use Bash, Python, and
Node; direct script execution depends on your platform.

## Header Schema

```bash
#!/usr/bin/env bash
# @description Short summary the model sees in the tool list. Continuation
# lines without an @ tag continue @description — keep multi-line
# descriptions readable.
# @param *target string Path to operate on
# @param force boolean Overwrite if the destination exists
# @param count integer Number of iterations
```

### Tags

| Tag | Required | Notes |
|---|---|---|
| `@description` (or `@desc`) | yes | Free text. Continuation lines (no leading `@`) append to it. |
| `@param NAME TYPE DESC` (or `@arg`) | repeatable | Declares a parameter. See below. |

Unknown tags are ignored with a debug log so the format can grow without breaking existing tools.

### Parameter syntax

```text
@param [*]NAME TYPE DESCRIPTION...
```

- **Required** — prefix the name with `*` (e.g. `*target`). Mirrors how octomind renders required params in `/mcp full`
  output. The `*` must be attached directly to the name: `*target`, not `* target`.
- **Optional** — no prefix. This is the default.
- **TYPE** — one of `string`, `number`, `integer`, `boolean`, `array`, `object`. Common aliases (`str`→string,
  `int`→integer, `num`/`float`→number, `bool`→boolean, `list`→array, `obj`/`map`→object) are normalized. Unknown types
  fall back to `string`.
- **DESCRIPTION** — everything after the type, joined with single spaces. Shown in the tool's parameter docs.

A bare `*` with no name (e.g. `@param * string ...`), or an empty `@param` line, is silently skipped (logged at debug).
If a parameter seems to vanish, check that the name is well-formed.

Example with all flavors:

```bash
# @param *target string  Required path argument
# @param force boolean   Optional flag (no * prefix)
# @param count integer   Optional, default behavior left to the script
# @param tags array      Optional list, JSON-encoded on stdin
```

### Schema generation

The header is converted to a standard JSON-Schema tool definition:

```json
{
  "name": "echo",
  "description": "Echo a message back, optionally uppercased.",
  "parameters": {
    "type": "object",
    "properties": {
      "message":   {"type": "string",  "description": "The text to echo"},
      "uppercase": {"type": "boolean", "description": "Uppercase the output"}
    },
    "required": ["message"]
  }
}
```

The schema tells the model which values to send. The local executor does not validate required fields or types before
spawning the script: validate inputs in the script itself. Optional parameters have no generated defaults; arrays have
no item schema, and objects have no nested schema in this header format.

## Calling Convention

When the model invokes the tool, octomind spawns the script with:

| Channel | Contains |
|---|---|
| **stdin** | JSON object of all params (`{"message":"hi","uppercase":true}`). One write, then EOF. |
| **env `OCTOMIND_PARAM_<UPPER>`** | Each param as a separate env var. Strings/numbers/bools become their natural string form; arrays/objects are JSON-stringified; `null` becomes an empty string. Only supplied keys are assigned. |
| **env `OCTOMIND_TOOL_NAME`** | The tool name, in case one binary handles multiple. |
| **env `OCTOMIND_WORKDIR`** | The session's working directory (also `cwd`). |
| **stdout** | Result content shown to the model. |
| **stderr** | Non-whitespace stderr is appended to the result under an `[stderr]` marker — even on a successful (exit 0) run. Use stderr only for content you want the model to see; don't leak progress chatter there. |
| **exit code** | Non-zero → tool error. The message is `local tool '<name>' exited with status <code>`, followed (when non-empty) by an `[stderr]` block and then a separate `[stdout]` block. |

Pick whichever input style fits the language. Bash scripts usually read env vars; Python scripts often parse stdin JSON.
The JSON payload and supplied parameter variables arrive on each call. The child also inherits the parent environment,
so avoid exporting `OCTOMIND_PARAM_*` globally. No positional arguments are supplied.

### Bash example (env-driven)

Save this as `.agents/tools/greet` and make it executable, as in the quickstart.

```bash
#!/usr/bin/env bash
# @description Greet someone politely.
# @param *who string Person to greet
# @param shout boolean Yell the greeting
set -euo pipefail
greeting="Hello, ${OCTOMIND_PARAM_WHO}"
if [[ "${OCTOMIND_PARAM_SHOUT:-false}" == "true" ]]; then
  greeting="$(printf '%s' "$greeting" | tr '[:lower:]' '[:upper:]')!"
fi
printf '%s\n' "$greeting"
```

### Python example (stdin JSON)

Save this as `.agents/tools/sum-values`:

```python
#!/usr/bin/env python3
# @description Sum a list of integers.
# @param *values array JSON list of integers, e.g. [1,2,3]
import json
import sys
params = json.load(sys.stdin)
values = params.get("values")
if not isinstance(values, list) or any(type(value) is not int for value in values):
    print("values must be a list of integers", file=sys.stderr)
    sys.exit(1)
print(sum(values))
```

For example, after saving it:

```bash
chmod +x .agents/tools/sum-values
printf '%s' '{"values":[1,2,3]}' | .agents/tools/sum-values
```

The result is `6`.

### Node example

Save this as `.agents/tools/capitalize` and make it executable:

```javascript
#!/usr/bin/env node
// @description Capitalize a string.
// @param *text string Input text
let buf = '';
process.stdin.on('data', d => buf += d);
process.stdin.on('end', () => {
  const { text } = JSON.parse(buf || '{}');
  if (typeof text !== 'string') {
    console.error('text must be a string');
    process.exitCode = 1;
    return;
  }
  console.log(text.toUpperCase());
});
```

```bash
chmod +x .agents/tools/greet .agents/tools/capitalize
printf '%s' '{"text":"hello"}' | .agents/tools/capitalize
```

## Discovery & Lifecycle

- **When**: whenever Octomind rebuilds the available function list, and again when routing/executing a local tool.
  Discovery is a `read_dir` plus header parsing; there is no cache.
- **Where**: the **session's current working directory**. If the workdir tool changes the directory mid-session, the
  next turn's tool list reflects the new location.
- **Always-on**: appended to every role's tool list automatically. There is no `[[mcp.servers]]` entry to add and no
  `allowed_tools` filter — local tools are role-agnostic by design (matches the `OCTOMIND_SKILLS` shape, but driven by
  file presence rather than env).
- **Lowest priority on collision**: if a local tool's name matches a config-defined or dynamic tool, the config/dynamic
  tool wins. You can't accidentally hijack `shell` by naming a script `shell`.
- **Hot reload**: edit and save — execution re-reads the script; the next function-list rebuild advertises the updated
  schema. No session restart needed.

## Errors and Edge Cases

| Symptom | Cause | Fix |
|---|---|---|
| Tool doesn't appear under server `local` in `/mcp` | Not executable (Unix) | `chmod +x .agents/tools/<name>` |
| Tool doesn't appear under server `local` in `/mcp` | Header missing `@description` | Add the line; debug log shows `parse … failed: missing @description` |
| Tool doesn't appear under server `local` in `/mcp` | Filename has `.` (e.g. `mytool.sh`) or starts with `-`/`.` | Drop the extension and leading punctuation — `mytool` |
| Output has replacement characters | Binary/non-UTF-8 stdout is decoded lossily | Return UTF-8 text; JSON is optional |
| Tool times out | 300-second output-wait timeout after stdin delivery; not configurable for local tools | Make the script faster or split into multiple calls |
| Param values look wrong | Script assumed presence/type without validating it | Mark required schema fields and validate inputs in the script |

The 300-second timer wraps waiting for output, not spawn retries or stdin writes. On timeout/cancellation the direct
child is killed on drop; descendant process-tree cleanup is not guaranteed. Keep scripts bounded and read stdin when
accepting large JSON payloads.

To see why specific files were skipped during discovery, raise the log level with config `log_level = "debug"`, the
`/loglevel debug` session command, or tracing configuration. There is no global `octomind --log-level` flag;
`--log-level` belongs to `octomind config` and persists the setting.

```text
/loglevel debug
/mcp full
/loglevel info
```

To persist debug logging for later starts:

```bash
octomind config --log-level debug
```

## Security Notes

Local tools are **arbitrary code on disk** — by definition they run with the same privileges as octomind. The intent is
"the project author wrote these scripts and committed them to the repo." Treat `.agents/tools/` like `package.json`
`scripts:` or a `Makefile`: trust the source.

If you check out a third-party project that ships local tools, audit them before allowing the model to call them.
Discovery only advertises schemas; a selected tool script then runs with Octomind's process privileges.

## Comparison

| Need | Use |
|---|---|
| Inject domain *instructions* into context | [Skills](15-skills.md) |
| One-off project-specific *action* (publish, lint, fetch internal data) | **Local tools** |
| Reusable cross-project tool with schema, prompts, multi-step logic | Author a tap with an MCP server |
| Tool that needs a long-lived process | External `stdio`/`http` MCP server, configured in `[[mcp.servers]]` |

## Source reference

| Surface | Source |
|---------|--------|
| Header, schema, discovery, execution | [src/mcp/core/local_tool.rs](../../src/mcp/core/local_tool.rs) |
| Registration and routing | [src/mcp/tool_map.rs](../../src/mcp/tool_map.rs), [src/mcp/mod.rs](../../src/mcp/mod.rs) |
| Inspection and logging commands | [src/session/chat/session/commands/mcp.rs](../../src/session/chat/session/commands/mcp.rs), [src/commands/config.rs](../../src/commands/config.rs) |

## See also

- [Skills](15-skills.md)
- [MCP tools](07-mcp-tools.md)
- [Guardrails](18-guardrails.md)
- [Tap system](../integration/04-tap-system.md)
