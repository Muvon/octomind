# Token Efficiency: Capabilities, Auto-Activation, and LRU Eviction

Octomind keeps the model's tool surface small on purpose. Every exposed tool eats prompt tokens on every turn, and a wide tool zoo encourages the LLM to pick wrong tools. This document describes the runtime mechanisms that make that work:

- The **`capability`** tool — discover and activate domain bundles on demand.
- **Deterministic auto-activation** — flip the right capability on without burning an LLM turn.
- **LRU eviction** — bound the active surface so it cannot grow without limit.

## The Problem

Static MCP config has two failure modes:

1. **Over-provisioned**: every potentially-useful server is bound at boot. The system prompt balloons, costs rise, and the model wastes attention on irrelevant tools.
2. **Under-provisioned**: only the bare minimum is bound. The model silently fails when a query needs a missing tool, or worse, fakes the result.

Capabilities split the difference. They are TOML-defined bundles (`<tap>/capabilities/<name>/<provider>.toml`) that resolve to one or more MCP servers and optional tool filters. Nothing about a capability is loaded until it is actually needed. When loaded, only the relevant tools enter the prompt.

## The `capability` Tool

A built-in `core` MCP tool. Available to every role in every session.

| Action | Description |
|--------|-------------|
| `list` | Show all installed capabilities. Active ones are marked. |
| `discover` | Semantic search: `intent="..."` returns the top 5 capabilities matching the user's wording. |
| `enable` | Register and connect a capability's MCP servers. Tools become available next turn. |
| `disable` | Disconnect a capability's servers and remove its tools. |

```json
{"action": "list"}
{"action": "discover", "intent": "I need to query a Postgres database"}
{"action": "enable", "name": "database-postgres"}
{"action": "disable", "name": "database-postgres"}
```

`enable` and `disable` are idempotent. Activation paths are also entered automatically by the auto-activator (see below) and by skills that declare `capabilities: ...` in their frontmatter.

### What "Activate" Actually Does

For each `[[mcp.servers]]` block in the resolved capability TOML:

1. Register the server with the dynamic registry (idempotent).
2. Compute a per-server tool filter from the capability's `allowed_tools` — namespace prefixes (`playwright:*`) are stripped to bare names (`*`); patterns scoped to other servers are dropped.
3. `dynamic::enable_server` connects the server, fetches its tool list, applies the filter, and registers the resulting tools in the global tool map.
4. The capability is recorded in the active set with its server names and a fresh `last_used` timestamp.

Capabilities with no `[[mcp.servers]]` block but a non-empty `[deps]` section are toolchain capabilities (e.g. `programming-nodejs`): activation runs the dep installers and that *is* the activation. They are tracked by the LRU registry with an empty server list.

## Deterministic Auto-Activation

Asking the model "do you need a database tool?" before every turn would burn a routing turn for every message. Instead, Octomind embeds the user's message and matches it against hand-authored triggers in each capability TOML. No LLM in the routing loop.

### When It Runs

Inside `prepare_for_api_call`, **before every API request**:

```text
user message arrives
  → run_activation hook (skill auto-activation)
  → prepare_for_api_call
       ├─ compression check
       ├─ auto_activate_capabilities      ← here
       └─ system message caching
  → API call
```

It is a silent no-op when:

- The last message in the session is not a fresh user message (e.g. mid tool loop).
- The local embedding model (`muvon/octomind-embed`, a BGE-small-en-v1.5 fine-tune) is not yet ready (still downloading on first run).
- No capability has triggers, or every cap is already active.
- No score clears the gate.

### How It Decides

```
1. Strip XML blocks (skill injections, <log> pastes, <instructions>, etc.)
   from the user message so pasted content does not drive matches.

2. Embed the (cleaned) intent once.

3. For each inactive capability:
     - Embed its triggers (cached by content hash; free after first turn).
     - Score = mean of top-3 cosines between intent and trigger vectors.

4. Margin gate:
     activate iff   top1 >= 0.55   AND   top1 - top2 >= 0.08.

5. On a hit, register + enable the capability's MCP servers directly.
   The agent never sees the routing decision; it just gets a wider tool
   surface next turn.
```

| Constant | Value | Purpose |
|----------|-------|---------|
| `AUTO_ACTIVATE_THRESHOLD` | `0.55` | Minimum mean-of-top-3 cosine. Tuned for the fine-tuned octomind-embed model over short hand-authored triggers. |
| `AUTO_ACTIVATE_MARGIN` | `0.08` | Required gap between top-1 and top-2. Prevents flipping a near-tied competitor on. |
| `AUTO_ACTIVATE_TOP_K` | `3` | Number of triggers averaged per capability. Mean-of-top-K smooths a single noisy trigger while still rewarding cap-author-aligned triggers. |

These are compile-time constants in `src/mcp/core/capability.rs`. The values are picked so that capability fixtures pass ≥85% top-1 accuracy on positive cases and ≥80% abstain rate on negatives — see `capability_routing_fixtures_match_expected_caps` in the same file.

### Why Margin Matters

A simple threshold flips the wrong capability on whenever two are nearly tied (e.g. `database-postgres` vs `database-mysql`, both well above the threshold for a generic database intent). The margin gate makes the system **abstain** in those cases. The user (or the agent on a later turn via `capability(action="discover")`) provides the disambiguating signal.

### Why Triggers, Not Descriptions

Descriptions are written for humans and tend to use abstract domain language ("PostgreSQL adapter for relational queries"). User messages are concrete and verbal ("I want to look at the slow query in our Postgres prod"). Mean-of-top-K cosine over hand-authored example triggers ("query a postgres database", "EXPLAIN ANALYZE a slow postgres query", "look at the postgres schema") puts the cap centroid where users actually live.

## LRU Eviction

The active set has a soft cap of `MAX_ACTIVE_CAPS` capabilities (currently 4). When activating one more would exceed it, the **least-recently-used** active capability is disabled first to make room. Eviction is the only auto-disable mechanism — Octomind deliberately does not time-decay or domain-shift evict, because production agent UX is hurt more by false-disable than by carrying an idle cap.

### Shared MCP Servers Are Safe

Multiple capabilities can legitimately back onto the **same** MCP server with disjoint tool filters. For example, `octocode` exposes both `semantic_search`+`view_signatures` (used by the `codesearch` capability) and `graphrag`+`get_node` (used by `codesearch-graph`). Both capabilities can be active simultaneously, and disabling one does **not** strip tools belonging to the other.

Each `CapState` records `server_tools: Vec<(String, Vec<String>)>` — the precise list of bare tool names *this* capability registered on each backing server. Eviction (and explicit `disable`) computes a refcount across the active set:

- **kill_server = true** → no other active cap references this server. The server process is shut down, its function cache cleared, and all of its tools are removed from `TOOL_MAP`.
- **kill_server = false** → another active cap still references this server. Only **this cap's** tools are stripped from `TOOL_MAP`; the server stays enabled and its process keeps running for the remaining cap(s).

The decision is per-(capability, server) pair, computed atomically under a single registry write lock so refcounts never see a partial state. This means tap authors can split a chunky capability into focused sub-capabilities (`filesystem` + `filesystem-edit`, `memory` + `memory-knowledge`, `codesearch` + `codesearch-structural` + `codesearch-graph`) without worrying that activating one will tear down the others — even when they all point at the same MCP server binary.

### What "Recently Used" Means

Real tool usage, not activation order:

- Every successful tool execution checks if the tool came from a dynamic-server-backed capability.
- If yes, the owning capability's `last_used` is bumped to `Instant::now()`.
- Failed tool calls do **not** refresh the timestamp — a flapping server stays evictable.

```rust
// src/mcp/mod.rs (tool dispatch path, simplified)
let result = try_execute_tool_call(call, ...).await;

if result.is_ok() {
    if let Some(server) = dynamic::get_dynamic_server_name_by_tool(&call.tool_name) {
        capability::touch_capability_for_server(&server);
    }
}
```

The touch is one HashMap scan over at most `MAX_ACTIVE_CAPS` active caps (4) — negligible cost per call.

### Eviction Algorithm

```rust
// src/mcp/core/capability.rs
fn evict_lru_if_full(config: &Config) {
    if active_count() < MAX_ACTIVE_CAPS { return; }     // 1. soft cap check

    // 2. Compute the disable plan under one write lock so refcounts
    //    are consistent: pull the LRU's per-server tool record,
    //    remove the cap from the registry, then compute kill_server
    //    for each (server, tools) by counting remaining references.
    let plan: Option<(String, Vec<(String, Vec<String>, bool)>)> = {
        let mut reg = registry().write().unwrap();
        select_lru_in(&mut reg).map(|(lru_name, server_tools)| {
            let entries = server_tools.into_iter()
                .map(|(srv, tools)| {
                    let kill = server_refcount(&reg, &srv, &lru_name) == 0;
                    (srv, tools, kill)
                })
                .collect();
            (lru_name, entries)
        })
    };

    // 3. Apply the plan outside the lock. `disable_server_tools` strips
    //    the listed tool names from TOOL_MAP, and only kills the server
    //    process when kill == true.
    if let Some((name, entries)) = plan {
        for (srv, tools, kill) in &entries {
            dynamic::disable_server_tools(srv, tools, *kill, Some(config))?;
        }
        log_info!("capability LRU evicted: '{}' ({} server-tool-group(s))", ...);
    }
}
```

Called at every activation entry point:

| Entry point | Site |
|-------------|------|
| `capability(action="enable")` server path | `handle_enable` |
| `capability(action="enable")` deps-only path | `handle_enable` (deps branch) |
| `auto_activate_capabilities` | via `activate_capability_inline` |
| `activate_capability_inline` deps-only path | inside helper |

### Properties

- **Idempotent below the cap.** Cheap no-op until all `MAX_ACTIVE_CAPS` slots are filled.
- **One eviction per activation.** Matches the call pattern (every new activation makes room for itself); no loop is needed.
- **Demand-driven only.** No background timer, no idle cleanup. A capability sitting unused at 4/4 stays active forever — until a 5th activation pushes the LRU one out.
- **Failure-tolerant.** A failure to disable one server is logged but does not block the new activation. Worst case: the cap is removed from the active set while one server stays enabled — preferable to refusing to activate.
- **Pre-loaded boot capabilities are not tracked.** Anything resolved from the agent manifest at boot is merged into the role's effective config and behaves as a regular MCP server. The LRU registry only governs runtime-activated caps.

### Pseudocode of the Full Lifecycle

```text
on user_message:
    auto_activate_capabilities():
        if not embedding_model_ready: return
        if no inactive caps: return
        score each inactive cap (mean-of-top-3 trigger cosine)
        if top1 >= 0.55 and top1 - top2 >= 0.08:
            activate_capability_inline(top1.name):
                evict_lru_if_full()         ← may disable LRU cap + its servers
                register + enable each [[mcp.servers]]
                mark_active(name, [servers], now)

on tool_call:
    try_execute_tool_call() -> result
    if result is ok:
        if tool came from a dynamic server:
            touch_capability_for_server(server)   ← bumps last_used to now

on capability(action="disable"):
    for each server in cap: dynamic::disable_server(server)
    mark_inactive(cap)

on capability(action="enable", name=X):
    if X already active: return idempotent ok
    evict_lru_if_full()
    register + enable each server
    mark_active(X, [servers], now)
```

## Operational Notes

- **Logs.** Auto-activation logs at `info` (`capability auto-activated: 'X' (score 0.NN) — servers: [...]`). Eviction logs at `info` (`capability LRU evicted: 'X' (N server(s) disabled to make room)`). Embedding model warmup and silent skips log at `debug`.
- **Discovering what's installed.** `capability(action="list")` shows everything available across taps with active markers. `capability(action="discover", intent="...")` ranks them by trigger similarity.
- **Skills can pull capabilities.** A skill with `capabilities: programming-rust git` resolves and activates those capabilities on `skill use`. They go through the same registry and LRU bookkeeping.
- **`MAX_ACTIVE_CAPS = 4`** is a compile-time constant in `src/mcp/core/capability.rs`. The value balances two pressures: (1) tool-overload research (Microsoft, AWS, Boundary, Chroma) shows sharp accuracy degradation past ~20-25 total tools exposed to the model — with ~15-20 baseline tools plus ~4-5 tools per cap, four active caps keeps total surface in the safe zone; (2) real task concurrency rarely needs more than 2-3 capabilities at once, so 4 leaves headroom for cross-domain work without churning.
- **Re-activation on next match.** Evicted capabilities can be re-activated immediately if the next user message or `enable` call demands them. Eviction only releases servers; trigger embeddings stay cached.

## Token-Cost Intuition

Per turn, the prompt carries the JSON schema for every active tool. With ~10 tools per medium-sized MCP server and ~200 tokens of schema each, a single large server adds ~2k tokens to *every* request and *every* response. Multiply by the rest of the conversation and an extra always-on server is a multi-cent-per-turn overhead even on cheap models.

Octomind's design point: the model gets exactly the tools it needs, when it needs them, automatically — without paying for an LLM-routing turn or a bloated default prompt.

## Where to Look (Code Map)

| Concern | File |
|---------|------|
| Active registry, eviction, scoring, auto-activation | `src/mcp/core/capability.rs` |
| Touch hook in tool dispatch | `src/mcp/mod.rs` (around the `try_execute_tool_call` site) |
| Auto-activation call site | `src/session/chat/session/api_prep.rs` → `prepare_for_api_call` |
| Server enable / disable / unregister | `src/mcp/core/dynamic.rs` |
| Embedding model | `src/embeddings/` (`muvon/octomind-embed`, fine-tuned BGE-small via candle) |
| Capability TOML parsing | `src/agent/registry.rs` → `parse_capability_toml`, `list_all_capabilities` |
| Tap layout (capabilities directory) | `doc/integration/04-tap-system.md` |

## See Also

- [MCP Tools Reference](07-mcp-tools.md) — full reference for built-in tools.
- [Skills](15-skills.md) — skills can declare capabilities to auto-load.
- [Tap System](../integration/04-tap-system.md) — where capabilities live on disk.
- [Roles](06-roles.md) — base tool surface for each role before runtime activation.
