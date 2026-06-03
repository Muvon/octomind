# Guardrails

Per-project policy for tool use and input preprocessing, defined in `.agents/guardrails.toml`. Four section types cover four phases of the session lifecycle:

| Section | Phase | What it does | Side effect |
|---|---|---|---|
| `[[pipe]]` | Pre-model | Transform or validate user input before the model sees it | Non-zero exit → hard stop; stdout replaces user message |
| `[[guard]]` | Pre-call | Block a tool from running | Synthetic error result returned to the model |
| `[[hook]]` | Post-result | Run a script against the tool result | Non-zero exit → script stdout pushed to the session inbox (delivered as a user turn on the next request) |
| `[[validator]]` | End-of-turn | Run a script after the assistant's final message | Non-zero exit → `<validation>`-wrapped stdout pushed to the session inbox |

All four live in the same file. Nothing is mandatory; missing file = no policy.

**Who can stop vs who can only nudge:** only `[[pipe]]` and `[[guard]]` can *block* — a pipe stops the message before the model sees it, a guard stops a tool call before it runs. `[[hook]]` and `[[validator]]` can only *nudge*: they cannot undo anything, they just push a message to the inbox that the model reads on the next turn.

## File location

`<workdir>/.agents/guardrails.toml` — loaded fresh at session start. Parse errors are printed to stderr and treated as "no policy"; a broken file never crashes the session.

The file is read once per session (`init_for_session`) and the resulting rules — together with the per-session state they drive (call log, per-validator cursors, per-pipe run counts, message counter) — are scoped to that session. Editing `guardrails.toml` mid-session has no effect; start a new session for changes to take effect.

## Matching DSL

Used inside `match` (guards, hooks) and inside `when` entries (guards, validators):

```
capability                       # any call to that capability
capability(regex)                # regex matched against full args JSON
capability(arg_name=regex)       # regex matched against a specific arg
```

- **capability** = the MCP capability name as declared in tap manifests (e.g. `shell`, `filesystem-read`, `filesystem-write`). See [How a tool call resolves to a capability](#how-a-tool-call-resolves-to-a-capability) below. Tools that aren't part of any capability never match.
- **regex on full args JSON** = the call's params object serialized to JSON, then matched. Use for any-arg patterns.
- **arg-targeted** = regex matched against just that arg's value. String args matched directly (no quotes); arrays/objects/numbers matched against their JSON form. Example: `paths=secret` matches `paths=["a","b/secret.env"]` because the haystack becomes `["a","b/secret.env"]`.

> **Two namespaces, one word.** The word "capability" appears in three places with **two distinct meanings**, so be careful:
> - In `match` / `when` targets and in the hook payload (`capability` field / `OCTOMIND_CAPABILITY` env) it means a **capability name** — the resolved tap-capability that owns the tool (e.g. `shell`, `filesystem-read`).
> - In `[[guard]] has` it means an **MCP server name** — the name of a configured `[[mcp.servers]]` entry active for the role (e.g. `core`, `runtime`, `agent`, `filesystem`). These are *not* capability names, so `has = "filesystem-read"` would never match.

### How a tool call resolves to a capability

`match` / `when` targets and the hook `capability`/`OCTOMIND_CAPABILITY` value all use a **capability name**. Octomind resolves a call's `(server, tool)` pair to that name as follows:

1. **Static tap manifests first** — a `(server, tool) → capability` map is built from every installed capability's `allowed_tools`. A capability "owns" a tool when its `allowed_tools` lists `server:tool` (exact) or `server:*` (wildcard). The capability name is matched exact first, then by wildcard.
2. **Runtime overlay fallback** — capabilities activated mid-session (via `skill use` or enabling a capability) are checked next.
3. **No owner → no match.** Tools registered directly by a role (not through any capability) resolve to *no* capability and can never be matched by a guard/hook/validator DSL target.

So the capability name for, say, the `view` tool is whatever tap capability lists `filesystem:view` (or `filesystem:*`) in its `allowed_tools` — not the server name `filesystem`. To discover the capability names available in a session, use the `/mcp` command (see the Skills and Token Efficiency docs for how capabilities are declared).

## `when` conditions (signed list)

Used by `[[guard]]` (session-wide history) and `[[validator]]` (since-last-run history). Each entry is one DSL target with a sign prefix:

```toml
when = [
  "+filesystem-write",                  # was used
  "-shell(command=cargo test)",         # was NOT used
]
```

- `+target` = at least one matching call exists in the relevant history window.
- `-target` = no matching call exists in the relevant history window.

All `when` items are AND'd. Cross-section: history is the **session call log**, accumulated as tool calls succeed; blocked calls don't enter the log.

## `[[pipe]]` — pre-model input transform

Runs a matching script on the raw user input **before the model sees it**. The script receives the user message on stdin; its stdout replaces the message sent to the model. Non-zero exit is a hard stop — the message is not sent to the model and an error is displayed.

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
- At most one pipe may match per message. If two or more pipes match, an error is displayed and the message is not sent to the model.
- The pipe runs in the session's working directory.
- Script timeout: 300 seconds (same as hooks and validators).

### Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | yes | identifier, used in error messages and `PIPE_NAME` env var |
| `command` | path | yes | script path, relative to workdir (or absolute) |
| `match` | regex | no | regex on user message text; empty = matches all messages |
| `when` | enum | no | `"first"` (first message only) or `"any"` (default, every message) |
| `roles` | list of strings | no | role filter; exact (`developer:general`) or domain prefix (`developer` ≡ `developer:*`) |

### Script contract

| Channel | Use |
|---|---|
| **cwd** | session workdir |
| **stdin** | raw user message text |
| **env** | `OCTOMIND_ROLE`, `OCTOMIND_WORKDIR`, `PIPE_NAME`, `PIPE_RUN_COUNT`, `SESSION_MESSAGE_COUNT` |
| **stdout** | replaces the user message (used as-is, no trimming) |
| **stderr** | logged at debug level |
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
| `SESSION_MESSAGE_COUNT` | total number of user messages in the session so far (including the current one); incremented for every user message even when no pipe matches |

### Example: validate input format

```toml
[[pipe]]
name    = "validate"
command = "./scripts/validate-input.sh"
```

`validate-input.sh`:
```bash
#!/usr/bin/env bash
input=$(cat)
if [[ "$input" =~ ^/ ]]; then
  echo "Commands must not start with /" >&2
  exit 1
fi
echo "$input"
```

### Example: enrich first message with context

```toml
[[pipe]]
name    = "context-enricher"
command = "./scripts/add-context.sh"
when    = "first"
roles   = ["developer"]
```
---

## `[[guard]]` — pre-call deny rules

```toml
[[guard]]
match   = "shell(command=^rm\\s+-rf?)"   # required: DSL target on the call
has     = "filesystem"                   # optional: MCP server must be loaded
when    = ["-filesystem-read"]           # optional: history filter
message = "rm -rf blocked."              # required: shown to the model
```

### Semantics

- Evaluated in declaration order; **first match wins**.
- All conditions AND'd. Rule fires only when:
  - `match` target matches the current call, AND
  - every `has` entry is the name of an MCP **server** active for the role, AND
  - every `when` item is satisfied against the session call log.
- **`has` uses MCP server names, not capability names.** It is checked against the set of configured `[[mcp.servers]]` entries active for the current role (e.g. `core`, `runtime`, `agent`, `filesystem`) — *not* the capability vocabulary used by `match`/`when`. A value like `filesystem-read` (a capability name) will never match and the rule will never fire.
- When the rule fires, the call is **blocked** before the executor runs. The model receives a synthetic tool error: `[guardrail] <message>`.

### Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `match` | DSL target | yes | the call to match |
| `has` | string or list | no | loaded-MCP-**server**-name filter (e.g. `core`, `filesystem`); empty = no filter. Not a capability name. |
| `when` | list of `+/-target` | no | history filter; empty = no filter |
| `message` | string | yes | the text the model sees |

### Example: tiered shell policy

```toml
[[guard]]
match   = "shell(command=^rm\\s+-rf?\\s+/)"
message = "Refusing rm -rf on root paths."

[[guard]]
match   = "shell(command=git push.*(--force|-f)\\b)"
message = "Force push blocked. Use --force-with-lease and ask first."

[[guard]]
match   = "shell(command=^ls\\b)"
has     = "filesystem"
when    = ["-filesystem-read"]
message = "Use the view tool instead of ls."
```

`has = "filesystem"` checks that the `filesystem` MCP server is loaded for the role; `when = ["-filesystem-read"]` checks that the `filesystem-read` *capability* has not been used yet in this session — two different namespaces working together.

### Performance

Guards evaluate **in batch, in arrival order, before any tool spawns**. Each allowed call is recorded into the session log so the next call in the same batch sees it via `when`. Blocked calls never reach the executor — no time is wasted.

---

## `[[hook]]` — post-result scripts

```toml
[[hook]]
match  = "shell(command=^cargo build)"   # optional: filter on the call
result = "error\\[E\\d+\\]"              # optional: regex on result text
on     = "any"                           # optional: "success" | "error" | "any"
script = ".agents/hooks/cargo-lint.sh"   # required: path relative to workdir
```

### Semantics

- Fires after each tool result lands, before results are returned to the model.
- All matching hooks fire (no first-match-wins). Multiple hooks compose.
- Skipped for guardrail-blocked tools — their synthetic result is not a real result.
- Script runs with the tool context on stdin and environment. Exit 0 = no-op; exit ≠ 0 = stdout pushed to the session inbox (delivered as a user turn on the next request). Stdout is trimmed first; if it is empty after trimming, nothing is pushed even on a non-zero exit.

### Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `match` | DSL target | no | call filter; empty/whitespace = any tool (skipped, treated as no filter) |
| `result` | regex | no | regex on the result text; `result = ""` compiles to a match-everything regex (unlike `match`, which skips empty), so it matches any result incl. empty |
| `on` | enum | no | `success`, `error`, or `any` (default) |
| `script` | path | yes | relative to workdir |

### Script contract

| Channel | Use |
|---|---|
| **cwd** | session workdir |
| **stdin** | JSON `{capability, tool, tool_id, params, result, success}` (`capability` = resolved capability name, or `null` if the tool isn't owned by any capability) |
| **env** | `OCTOMIND_CAPABILITY`, `OCTOMIND_TOOL`, `OCTOMIND_SUCCESS=1\|0`, `OCTOMIND_WORKDIR` |
| **stdout** | pushed to the session inbox if exit ≠ 0; trimmed first |
| **stderr** | logged at debug level, never injected |
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

`cargo-summary.sh`:
```bash
#!/usr/bin/env bash
set -e
errors=$(jq -r '.result' <<< "$(cat)" | grep -oE 'error\[E[0-9]+\]' | sort -u)
[ -z "$errors" ] && exit 0
echo "Build emitted: $errors. Fix the type errors before continuing."
exit 1
```

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
- Per-validator cursor into the session call log. `when` is evaluated against `call_log[cursor..]` — i.e. **calls since this validator last ran**. On run, cursor advances to `call_log.len()`.
- All filters AND'd. None set → fires every turn (skill-like).
- Guardrail validators run at the end of **every** assistant turn (subject only to the `roles`/`when`/`match` filters). They are a separate system from skill `SKILL.md` validate scripts and are **not** gated by the `[skills].auto_validation` config flag — that flag only controls skill validators.
- On exit ≠ 0, stdout is trimmed, then wrapped as `<validation validator="<name>">…</validation>` and pushed to the session inbox. If stdout is empty after trimming, nothing is pushed even on a non-zero exit.

### Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | yes | unique identifier, used for the cursor and the XML tag |
| `match` | regex | no | matched against the assistant's final message text |
| `when` | list of `+/-target` | no | history check vs the slice since last run |
| `roles` | list of strings | no | role filter; exact (`developer:general`) or domain prefix (`developer` ≡ `developer:*`) |
| `script` | path | yes | relative to workdir |

### Filter short-circuit order (cheapest first)

1. **role filter** — skip the validator entirely if the current session role isn't in the list.
2. **`when` filter** — slice the call log from this validator's cursor, run `+used / -unused` checks.
3. **`match` regex** — run the regex over the assistant's final message text.

Only validators that pass all three filters spawn their script. The cursor advances **as soon as the script spawns**, regardless of exit code: "the validator ran" consumes the window.

### Script contract

| Channel | Use |
|---|---|
| **cwd** | session workdir |
| **stdin** | JSON `{validator, role, assistant_text, triggered_by:[{capability,params}, …]}` |
| **env** | `OCTOMIND_VALIDATOR`, `OCTOMIND_ROLE`, `OCTOMIND_WORKDIR` |
| **stdout** | trimmed, then wrapped + pushed to inbox if exit ≠ 0 |
| **stderr** | logged at debug level |
| **exit 0** | no-op |
| **exit ≠ 0** | `<validation validator="<name>">stdout</validation>` → inbox; if stdout is empty after trimming, nothing is pushed |
| **timeout** | 300 s |

`triggered_by` lists the calls in the since-last-run slice that matched a `+used` target. When the validator has no `+used` targets configured, it instead contains **every** call in that slice — i.e. everything that happened since this validator last ran — so an always-on validator still sees the full window of activity.

### Example: nudge to test after edits

```toml
[[validator]]
name   = "test-after-edit"
when   = ["+filesystem-write", "-shell(command=cargo test)"]
script = ".agents/validators/remind-tests.sh"
```

`remind-tests.sh`:
```bash
#!/usr/bin/env bash
echo "You edited files but didn't run cargo test. Run it before declaring done."
exit 1
```

### Example: always-on linter

```toml
[[validator]]
name   = "always-lint"
script = ".agents/validators/lint.sh"
```

No filters → fires every turn end.

---

## How history works (call log)

The session maintains a single ordered call log: `Vec<(capability, params)>`. Every **successful** tool call is appended. Blocked calls (denied by a `[[guard]]`) are **not** recorded — they didn't happen, so they shouldn't satisfy history conditions on retry.

- `[[guard]]` `when` reads the entire log (session-wide history).
- `[[validator]]` `when` reads `log[cursor..]` where the cursor is per-validator.
- `[[hook]]` doesn't use `when` — it's a per-result reaction.

This means the same DSL has consistent meaning everywhere; the difference is just which slice is consulted.

---

## Execution order

```
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
  │       if allowed → record in call log, spawn task
  ├── join_all (parallel tool execution)
  ├── run_hooks (post-result):
  │     for each (call, real result):
  │       evaluate [[hook]] rules
  │       spawn matching scripts in parallel
  │       non-zero exits with non-empty (trimmed) stdout → inbox push
  ├── truncate large outputs
  ├── return results to the LLM
LLM produces final assistant message (no more tool calls)
  ├── run_turn_validators:
  │     for each [[validator]] in declaration order:
  │       role filter → when filter → match filter
  │       survivors spawn scripts; advance their cursors immediately
  │         (cursor moves regardless of exit code — "the validator ran")
  │       collect outputs; non-zero exits → wrap + inbox push
Inbox messages flow into the next API call as user turns
```

---

## Sample full file

```toml
# Pre-model input transform
[[pipe]]
name    = "context-enricher"
command = "./scripts/add-context.sh"
when    = "first"
roles   = ["developer"]

# Pre-call denials
[[guard]]
match   = "shell(command=^rm\\s+-rf?)"
message = "rm -rf blocked."

[[guard]]
match   = "shell(command=^ls\\b)"
has     = "filesystem"
when    = ["-filesystem-read"]
message = "Use the view tool instead of ls."

[[guard]]
match   = "filesystem-read(paths=\\.env)"
message = "Refusing to read .env files."

# Post-result reactions
[[hook]]
match  = "shell(command=^cargo (build|test|check))"
result = "error\\["
script = ".agents/hooks/cargo-summary.sh"

[[hook]]
on     = "error"
script = ".agents/hooks/log-failures.sh"

# End-of-turn validators
[[validator]]
name   = "lint-after-edit"
when   = ["+filesystem-write"]
script = ".agents/validators/lint.sh"

[[validator]]
name   = "test-before-done"
match  = "(?i)\\b(done|completed|finished)\\b"
when   = ["+filesystem-write", "-shell(command=cargo test)"]
script = ".agents/validators/remind-tests.sh"
```

---

## Inbox routing

Hook and validator injections land in the **session inbox** — the same queue used by skill validators, scheduled messages, webhooks, etc. The session loop drains the inbox before the next API request. Each non-zero-exit script produces one inbox entry; entries are flushed in the order they were enqueued.

Inbox source kinds (the `source_kind` field in JSONL/WebSocket output) emitted by guardrails:

| Source | When |
|---|---|
| `guardrail_hook` | a `[[hook]]` script exited non-zero (with non-empty trimmed stdout) |
| `guardrail_validator` | a `[[validator]]` script exited non-zero (with non-empty trimmed stdout) |

These two values let you distinguish guardrail injections from the other inbox sources sharing the same queue — `skill_validator`, `schedule`, `webhook`, `background_agent`, `tap_run`, `skill`, and `inject`. Grep the JSONL/WebSocket stream by `source_kind` to filter for just guardrail output. See [WebSocket Server](../integration/01-websocket-server.md) for the full structured-output schema.

---

## Authoring tips

- **Start permissive, tighten over time.** A wrong `[[guard]]` blocks real work; a wrong `[[validator]]` just nags. Prefer validators while iterating.
- **Use `+used / -unused` for conditional rules, not separate `[[guard]]`s.** Composing two rules with `when` is clearer than three rules with no history.
- **Keep scripts fast.** Hook and validator scripts run synchronously in the turn boundary; a 30-second script is a 30-second pause. The 300 s timeout exists as a backstop, not a target.
- **stdout = the message, stderr = debugging.** If you're injecting noise, the model treats it as a literal user message. Be precise.
- **Test before shipping.** `echo 'call shell …' | octomind run --format jsonl` lets you grep the JSONL for `"type":"injected"` events and verify the guardrail fires exactly when intended. Filter by the `source_kind` field (`guardrail_hook` / `guardrail_validator`) to isolate guardrail output from other inbox injections.

---

## Differences from skills

| | Skill validators (`programming-rust`, etc.) | Guardrails (`[[validator]]`) |
|---|---|---|
| Source | tap manifest (skill) | `.agents/guardrails.toml` (project) |
| Trigger | declared in skill config | declared in guardrails file |
| State | skill auto-activation | per-validator cursor into call log |
| Filter | always when skill active | `roles` + `when` + `match` |
| Gated by `[skills].auto_validation` | yes — the flag enables/disables skill validate scripts | no — guardrail validators always run at turn end |
| Wrapping | `<validation skill="…">` | `<validation validator="…">` |

They share the inbox path and the activation-by-failure pattern, but live at different layers — skills are reusable across projects, guardrails are project-local policy.
