# Token Efficiency: Capabilities, Auto-Activation, and LRU Eviction

Octomind keeps the model's tool surface small on purpose. Every exposed tool eats prompt tokens on every turn, and a wide tool zoo encourages the LLM to pick wrong tools. This document describes the runtime mechanisms that make that work:

- The **`capability`** tool — discover and activate domain bundles on demand.
- **Deterministic auto-activation** — flip the right capability on without burning an LLM turn.
- **LRU eviction** — bound the active surface so it cannot grow without limit.

## The Problem

Static MCP config has two failure modes:

1. **Over-provisioned**: every potentially-useful server is bound at boot. The system prompt balloons, costs rise, and the model wastes attention on irrelevant tools.
2. **Under-provisioned**: only the bare minimum is bound. The model silently fails when a query needs a missing tool, or worse, fakes the result.

Capabilities split the difference. A capability is a directory in a tap — `<tap>/capabilities/<name>/` — that resolves to one or more MCP servers and optional tool filters. Nothing about a capability is loaded until it is actually needed. When loaded, only the relevant tools enter the prompt.

### Anatomy of a Capability

Each capability is two kinds of file:

- **`config.toml`** — capability-level metadata, shared across every provider. Holds the **required** `triggers = [...]` array (the phrases that drive auto-activation and `discover`) and an **optional** `domains = [...]` array (which roles may load it). If `config.toml` is missing or has no `triggers`, the capability fails to resolve.
- **`<provider>.toml`** — provider-specific MCP wiring: `[[mcp.servers]]` / `server_refs`, `allowed_tools`, and `[deps]`. The provider name comes from the `[capabilities]` config map (`<name> = "<provider>"`), defaulting to `default` (so `codesearch.toml` unless you set `codesearch = "octocode"`).

Triggers — central to everything below — live in `config.toml`, never in the `<provider>.toml`.

## The `capability` Tool

A built-in `runtime` MCP tool — the `runtime` builtin server hosts `mcp`, `agent`, `skill`, `schedule`, and `capability`, while `core` hosts only `plan` and `tap`. Available to every role in every session.

| Action | Description |
|--------|-------------|
| `list` | Show all installed capabilities (in the current domain). Active ones are marked. |
| `discover` | Semantic search: `intent="..."` scores caps by trigger similarity, drops anything at or below a 0.2 cosine noise floor, and returns up to the top 5. |
| `enable` | Register and connect a capability's MCP servers. Tools become available next turn. |
| `disable` | Disconnect a capability's servers and remove its tools. |

```json
{"action": "list"}
{"action": "discover", "intent": "I need to query a Postgres database"}
{"action": "enable", "name": "database-postgres"}
{"action": "disable", "name": "database-postgres"}
```

`enable` and `disable` are idempotent. Activation paths are also entered automatically by the auto-activator (see below) and by skills that declare `capabilities: ...` in their frontmatter.

**Domains.** Every action is scoped to the current session's *domain* — the category part of the active role (`developer` for `developer:general`). A capability with a non-empty `domains` list is only visible to roles in those domains; an empty list means universal. `list` and `discover` silently omit out-of-domain caps, and `enable` **hard-fails** for one (returning an error that names the role you'd need to run). See [Domain Gating](#domain-gating) below.

### What "Activate" Actually Does

For each `[[mcp.servers]]` block in the resolved capability's `<provider>.toml`:

1. Compute a per-server tool filter from the capability's `allowed_tools` — namespace prefixes (`playwright:*`) are stripped to bare names (`*`); patterns scoped to other servers are dropped.
2. Branch on whether the server is already in the role's **static** config:
   - **Already static** (the role's `capabilities = [...]` brought it in at boot): the server is already running, so we don't re-register it. Instead we extend the role's effective per-server filter via `runtime_overlay::set_capability_extras` and register this cap's named tools in the global tool map so dispatch can route them. Because the server belongs to the role, eviction never tears it down — only this cap's overlay-added tools are stripped.
   - **Fully dynamic** (the cap brought the server in at runtime): register it with the dynamic registry and `dynamic::enable_server` connects it, fetches its tool list, applies the filter, and registers the resulting tools.
3. The capability is recorded in the active set with its `(server, tools)` records and a fresh `last_used` timestamp.

Capabilities with no `[[mcp.servers]]` block but a non-empty `[deps]` section are toolchain capabilities (e.g. `programming-nodejs`): activation runs the dep installers and that *is* the activation. They are tracked by the LRU registry with an empty server list.

## Deterministic Auto-Activation

Asking the model "do you need a database tool?" before every turn would burn a routing turn for every message. Instead, Octomind embeds the user's message and matches it against the hand-authored triggers in each capability's `config.toml`. No LLM in the routing loop.

### When It Runs

Inside `prepare_for_api_call`, **before every API request**:

```text
user message arrives
  → run_activation hook (skill auto-activation)
  → prepare_for_api_call
       ├─ compression check
       ├─ if config.auto_capabilities:    ← master toggle (default true)
       │     auto_activate_capabilities   ← here
       └─ system message caching
  → API call
```

`auto_capabilities = true` in `default.toml` is the master switch for this whole path. Set it to `false` and capabilities only ever activate through an explicit `capability(action="enable")` call (manual, skill-declared, or `OCTOMIND_CAPABILITIES`).

It is a silent no-op when:

- `config.auto_capabilities` is `false` (the entire call is gated on the master toggle).
- The last message in the session is not a fresh user message (e.g. mid tool loop).
- The cleaned user message has fewer than `MIN_INTENT_NON_WS_CHARS` (8) non-whitespace characters — short acknowledgments like `ok` or `do it` are suppressed because they produce noisy embeddings that can clear the threshold against an unrelated trigger by coincidence.
- The local embedding model (`muvon/octomind-embed`, a BGE-small-en-v1.5 fine-tune) is not yet ready (still downloading on first run).
- No capability is in the current domain, has triggers, or every cap is already active.
- No score clears the gate.

### How It Decides

```
1. Strip XML blocks (skill injections, <log> pastes, <instructions>, etc.)
   from the user message so pasted content does not drive matches.

2. Bail if the cleaned intent has < 8 non-whitespace chars.

3. Drop out-of-domain capabilities (current role's domain), then keep
   only the inactive ones. This happens before embedding, to save work.

4. Embed the (cleaned) intent once.

5. For each remaining capability:
     - Embed its triggers (cached by content hash; free after first turn).
     - Score = mean of top-3 cosines between intent and trigger vectors.

6. Margin gate:
     activate iff   top1 >= 0.45   AND   top1 - top2 >= 0.08.

7. On a hit, register + enable the capability's MCP servers directly.
   The agent never sees the routing decision; it just gets a wider tool
   surface next turn.
```

| Constant | Value | Purpose |
|----------|-------|---------|
| `AUTO_ACTIVATE_THRESHOLD` | `0.45` | Minimum mean-of-top-3 cosine. Calibrated for the fine-tuned octomind-embed model, which separates chitchat/out-of-domain inputs into a distinct cluster — so the floor can drop and the margin gate becomes the binding constraint. History: `0.42` (base BGE, recall-tuned) → `0.55` (base BGE, false-positive-tuned) → `0.45` (FT model). |
| `AUTO_ACTIVATE_MARGIN` | `0.08` | Required gap between top-1 and top-2. Prevents flipping a near-tied competitor on. |
| `AUTO_ACTIVATE_TOP_K` | `3` | Number of triggers averaged per capability. Mean-of-top-K smooths a single noisy trigger while still rewarding cap-author-aligned triggers. |

These are compile-time constants in `src/mcp/core/capability.rs`. The values are picked so that capability fixtures pass ≥85% top-1 accuracy on positive cases and ≥80% abstain rate on negatives — see `capability_routing_fixtures_match_expected_caps` in the same file.

### Why Margin Matters

A simple threshold flips the wrong capability on whenever two are nearly tied (e.g. `database-postgres` vs `database-mysql`, both well above the threshold for a generic database intent). The margin gate makes the system **abstain** in those cases. The user (or the agent on a later turn via `capability(action="discover")`) provides the disambiguating signal.

### Why Triggers, Not Descriptions

Descriptions are written for humans and tend to use abstract domain language ("PostgreSQL adapter for relational queries"). User messages are concrete and verbal ("I want to look at the slow query in our Postgres prod"). Mean-of-top-K cosine over hand-authored example triggers ("query a postgres database", "EXPLAIN ANALYZE a slow postgres query", "look at the postgres schema") puts the cap centroid where users actually live.

## Domain Gating

A capability can declare optional `domains = [...]` in its `config.toml`. The **session domain** is the category part of the active role: `developer:general` runs in the `developer` domain. The gate rule is simple:

- **Empty `domains`** → universal. Available in every role (typical for filesystem-style utilities).
- **Non-empty `domains`** → the capability is available only when the session domain is in the list.

This single gate is applied at *every* entry point, with no bypass:

- `capability(action="list")` and `capability(action="discover")` silently omit out-of-domain caps.
- Auto-activation filters out-of-domain caps *before* embedding their triggers — so a `developer:general` message can never accidentally flip on a `medical`-domain capability.
- `capability(action="enable")` **hard-fails** for an out-of-domain cap with an error like `Capability 'X' is bound to domains ["medical"]; current domain is 'developer'. Run the matching role (e.g. octomind run medical:general) to access it.`
- `OCTOMIND_CAPABILITIES` boot-loading (below) goes through the same `enable` path, so it is gated too.

When no domain is set at all (early init, out-of-session tool calls), only universal caps survive — the strict reading of "a domain-restricted cap needs a known domain context."

## LRU Eviction

The active set has a soft cap of `MAX_ACTIVE_CAPS` capabilities (currently 4). When activating one more would exceed it, the **least-recently-used** active capability is disabled first to make room. Eviction is the only auto-disable mechanism — Octomind deliberately does not time-decay or domain-shift evict, because production agent UX is hurt more by false-disable than by carrying an idle cap.

### Shared MCP Servers Are Safe

Multiple capabilities can legitimately back onto the **same** MCP server with disjoint tool filters. For example, `octocode` exposes both `semantic_search`+`view_signatures` (used by the `codesearch` capability) and `graphrag`+`get_node` (used by `codesearch-graph`). Both capabilities can be active simultaneously, and disabling one does **not** strip tools belonging to the other.

Each `CapState` records `server_tools: Vec<(String, Vec<String>)>` — the precise list of bare tool names *this* capability registered on each backing server. Eviction (and explicit `disable`) computes a refcount across the active set:

- **kill = true** → the server is *not* in the role's static config **and** no other active cap references it. The server process is shut down, its function cache cleared, and all of its tools are removed from `TOOL_MAP`.
- **kill = false** → another active cap still references this server, **or** the server is declared in the role's static config. Only **this cap's** tools are stripped from `TOOL_MAP`; the server stays enabled and its process keeps running.

So a server present in the role's static config is **never torn down** by eviction or `disable` — `kill = !static_owned && refcount == 0`. The capability only contributed an overlay filter and some tool-map entries to a server the role already owns; those overlay-added tools are stripped, the server keeps running.

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
    //    remove the cap from the registry, then compute kill for each
    //    (server, tools): never kill a static-owned server, otherwise
    //    kill only when no other active cap references it.
    let plan: Option<(String, Vec<(String, Vec<String>, bool)>)> = {
        let mut reg = registry().write().unwrap();
        select_lru_in(&mut reg).map(|(lru_name, server_tools)| {
            let entries = server_tools.into_iter()
                .map(|(srv, tools)| {
                    // A server in the role's static config is never killed:
                    // the role owns it regardless of dynamic-cap activity.
                    let static_owned = config.mcp.servers.iter().any(|s| s.name() == srv);
                    let kill = !static_owned && server_refcount(&reg, &srv, &lru_name) == 0;
                    (srv, tools, kill)
                })
                .collect();
            (lru_name, entries)
        })
    };

    // 3. Apply the plan outside the lock. Drop the overlay contributions
    //    first, then `disable_server_tools` strips the listed tool names
    //    from TOOL_MAP, and only kills the server process when kill == true.
    if let Some((name, entries)) = plan {
        clear_capability_extras(&name);
        for (srv, tools, kill) in &entries {
            dynamic::disable_server_tools(srv, tools, *kill, Some(config))?;
        }
        log_info!("capability LRU evicted: '{}' ({} server-tool-group(s) processed)", ...);
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
- **Static servers are protected.** A server declared in the role's static config is never killed by eviction or `disable`, even at refcount 0 — only the cap's overlay-added tools are stripped from `TOOL_MAP`.
- **Pre-loaded boot capabilities are not tracked.** Anything resolved from the agent manifest at boot is merged into the role's effective config and behaves as a regular MCP server. The LRU registry only governs runtime-activated caps.

### Pseudocode of the Full Lifecycle

```text
on user_message (only if config.auto_capabilities):
    auto_activate_capabilities():
        if intent < 8 non-ws chars: return
        if not embedding_model_ready: return
        drop out-of-domain caps; keep inactive ones
        if no candidates: return
        score each candidate (mean-of-top-3 trigger cosine)
        if top1 >= 0.45 and top1 - top2 >= 0.08:
            activate_capability_inline(top1.name):
                evict_lru_if_full()         ← may disable LRU cap + its servers
                register + enable each [[mcp.servers]]
                mark_active(name, [(server, tools)], now)

on tool_call:
    try_execute_tool_call() -> result
    if result is ok:
        if tool came from a dynamic server:
            touch_capability_for_server(server)   ← bumps last_used to now

on capability(action="disable", name=X):
    if X not active: return idempotent ok
    remove X from registry under one write lock
    for each (server, tools) in X:
        kill = !static_owned && refcount(server) == 0
        dynamic::disable_server_tools(server, tools, kill)

on capability(action="enable", name=X):
    if X already active: return idempotent ok
    if X out of domain: return error (run the matching role)
    evict_lru_if_full()
    register + enable each server (static-overlay or fully-dynamic)
    mark_active(X, [(server, tools)], now)
```

## Operational Notes

- **Logs.** Auto-activation logs at `info` (`· capability auto-activated: 'X' (score 0.NN) — servers: [...]`). Eviction logs at `info` (`capability LRU evicted: 'X' (N server-tool-group(s) processed)`). Embedding model warmup and silent skips (including the intent-too-short and domain skips) log at `debug`.
- **Discovering what's installed.** `capability(action="list")` shows everything available in the current domain with active markers. `capability(action="discover", intent="...")` ranks in-domain caps by trigger similarity, drops scores at or below the 0.2 noise floor, and returns up to 5. `discover` is embedding-only — there is no keyword fallback, so it errors if the embedding model is still downloading. (The capability tool's own JSON schema description still says discover "falls back to keyword match"; that fallback no longer exists in the code.)
- **Skills can pull capabilities.** A skill with `capabilities: programming-rust git` resolves and activates those capabilities on `skill use`. They go through the same registry and LRU bookkeeping.
- **Force-loading at boot.** `OCTOMIND_CAPABILITIES=cap1,cap2 octomind run -r developer` force-activates the listed capabilities at session start — *before* the agent's first turn — bypassing both the embedding auto-activator and the `capability` tool. Activation still passes through the domain gate and the LRU; already-active caps are no-ops, and any failures are logged and skipped (never aborting the session). Useful for CI / non-interactive runs that need a deterministic tool surface.
- **Master toggle.** `auto_capabilities = true` (the default in `default.toml`) controls the whole auto-activation path; set it `false` to require explicit `enable` calls only.
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
| Static-server filter extension | `src/config/runtime_overlay.rs` → `set_capability_extras` |
| Domain gate | `src/agent/registry.rs` → `cap_available_in_domain`; `src/mcp/core/capability.rs` → `filter_caps_by_domain` |
| Embedding model | `src/embeddings/` (`muvon/octomind-embed`, fine-tuned BGE-small via candle) |
| Capability TOML parsing (`config.toml` + `<provider>.toml`) | `src/agent/registry.rs` → `read_capability_config`, `parse_capability_toml`, `list_all_capabilities` |
| Master toggle / intent gate | `src/session/chat/session/api_prep.rs`; `src/mcp/core/skill_auto.rs` → `MIN_INTENT_NON_WS_CHARS` |
| Tap layout (capabilities directory) | `doc/integration/04-tap-system.md` |

## See Also

- [MCP Tools Reference](07-mcp-tools.md) — full reference for built-in tools.
- [Skills](15-skills.md) — skills can declare capabilities to auto-load.
- [Tap System](../integration/04-tap-system.md) — where capabilities live on disk.
- [Roles](06-roles.md) — base tool surface for each role before runtime activation.
