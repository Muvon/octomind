# Guardrails

Per-project policy for tool use, defined in `.agents/guardrails.toml`. Three section types cover three phases of the tool-execution lifecycle:

| Section | Phase | What it does | Side effect |
|---|---|---|---|
| `[[guard]]` | Pre-call | Block a tool from running | Synthetic error result returned to the model |
| `[[hook]]` | Post-result | Run a script against the tool result | Non-zero exit → script stdout injected as user message |
| `[[validator]]` | End-of-turn | Run a script after the assistant's final message | Non-zero exit → `<validation>`-wrapped stdout injected |

All three share the same matching DSL and live in the same file. Nothing is mandatory; missing file = no policy.

## File location

`<workdir>/.agents/guardrails.toml` — loaded fresh at session start. Parse errors are printed to stderr and treated as "no policy"; a broken file never crashes the session.

## Matching DSL

Used inside `match` (guards, hooks) and inside `when` entries (guards, validators):

```
capability                       # any call to that capability
capability(regex)                # regex matched against full args JSON
capability(arg_name=regex)       # regex matched against a specific arg
```

- **capability** = the MCP capability name as declared in tap manifests (e.g. `shell`, `filesystem-read`, `filesystem-write`). Resolved from the call's MCP server + tool name. Tools that aren't part of any capability never match.
- **regex on full args JSON** = the call's params object serialized to JSON, then matched. Use for any-arg patterns.
- **arg-targeted** = regex matched against just that arg's value. String args matched directly (no quotes); arrays/objects/numbers matched against their JSON form. Example: `paths=secret` matches `paths=["a","b/secret.env"]` because the haystack becomes `["a","b/secret.env"]`.

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

---

## `[[guard]]` — pre-call deny rules

```toml
[[guard]]
match   = "shell(command=^rm\\s+-rf?)"   # required: DSL target on the call
has     = "filesystem-read"              # optional: capability must be loaded
when    = ["-filesystem-read"]           # optional: history filter
message = "rm -rf blocked."              # required: shown to the model
```

### Semantics

- Evaluated in declaration order; **first match wins**.
- All conditions AND'd. Rule fires only when:
  - `match` target matches the current call, AND
  - every `has` capability is loaded in the session, AND
  - every `when` item is satisfied against the session call log.
- When the rule fires, the call is **blocked** before the executor runs. The model receives a synthetic tool error: `[guardrail] <message>`.

### Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `match` | DSL target | yes | the call to match |
| `has` | string or list | no | loaded-capability filter; empty = no filter |
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
has     = "filesystem-read"
when    = ["-filesystem-read"]
message = "Use the view tool instead of ls."
```

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
- Script runs with the tool context on stdin and environment. Exit 0 = no-op; exit ≠ 0 = stdout injected as one user message.

### Fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `match` | DSL target | no | call filter; empty = any tool |
| `result` | regex | no | regex on the result text; empty = any result (incl. empty) |
| `on` | enum | no | `success`, `error`, or `any` (default) |
| `script` | path | yes | relative to workdir |

### Script contract

| Channel | Use |
|---|---|
| **cwd** | session workdir |
| **stdin** | JSON `{capability, tool, tool_id, params, result, success}` |
| **env** | `OCTOMIND_CAPABILITY`, `OCTOMIND_TOOL`, `OCTOMIND_SUCCESS=1\|0`, `OCTOMIND_WORKDIR` |
| **stdout** | injected as user message if exit ≠ 0 |
| **stderr** | logged at debug level, never injected |
| **exit 0** | no-op |
| **exit ≠ 0** | stdout → inbox |
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
- On exit ≠ 0, stdout is wrapped as `<validation validator="<name>">…</validation>` and injected.

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
| **stdout** | wrapped + injected if exit ≠ 0 |
| **stderr** | logged at debug level |
| **exit 0** | no-op |
| **exit ≠ 0** | `<validation validator="<name>">stdout</validation>` → inbox |
| **timeout** | 300 s |

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

## Where it hooks in the pipeline

```
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
  │       non-zero exits → inbox push
  ├── truncate large outputs
  ├── return results to the LLM
LLM produces final assistant message (no more tool calls)
  ├── run_turn_validators:
  │     for each [[validator]] in declaration order:
  │       role filter → when filter → match filter
  │       survivors spawn scripts in parallel
  │       advance per-validator cursors
  │       non-zero exits → wrap + inbox push
Inbox messages flow into the next API call as user messages
```

---

## Sample full file

```toml
# Pre-call denials
[[guard]]
match   = "shell(command=^rm\\s+-rf?)"
message = "rm -rf blocked."

[[guard]]
match   = "shell(command=^ls\\b)"
has     = "filesystem-read"
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

Inbox source kinds (visible in JSONL/WebSocket output):

| Source | When |
|---|---|
| `guardrail_hook` | a `[[hook]]` script exited non-zero |
| `guardrail_validator` | a `[[validator]]` script exited non-zero |

---

## Authoring tips

- **Start permissive, tighten over time.** A wrong `[[guard]]` blocks real work; a wrong `[[validator]]` just nags. Prefer validators while iterating.
- **Use `+used / -unused` for conditional rules, not separate `[[guard]]`s.** Composing two rules with `when` is clearer than three rules with no history.
- **Keep scripts fast.** Hook and validator scripts run synchronously in the turn boundary; a 30-second script is a 30-second pause. The 300 s timeout exists as a backstop, not a target.
- **stdout = the message, stderr = debugging.** If you're injecting noise, the model treats it as a literal user message. Be precise.
- **Test before shipping.** `echo 'call shell …' | octomind run --format jsonl` lets you grep the JSONL for `"type":"injected"` and verify the guardrail fires exactly when intended.

---

## Differences from skills

| | Skill validators (`programming-rust`, etc.) | Guardrails (`[[validator]]`) |
|---|---|---|
| Source | tap manifest (skill) | `.agents/guardrails.toml` (project) |
| Trigger | declared in skill config | declared in guardrails file |
| State | skill auto-activation | per-validator cursor into call log |
| Filter | always when skill active | `roles` + `when` + `match` |
| Wrapping | `<validation skill="…">` | `<validation validator="…">` |

They share the inbox path and the activation-by-failure pattern, but live at different layers — skills are reusable across projects, guardrails are project-local policy.
