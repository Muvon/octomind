# Delegate work to tap roles

Grant the coordinator the orchestration server and the `orchestration:tap` tool. To start an installed role, call `tap`:

```json
{"action": "run", "prompt": "Audit src/auth/ for security bugs. Do not edit files. Return file:line evidence and concrete failure scenarios."}
```

The runtime matches the prompt to the best installed agent and starts it; the reply names the matched `role` and
`title`. If no agent fits, `run` starts nothing and returns an error that lists the closest ones. To use one of
them anyway, repeat the call with `role` set to its name.

`run` starts in the background and returns its ID. On completion, the inbox receives a tap-run result labeled with the
ID and role. Use these arguments to inspect, message, resume, or stop it:

```json
{"action": "list"}
```

While the agent is running, send a follow-up for its next available turn. It keeps working on the current turn
and receives the queued message in the same conversation:

```json
{"action": "send", "session": "RUN_ID", "prompt": "Also check cancellation and report any uncovered races."}
```

After it finishes, use `run` to resume it:

```json
{"action": "run", "session": "RUN_ID", "prompt": "Recheck the highest-severity finding and report the evidence."}
```

```json
{"action": "stop", "session": "RUN_ID"}
```

An agent that declares the `orchestration` capability can start copies of itself: set `role` to its own name, or omit `role` when it reaches no other agent. Add `"readonly": true` to start a read-only copy that scouts or reviews without changing files. Several writable copies can run at once on disjoint files. A copy cannot start further copies.

`list` lists runs in the current session, not the installed role catalog. Omitting `session` on a follow-up starts a
fresh agent. Resuming a still-running tap returns a busy error.

Matching requires initialized embeddings. A vague prompt may match no agent. The separate `tap` capability action
attempts capability auto-activation and can return an empty activation list; it does not share matching's explicit
embedding error contract:

```json
{"action": "capability", "prompt": "I need to search the project's source code."}
```

## Create a specialist for one session

```json
{"action":"create","name":"summary","description":"Summarize supplied notes","system":"Return three bullets grounded in the notes.","tools":[]}
```

`create` returns a tag of the form `<namespace>:<name>`. A base tap role creates in its own namespace; a role from your local config creates in the `personal` namespace. Variants, copies and read-only runs cannot create roles.

Run the returned tag with `{"action":"run","role":"RETURNED_TAG","prompt":"Summarize these notes: …"}`.
The role is private to its creator session. Omitted tools mean no tools. `tools` names tools the creator can call now; tools of the orchestration, runtime and local servers, and `code`, `search_tools`, `context`, cannot be granted. The optional `model` overrides the model. Creating the same name again in the session replaces the definition.

The personal tap is a local folder Octomind owns at `<data dir>/personal-tap/`. It is not a Git clone and is never updated from Git. Saved roles live in it at `agents/<namespace>/<name>.toml`.

To keep the role for later sessions, save it to the personal tap. No approval prompt is shown:
`{"action":"persist","role":"RETURNED_TAG"}`.
Later sessions find the saved role: `run` can match it and `role` can name it. Saving again overwrites the earlier save. It is refused when another tap provides that tag.

A saved role keeps the fixed tool list it was created with. To change its prompt or tools, create it again with the same name, then persist again. A name already saved in the personal tap may be created again; the session copy is used until it is saved again.

To delete a saved role, use `{"action":"unpersist","role":"RETURNED_TAG"}`. Only roles in your own namespace can be deleted.

Workdir and read-only mode belong to the run, not the definition.

## See also

- [MCP tools](../usage/07-mcp-tools.md)
- [Tap system](../integration/04-tap-system.md)
- [Roles](../usage/06-roles.md)
- [Custom development workflows](03-custom-development-workflow.md)
- [Event-driven webhooks](02-event-driven-agent.md)
