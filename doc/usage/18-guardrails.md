# Guardrails

Use `.agents/guardrails.toml` to transform session input, deny matching tool calls, and run feedback scripts. This guide
is for project authors configuring those rules and checking their execution boundaries.

For semantic checks against the user's request, optionally enable the supervisor
[authorizer](14-supervisor.md#tool-authorization). It runs after native pre-call checks, before tools execute,
and can feed grounded candidate leads into the existing learning/evolution lifecycle.

## Get started

From the project root, create an input-transform script:

```bash
mkdir -p .agents scripts
cat > scripts/prepare-input.sh <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' 'Project rule: distinguish observed results from assumptions.'
cat
EOF
chmod +x scripts/prepare-input.sh
```

Add this entry to `.agents/guardrails.toml` (create the file if it does not exist):

```toml
[[pipe]]
name = "prepare"
command = "./scripts/prepare-input.sh"
roles = ["developer"]
```

Start a new session to load it:

```bash
printf '%s\n' 'Explain the project rule added to this request.' | octomind run developer:general --format jsonl
```

The pipe prepends the rule to input. The examples below are alternatives or additions to that file; do not combine
multiple catch-all pipes, because a message may match at most one. Script examples assume Unix and their named
interpreter is installed; make each file executable after saving it. `command` and `script` are executable paths, not
shell command strings with arguments.

## Overview

| Section | Phase | What it does | Side effect |
|---|---|---|---|
| `[[pipe]]` | Pre-model | Transform or validate user input before the model sees it | Non-zero exit → hard stop; stdout replaces user message |
| `[[guard]]` | Pre-call | Block a tool from running | Synthetic error result returned to the model |
| `[[hook]]` | Post-result | Run a script against the tool result | Non-zero exit → script stdout pushed to the session inbox (delivered as system-managed feedback on a subsequent request) |
| `[[validator]]` | End-of-turn | Run a script after the assistant's final message | Non-zero exit → `<validation>`-wrapped stdout pushed to the session inbox |

All four live in the same file. Nothing is mandatory; missing file = no authored rules. Opt-in generated evolution rules
may still be appended.

**Who can stop vs who can only nudge:** only `[[pipe]]` and `[[guard]]` can *block* — a pipe stops the message before
the model sees it, a guard stops a tool call before it runs. `[[hook]]` and `[[validator]]` can only *nudge*: their
feedback does not undo the tool result; they push a message to the inbox that the model reads on the next turn.

## File location

`<workdir>/.agents/guardrails.toml` — loaded fresh at session start. Parse errors are printed to stderr and treated as
no authored rules; a broken file never crashes the session.

Rules and runtime history are session-scoped. Edits are not hot-reloaded. Start a new session to reload rules and reset
history. `/done` also reloads authored/generated rules when learning is enabled, but does not reset the call log,
validator cursors, pipe counts, or message counter.

## Matching DSL

Used inside `match` (guards, hooks) and inside `when` entries (guards, validators):

```text
capability                       # any call to that capability
capability(regex)                # regex matched against full args JSON
capability(arg_name=regex)       # regex matched against a specific arg
```

- **capability** = the exact MCP capability name resolved from tap manifests or the runtime overlay (e.g. `shell`,
  `filesystem-read`, `filesystem-write`). Matching is equality, not a prefix or fuzzy check: a tool name or MCP server
  name works only if it is also the resolved capability's exact name. Tools with no owning capability never match.
- **regex on full args JSON** = the call's params object serialized to JSON, then matched. Use for any-arg patterns.
- **arg-targeted** = regex matched against just that arg's value. String args matched directly (no quotes);
  arrays/objects/numbers matched against their JSON form. Example: `paths=secret` matches `paths=["a","b/secret.env"]`
  because the haystack becomes `["a","b/secret.env"]`.

> **Two namespaces, one word.** The word "capability" appears in three places with **two distinct meanings**, so be
> careful:
> - In `match` / `when` targets and in the hook payload (`capability` field / `OCTOMIND_CAPABILITY` env) it means a
> **capability name** — the resolved tap-capability that owns the tool (e.g. `shell`, `filesystem-read`).
> - In `[[guard]] has` it means an **MCP server name** — the name of a configured `[[mcp.servers]]` entry active for the
> role (e.g. `core`, `runtime`, `agent`, `filesystem`). These are *not* capability names, so `has = "filesystem-read"`
> works only if that is also an actual configured server name.

### How a tool call resolves to a capability

`match` / `when` targets and the hook `capability`/`OCTOMIND_CAPABILITY` value all use a **capability name**. Octomind
resolves a call's `(server, tool)` pair to that name as follows:

1. **Static tap manifests first** — a `(server, tool) → capability` map is built from every installed capability's
  `allowed_tools`. A capability "owns" a tool when its `allowed_tools` lists `server:tool` (exact) or `server:*`
  (wildcard). Exact `(server, tool)` entries take precedence over `server:*`; the first discovered owner wins within
  each map.
2. **Runtime overlay fallback** — static-server tool extras contributed by runtime capability activation are checked
  next, using exact tool names.
3. **No owner → no match.** A tool without either lookup entry resolves to no capability and cannot match a DSL target.
  Directly configured tools can still have an owner if an installed capability declares their `(server, tool)` pair.

So the capability name for, say, the `view` tool is whatever tap capability lists `filesystem:view` (or `filesystem:*`)
in its `allowed_tools` — not the server name `filesystem`. `/mcp full` shows server/tool names, not the resolved
capability ownership map. Inspect installed capability provider files and, for a live observation, a hook payload’s
`capability` value. The hook below works without a `match` filter, including for local tools that have no capability
owner:

```toml
[[hook]]
on = "any"
script = ".agents/hooks/show-owner"
```

```bash
mkdir -p .agents/hooks
cat > .agents/hooks/show-owner <<'EOF'
#!/usr/bin/env python3
import json
import sys
payload = json.load(sys.stdin)
print(json.dumps({key: payload[key] for key in ("tool", "capability")}), file=sys.stderr)
sys.exit(1)
EOF
chmod +x .agents/hooks/show-owner
```

Run with `/loglevel debug` and make a tool call. This diagnostic script exits 1 so the hook runner logs stderr; its
empty stdout prevents inbox feedback. Remove the diagnostic hook when finished.

```text
/loglevel debug
Use an available read-only tool to inspect the project directory.
/loglevel info
```

## `when` conditions (signed list)

Used by `[[guard]]` (session-wide history) and `[[validator]]` (since-last-run history). Each entry is one DSL target
with a sign prefix:

```toml
when = [
  "+filesystem-write",                  # was used
  "-shell(command=cargo test)",         # was NOT used
]
```

- `+target` = at least one matching call exists in the relevant history window.
- `-target` = no matching call exists in the relevant history window.

All `when` items are AND'd. History is the session call log of allowed calls, recorded before execution; blocked calls
do not enter it, while a later tool error does not erase an allowed call.

## `pipe` guardrail: Pre-model Input Transform

Runs a matching script on the raw user input **before the model sees it**. The script receives the user message on
stdin; its stdout replaces the message sent to the model. Non-zero exit is a hard stop — the message is not sent to the
model and an error is displayed.

At most one `[[pipe]]` may match per user message; multiple matches are an error.

```toml
[[pipe]]
name    = "prepare"                        # required: identifier
command = "./scripts/prepare-input.sh"       # required: path relative to workdir
match   = ".*"                              # optional: regex on user message text
when    = "any"                             # optional: "first" | "any" (default)
roles   = ["developer"]                    # optional: role filter
```

### Semantics

- Evaluated on every user message (subject to filters).
- Filter evaluation order (cheapest first): `roles` → `when` → `match`.
- At most one pipe may match per message. If two or more pipes match, an error is displayed and the message is not sent
  to the model.
- The pipe runs in the session's working directory.
- Output-wait timeout: 300 seconds (same as hooks and validators); stdin is written concurrently.

### Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | yes | identifier, used in error messages and `PIPE_NAME` env var |
| `command` | path | yes | script path, relative to workdir (or absolute) |
| `match` | regex | no | regex on user message text; empty = matches all messages |
| `when` | enum | no | `"first"` (first message only) or `"any"` (default, every message) |
| `roles` | list of strings | no | role filter; exact (`developer:general`) or domain prefix (`developer` matches `developer:general`; `developer:*` is not a glob) |

### Script contract

| Channel | Use |
|---|---|
| **cwd** | session workdir |
| **stdin** | raw user message text |
| **env** | `OCTOMIND_ROLE`, `OCTOMIND_WORKDIR`, `PIPE_NAME`, `PIPE_RUN_COUNT`, `SESSION_MESSAGE_COUNT` |
| **stdout** | replaces the user message (used as-is, no trimming) |
| **stderr** | logged at debug level; used in the rejection error on non-zero exit |
| **exit 0** | stdout becomes the new user message |
| **exit ≠ 0** | hard stop — error displayed, message not sent to model |
| **timeout** | 300 s; killed → hard stop |

### Environment variables

| Variable | Description |
|---|---|
| `OCTOMIND_ROLE` | current session role (e.g. `developer:general`) |
| `OCTOMIND_WORKDIR` | session working directory path |
| `PIPE_NAME` | the `name` field from the `[[pipe]]` entry |
| `PIPE_RUN_COUNT` | number of times this pipe has been invoked in this session (starts at `1`) |
| `SESSION_MESSAGE_COUNT` | number of entries through the pipe-processing path, including the current attempt even if no pipe matches or it is rejected; not a count of slash commands |

### Example: validate input format

```toml
[[pipe]]
name    = "validate"
command = "./scripts/validate-input.sh"
```

Save this as `scripts/validate-input.sh` and run `chmod +x scripts/validate-input.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
input=$(cat)
if [[ "$input" != Task:* ]]; then
  printf '%s\n' 'Start the request with Task: followed by the work to do.' >&2
  exit 1
fi
printf '%s\n' "$input"
```

With this pipe replacing the quickstart pipe, try:

```bash
printf '%s\n' 'Task: Explain the project layout.' | octomind run developer:general --format jsonl
```

Known slash commands are dispatched separately; a pipe is not a way to forbid `/exit` or `/done`.

### Example: enrich first message with context

Reuse the quickstart script with this pipe instead of the catch-all entry:

```toml
[[pipe]]
name    = "context-enricher"
command = "./scripts/prepare-input.sh"
when    = "first"
roles   = ["developer"]
```
---

## `[[guard]]` — pre-call deny rules

```toml
[[guard]]
match   = "shell(command=^rm\\s+-rf?)"   # required: DSL target on the call
has     = "filesystem"                   # optional: MCP server must be in merged config
when    = ["-filesystem-read"]           # optional: history filter
message = "rm -rf blocked."              # required: shown to the model
```

### Semantics

- Evaluated in declaration order; **first match wins**.
- All conditions AND'd. Rule fires only when:
  - `match` target matches the current call, AND
  - every `has` entry is the name of an MCP **server** in the merged role config, AND
  - every `when` item is satisfied against the session call log.
- **`has` uses MCP server names, not capability names.** It is checked against the set of configured `[[mcp.servers]]`
  entries active for the current role (e.g. `core`, `runtime`, `agent`, `filesystem`) — *not* the capability vocabulary
  used by `match`/`when`. The check reads the merged config’s server names; it does not verify a live connection or
  include every dynamically registered server.
- When the rule fires, the call is **blocked** before the executor runs. The model receives a synthetic tool error:
  `[guardrail] <message>`.

### Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `match` | DSL target | yes | the call to match |
| `has` | string or list | no | configured-MCP-**server**-name filter (e.g. `core`, `filesystem`); empty = no filter. Not a capability name. |
| `when` | list of `+/-target` | no | history filter; empty = no filter |
| `message` | string | yes | the text the model sees |

### Example: tiered shell policy

These examples require installed capability ownership names `shell` and `filesystem-read`, and a configured server named
`filesystem` where `has` is used. They match the literal argument text, not shell semantics; they do not cover every
equivalent destructive command.

```toml
[[guard]]
match   = "shell(command=^rm\\s+-rf?\\s+/)"
message = "Refusing rm -rf on root paths."

[[guard]]
match   = 'shell(command=git push.*(?:--force(?:\s|$)|-f(?:\s|$)))'
message = "Force push blocked. Use --force-with-lease and ask first."

[[guard]]
match   = "shell(command=^ls\\b)"
has     = "filesystem"
when    = ["-filesystem-read"]
message = "Use the available file-listing tool instead of ls."
```

`has = "filesystem"` checks that the `filesystem` MCP server is in the merged role config; `when = ["-filesystem-read"]`
checks that the `filesystem-read` *capability* has not been used yet in this session — two different namespaces working
together.

### Performance

Guards evaluate **in batch, in arrival order, before any tool spawns**. Each allowed call is recorded into the session
log so the next call in the same batch sees it via `when`, even before the earlier call has executed or succeeded.
Blocked calls never reach the executor — no time is wasted.

---

## `[[hook]]` — post-result scripts

```toml
[[hook]]
match  = "shell(command=^cargo build)"   # optional: filter on the call
result = "error\\[E\\d+\\]"              # optional: regex on result text
on     = "any"                           # optional: "success" | "error" | "any"
script = ".agents/hooks/cargo-summary.sh"   # required: path relative to workdir
```

### Semantics

- Fires after each tool result lands, after the ordinary tool batch completes and before truncation/condensation and
  model delivery.
- All matching hooks fire (no first-match-wins). Multiple hooks compose.
- Skipped for guardrail-blocked tools — their synthetic result is not a real result.
- Script runs with the tool context on stdin and environment. Exit 0 = no-op; exit ≠ 0 = stdout pushed to the session
  inbox (delivered as system-managed feedback on a subsequent request). Stdout is trimmed first; if it is empty after
  trimming, nothing is pushed even on a non-zero exit.

### Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `match` | DSL target | no | call filter; empty/whitespace = any tool (skipped, treated as no filter) |
| `result` | regex | no | regex on the result text; `result = ""` compiles to a match-everything regex (unlike `match`, which skips empty), so it matches any result incl. empty |
| `on` | enum | no | `success`, `error`, or `any` (default) |
| `script` | path | yes | relative to workdir or absolute |

### Script contract

| Channel | Use |
|---|---|
| **cwd** | session workdir |
| **stdin** | JSON `{capability, tool, tool_id, params, result, success}` (`capability` = resolved capability name, or `null` if the tool isn't owned by any capability) |
| **env** | `OCTOMIND_CAPABILITY`, `OCTOMIND_TOOL`, `OCTOMIND_SUCCESS=1\|0`, `OCTOMIND_WORKDIR` |
| **stdout** | pushed to the session inbox if exit ≠ 0; trimmed first |
| **stderr** | debug-logged for non-zero exits, never injected; exit-0 stderr is not logged |
| **exit 0** | no-op |
| **exit ≠ 0** | trimmed stdout → inbox; if empty after trimming, nothing is pushed |
| **timeout** | 300 s; killed → no inject |

### Example: parse cargo errors after every build

```toml
[[hook]]
match  = "shell(command=cargo (build|test|check))"
result = "error\\["
script = ".agents/hooks/cargo-summary.sh"
```

Save this as `.agents/hooks/cargo-summary.sh` (requires `jq`) and make it executable:

```bash
#!/usr/bin/env bash
set -euo pipefail
payload=$(cat)
errors=$(printf '%s' "$payload" | jq -r '.result' | grep -oE 'error\[E[0-9]+\]' | sort -u || true)
[ -z "$errors" ] && exit 0
printf '%s\n' "Build emitted: $errors. Inspect the diagnostics before continuing."
exit 1
```

```bash
chmod +x .agents/hooks/cargo-summary.sh
printf '%s' '{"capability":"shell","tool":"shell","tool_id":"build-1","params":{"command":"cargo check"},"result":"error[E0308]: mismatched types","success":false}' | .agents/hooks/cargo-summary.sh
```

This direct invocation should print the diagnostic message and exit 1; it does not run Cargo.

---

## `[[validator]]` — end-of-turn scripts

```toml
[[validator]]
name   = "test-before-done"
match  = "(?i)\\b(done|finished|completed)\\b"   # optional: regex on assistant message
when   = [                                        # optional: tool-history filter
  "+filesystem-write",
  "-shell(command=cargo test)",
]
roles  = ["developer"]                            # optional: role filter
script = ".agents/validators/remind-tests.sh"
```

### Semantics

- Fires once at the end of the assistant turn (after the model produces its final message with no further tool calls).
- Per-validator cursor into the session call log. `when` is evaluated against `call_log[cursor..]` — i.e. **calls since
  this validator last ran**. On run, cursor advances to `call_log.len()`.
- All filters AND'd. None set → fires every turn (skill-like).
- Guardrail validators run at the end of **every** assistant turn (subject only to the `roles`/`when`/`match` filters).
  They are a separate system from skill `SKILL.md` validate scripts and are **not** gated by the
  `[skills].auto_validation` config flag — that flag only controls skill validators.
- On exit ≠ 0, stdout is trimmed, then wrapped as `<validation validator="<name>">…</validation>` and pushed to the
  session inbox. If stdout is empty after trimming, nothing is pushed even on a non-zero exit.

### Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | yes | unique identifier, used for the cursor and the XML tag |
| `match` | regex | no | matched against the assistant's final message text |
| `when` | list of `+/-target` | no | history check vs the slice since last run |
| `roles` | list of strings | no | role filter; exact (`developer:general`) or domain prefix (`developer` matches `developer:general`; `developer:*` is not a glob) |
| `script` | path | yes | relative to workdir or absolute |

### Filter short-circuit order (cheapest first)

1. **role filter** — skip the validator entirely if the current session role isn't in the list.
2. **`when` filter** — slice the call log from this validator's cursor, run `+used / -unused` checks.
3. **`match` regex** — run the regex over the assistant's final message text.

Only validators that pass all three filters spawn their script. The cursor advances **when the async execution task is
scheduled**, before OS process-spawn success is known. A spawn failure, timeout, or non-zero exit still consumes that
history window.

### Script contract

| Channel | Use |
|---|---|
| **cwd** | session workdir |
| **stdin** | JSON `{validator, role, assistant_text, triggered_by:[{capability,params}, …]}` |
| **env** | `OCTOMIND_VALIDATOR`, `OCTOMIND_ROLE`, `OCTOMIND_WORKDIR` |
| **stdout** | trimmed, then wrapped + pushed to inbox if exit ≠ 0 |
| **stderr** | debug-logged for non-zero exits; exit-0 stderr is not logged |
| **exit 0** | no-op |
| **exit ≠ 0** | `<validation validator="<name>">stdout</validation>` → inbox; if stdout is empty after trimming, nothing is pushed |
| **timeout** | 300 s |

`triggered_by` lists the calls in the since-last-run slice that matched a `+used` target. When the validator has no
`+used` targets configured, it instead contains **every** call in that slice — i.e. everything that happened since this
validator last ran — so an always-on validator still sees the full window of activity.

### Example: nudge to check after edits

History cannot prove that tests passed: the negative shell condition below means no matching call was attempted, not no
successful run. The script asks the model to respect the user’s verification instructions.

```toml
[[validator]]
name   = "test-after-edit"
when   = ["+filesystem-write", "-shell(command=cargo test)"]
script = ".agents/validators/remind-tests.sh"
```

Save this as `.agents/validators/remind-tests.sh` and make it executable:
```bash
#!/usr/bin/env bash
printf '%s\n' "Edits were attempted without a recorded cargo test call. Follow the user’s verification instructions and report validation limits."
exit 1
```

```bash
mkdir -p .agents/validators
# Save remind-tests.sh above in this directory before running chmod.
chmod +x .agents/validators/remind-tests.sh
```

### Example: always-on response check

```toml
[[validator]]
name   = "validation-statement"
script = ".agents/validators/validation-statement"
```

No filters means it runs at every turn end through the shared response path. Save this complete script as
`.agents/validators/validation-statement`:

```python
#!/usr/bin/env python3
import json
import sys
payload = json.load(sys.stdin)
if "validation" not in payload["assistant_text"].lower():
    print("Include a validation statement describing what you checked and what remains unverified.")
    sys.exit(1)
```

```bash
chmod +x .agents/validators/validation-statement
printf '%s' '{"validator":"validation-statement","role":"developer:general","assistant_text":"Changed the docs.","triggered_by":[]}' | .agents/validators/validation-statement
```

This direct check emits feedback and exits 1; include a validation statement to obtain exit 0. Guardrail validators have
no `[skills].max_retries` cap, so design scripts to stop emitting feedback once the issue is addressed.

---

## How history works (call log)

For ordinary tool batches, the session maintains a single ordered call log: `Vec<(capability, params)>`. Every
**allowed** tool call is appended before execution, so a call remains in history even if the external tool later returns
an error. Blocked calls are not recorded because they never execute.

- `[[guard]]` `when` reads the entire log (session-wide history).
- `[[validator]]` `when` reads `log[cursor..]` where the cursor is per-validator.
- `[[hook]]` doesn't use `when` — it's a per-result reaction.

These conditions track attempted calls, not outcomes. A `+shell(command=cargo test)` condition can be satisfied by a
failing test or an earlier allowed call in the same batch. Use actual script checks or tool results to establish
success.

The main-session single-call `tap` action `capability` has a special inline path that bypasses ordinary batch checks,
call-log recording, and result hooks. Guardrails are not a complete process or filesystem sandbox.

---

## Execution order

```text
User sends message
  ├── run_pipe (pre-model):
  │     evaluate [[pipe]] rules (roles → when → match)
  │     at most one pipe may match; multiple = error
  │     if matched → spawn script, stdin = user message
  │       exit 0 → stdout replaces user message
  │       exit ≠ 0 → hard stop, error displayed
  │     if no match → pass through unchanged
LLM receives (possibly transformed) user message
LLM emits tool calls [t0, t1, …]
  ├── check_batch (pre-call):
  │     for each ti in arrival order:
  │       resolve capability
  │       evaluate [[guard]] rules
  │       if blocked → don't spawn, return synthetic error
  │       if allowed → record in call log; execute after batch checks
  ├── join_all (parallel tool execution)
  ├── run_hooks (post-result):
  │     for each (call, real result):
  │       evaluate [[hook]] rules
  │       spawn matching scripts in parallel
  │       non-zero exits with non-empty (trimmed) stdout → inbox push
  ├── truncate large plain-text outputs, then supervisor condensation
  ├── return results to the LLM
LLM produces final assistant message (no more tool calls)
  ├── run_turn_validators:
  │     for each [[validator]] in declaration order:
  │       role filter → when filter → match filter
  │       survivors spawn scripts; advance their cursors immediately
  │         (cursor moves regardless of exit code — "the validator ran")
  │       collect outputs; non-zero exits → wrap + inbox push
Inbox feedback flows into a subsequent API request as system-managed user-role content
```

---

## Inbox routing

Hook and validator injections land in the **session inbox** — the same queue used by skill validators, scheduled
messages, webhooks, etc. The session loop drains the inbox before the next API request. Each non-zero-exit script with
non-empty trimmed stdout produces one inbox entry; entries are flushed in the order they were enqueued.

Inbox source kinds (the `source_kind` field in JSONL/WebSocket output) emitted by guardrails:

| Source | When |
|---|---|
| `guardrail_hook` | a `[[hook]]` script exited non-zero (with non-empty trimmed stdout) |
| `guardrail_validator` | a `[[validator]]` script exited non-zero (with non-empty trimmed stdout) |

These two values let you distinguish guardrail injections from the other inbox sources sharing the same queue —
`skill_validator`, `schedule`, `webhook`, `background_agent`, `tap_run`, `skill`, and `inject`. Grep the JSONL/WebSocket
stream by `source_kind` to filter for just guardrail output. See [WebSocket
Server](../integration/01-websocket-server.md) for the full structured-output schema.

---

## Authoring tips

- **Start permissive, tighten over time.** A wrong `[[guard]]` blocks real work; a wrong `[[validator]]` just nags.
  Prefer validators while iterating.
- **Use `+used / -unused` for conditional rules, not separate `[[guard]]`s.** Composing two rules with `when` is clearer
  than three rules with no history.
- **Keep scripts fast.** Hook and validator scripts run synchronously in the turn boundary; a 30-second script is a
  30-second pause. The 300 s timeout exists as a backstop, not a target.
- **stdout = the message, stderr = debugging.** If you're injecting noise, it consumes model context and can trigger
  another response. Feedback is system-managed, not a new user instruction. Be precise.
- **Check script behavior first.** Use the direct stdin examples above, then observe actual session injections. For
  example, with the validation-statement validator installed:

```bash
printf '%s\n' 'Reply with the single word Hello.' | octomind run developer:general --format jsonl \
  | jq 'select(.type == "injected") | select(.source_kind == "guardrail_validator")'
```

This checks feedback events, not whether a guard denied a tool. Guards return tool errors rather than injected
hook/validator events, and model tool choice can vary.

---

## Differences from skills

| | Skill validators (`programming-rust`, etc.) | Guardrails (`[[validator]]`) |
|---|---|---|
| Source | `<tap>/skills/<name>/validate` | `.agents/guardrails.toml` (project) |
| Trigger | active skill plus enabled validation | declared in guardrails file |
| State | skill auto-activation | per-validator cursor into call log |
| Filter | always when skill active | `roles` + `when` + `match` |
| Gated by `[skills].auto_validation` | yes — the flag enables/disables skill validate scripts | no — guardrail validators always run at turn end |
| Wrapping | `<validation skill="…">` | `<validation validator="…">` |

They share the inbox path and the activation-by-failure pattern, but live at different layers — skills are reusable
across projects, guardrails are project-local policy.

## Common questions

**Why did the rule not load?** A parse error disables the authored file as a whole. Check TOML syntax, regex escaping,
signed `when` entries, and unique pipe/validator names. Start a new session and inspect debug output:

```bash
octomind config --log-level debug
octomind run developer:general
```

**Why does a deny rule not match?** Its target is a resolved capability name, not a tool name. Use the ownership hook
above; tools without an owner need an unfiltered hook or another enforcement mechanism. `has` checks config server
membership, and literal `developer` role filters match domain prefixes; `developer:*` is not supported.

**Why is script feedback absent?** Hooks/validators inject only non-empty stdout on non-zero exit. A timeout or spawn
failure does not inject repair feedback. Check executable permissions and the script path. The direct child is killed on
timeout; descendant process cleanup is not guaranteed.

**Do generated rules overwrite this file?** No. With learning evolution enabled, generated rules are appended after
authored rules through the same native runtime. Manage them with `/learning evolution`; see [Learning](13-learning.md).

## Source reference

| Surface | Source |
|---------|--------|
| File schema and matching | [src/config/guardrails.rs](../../src/config/guardrails.rs) |
| Session state, ownership, history | [src/session/guardrails.rs](../../src/session/guardrails.rs) |
| Pipe execution | [src/session/pipe.rs](../../src/session/pipe.rs) |
| Hooks and validators | [src/session/hooks.rs](../../src/session/hooks.rs) |
| Batch ordering and inbox routing | [src/session/chat/response/tool_execution.rs](../../src/session/chat/response/tool_execution.rs), [src/session/inbox.rs](../../src/session/inbox.rs) |
| Rule reload and evolution | [src/session/chat/session/commands/done.rs](../../src/session/chat/session/commands/done.rs), [src/supervisor/learning/evolution/runtime.rs](../../src/supervisor/learning/evolution/runtime.rs) |

## See also

- [Skills](15-skills.md)
- [Local tools](17-local-tools.md)
- [Token efficiency](16-token-efficiency.md)
- [Cross-session learning](13-learning.md)
- [WebSocket server](../integration/01-websocket-server.md)
- [Configuration reference](../reference/03-config-reference.md)
