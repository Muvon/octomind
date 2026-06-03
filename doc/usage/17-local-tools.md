# Local Tools

Drop a shebang script into `<workdir>/.agents/tools/<name>` and it becomes an MCP tool — auto-discovered, role-agnostic, no config required. The script's leading comment block declares the schema; octomind converts it to a JSON-Schema tool definition the model can call.

This is the lightweight cousin of [Skills](15-skills.md). Skills inject **instructions**; local tools expose **executable actions**. Use local tools when the project itself wants to bolt on an action ("publish to staging", "check this lint rule", "fetch our internal status board") without touching the role config or shipping a full MCP server.

## Quickstart

```bash
mkdir -p .agents/tools
cat > .agents/tools/echo <<'EOF'
#!/usr/bin/env bash
# @description Echo a message back, optionally uppercased.
# @param *message string The text to echo
# @param uppercase boolean Uppercase the output

if [[ "${OCTOMIND_PARAM_UPPERCASE:-false}" == "true" ]]; then
  printf '%s\n' "$OCTOMIND_PARAM_MESSAGE" | tr '[:lower:]' '[:upper:]'
else
  printf '%s\n' "$OCTOMIND_PARAM_MESSAGE"
fi
EOF
chmod +x .agents/tools/echo

octomind run developer:general
```

In the session, the model now sees `echo` under server `local`. Run `/mcp list` (or `/mcp full` for the parameter schemas) to confirm it was discovered.

## File Contract

| Aspect | Rule |
|---|---|
| **Path** | `<workdir>/.agents/tools/<tool-name>` (no extension) |
| **Tool name** | The filename. Must match `[A-Za-z0-9_-]+`. Names beginning with `-` or `.` are also skipped, along with any other invalid characters (e.g. a `.` extension). |
| **Executable** | On Unix, must be `chmod +x`; non-executable files are skipped (logged at debug). On non-Unix platforms (e.g. Windows) the executable bit check is bypassed — any existing file is treated as runnable and the OS decides whether it can run. |
| **Shebang** | Line 1 may be a `#!...` shebang. Skipped during header parsing. Required for the OS to actually run the file. |
| **Header** | Leading comment block. Comment prefixes `#`, `//`, `--` are recognized. Parsing stops at the first non-comment, non-blank line, or after 80 lines. |

Any `#`-comment language works (bash, python, ruby, lua, perl, R, julia, awk, …). For `//` use Node/Deno (the interpreter strips the shebang itself); for `--` use Lua/Haskell/SQL-shell wrappers.

## Header Schema

```bash
#!/usr/bin/env bash
# @description Short summary the model sees in the tool list. Continuation
# lines without an @ tag append to the previous tag — keep multi-line
# descriptions readable.
# @param *target string Path to operate on
# @param force boolean Overwrite if the destination exists
# @param count integer Number of iterations
```

### Tags

| Tag | Required | Notes |
|---|---|---|
| `@description` (or `@desc`) | yes | Free text. Continuation lines (no leading `@`) append to it. |
| `@param NAME TYPE DESC` | repeatable | Declares a parameter. See below. |

Unknown tags are ignored with a debug log so the format can grow without breaking existing tools.

### Parameter syntax

```
@param [*]NAME TYPE DESCRIPTION...
```

- **Required** — prefix the name with `*` (e.g. `*target`). Mirrors how octomind renders required params in `/mcp full` output. The `*` must be attached directly to the name: `*target`, not `* target`.
- **Optional** — no prefix. This is the default.
- **TYPE** — one of `string`, `number`, `integer`, `boolean`, `array`, `object`. Common aliases (`str`→string, `int`→integer, `num`/`float`→number, `bool`→boolean, `list`→array, `obj`/`map`→object) are normalized. Unknown types fall back to `string`.
- **DESCRIPTION** — everything after the type, joined with single spaces. Shown in the tool's parameter docs.

A bare `*` with no name (e.g. `@param * string ...`), or any otherwise-malformed `@param` line, is silently skipped (logged at debug). If a parameter seems to vanish, check that the name is well-formed.

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

The model's harness validates against this schema before the tool runs — required-param enforcement, type checking, and per-tool documentation come for free.

## Calling Convention

When the model invokes the tool, octomind spawns the script with:

| Channel | Contains |
|---|---|
| **stdin** | JSON object of all params (`{"message":"hi","uppercase":true}`). One write, then EOF. |
| **env `OCTOMIND_PARAM_<UPPER>`** | Each param as a separate env var. Strings/numbers/bools become their natural string form; arrays/objects are JSON-stringified. |
| **env `OCTOMIND_TOOL_NAME`** | The tool name, in case one binary handles multiple. |
| **env `OCTOMIND_WORKDIR`** | The session's working directory (also `cwd`). |
| **stdout** | Result content shown to the model. |
| **stderr** | Non-empty stderr is appended to the result under an `[stderr]` marker — even on a successful (exit 0) run. Use stderr only for content you want the model to see; don't leak progress chatter there. |
| **exit code** | Non-zero → tool error. The message is `local tool '<name>' exited with status <code>`, followed (when non-empty) by an `[stderr]` block and then a separate `[stdout]` block. |

Pick whichever input style fits the language. Bash scripts usually read env vars; Python scripts often parse stdin JSON. Both arrive every call.

### Bash example (env-driven)

```bash
#!/usr/bin/env bash
# @description Greet someone politely.
# @param *who string Person to greet
# @param shout boolean Yell the greeting
set -euo pipefail
greeting="Hello, ${OCTOMIND_PARAM_WHO}"
[[ "${OCTOMIND_PARAM_SHOUT:-false}" == "true" ]] && greeting="${greeting^^}!"
printf '%s\n' "$greeting"
```

### Python example (stdin JSON)

```python
#!/usr/bin/env python3
# @description Sum a list of integers.
# @param *values array JSON list of integers, e.g. [1,2,3]
import json, sys
params = json.load(sys.stdin)
print(sum(params["values"]))
```

### Node example

```javascript
#!/usr/bin/env node
// @description Capitalize a string.
// @param *text string Input text
let buf = '';
process.stdin.on('data', d => buf += d);
process.stdin.on('end', () => {
  const { text } = JSON.parse(buf || '{}');
  console.log(text.toUpperCase());
});
```

## Discovery & Lifecycle

- **When**: every turn. Discovery is a `read_dir` of `<workdir>/.agents/tools/` plus a header parse per file. Cheap, no caching needed.
- **Where**: the **session's current working directory**. If the workdir tool changes the directory mid-session, the next turn's tool list reflects the new location.
- **Always-on**: appended to every role's tool list automatically. There is no `[mcp.servers]` entry to add and no `allowed_tools` filter — local tools are role-agnostic by design (matches the `OCTOMIND_SKILLS` shape, but driven by file presence rather than env).
- **Lowest priority on collision**: if a local tool's name matches a config-defined or dynamic tool, the config/dynamic tool wins. You can't accidentally hijack `shell` by naming a script `shell`.
- **Hot reload**: edit a file and save — the next tool call sees the new schema/body. No session restart needed.

## Errors and Edge Cases

| Symptom | Cause | Fix |
|---|---|---|
| Tool doesn't appear under server `local` in `/mcp` | Not executable (Unix) | `chmod +x .agents/tools/<name>` |
| Tool doesn't appear under server `local` in `/mcp` | Header missing `@description` | Add the line; debug log shows `parse … failed: missing @description` |
| Tool doesn't appear under server `local` in `/mcp` | Filename has `.` (e.g. `mytool.sh`) or starts with `-`/`.` | Drop the extension and leading punctuation — `mytool` |
| Tool returns non-JSON garbage | Script writes binary on stdout | Stick to text; or base64-encode |
| Tool times out | Hard 5-minute (300s) cap — not configurable for local tools | Make the script faster or split into multiple calls |
| Param values look wrong | Forgot `*` and the script assumed required | Add `*` to the name in the header |

To see why specific files were skipped during discovery, raise the log level to debug — set config `log_level = "debug"`, use the `/loglevel debug` session command, pass `--log-level debug` on the CLI, or set `RUST_LOG`. The debug logs show non-executable files, header parse failures, and malformed or unknown tags.

## Security Notes

Local tools are **arbitrary code on disk** — by definition they run with the same privileges as octomind. The intent is "the project author wrote these scripts and committed them to the repo." Treat `.agents/tools/` like `package.json` `scripts:` or a `Makefile`: trust the source.

If you check out a third-party project that ships local tools, audit them before running an octomind session there. The same auto-discovery that makes the feature pleasant also means malicious files run on first use.

## Comparison

| Need | Use |
|---|---|
| Inject domain *instructions* into context | [Skills](15-skills.md) |
| One-off project-specific *action* (publish, lint, fetch internal data) | **Local tools** |
| Reusable cross-project tool with schema, prompts, multi-step logic | Author a tap with an MCP server |
| Tool that needs a long-lived process | External `stdio`/`http` MCP server, configured in `[[mcp.servers]]` |

Local tools are deliberately the simplest layer — one file, one shebang, one header. Reach for the heavier mechanisms only when you outgrow them.
