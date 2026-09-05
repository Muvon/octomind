# Token Efficiency

Use this guide to keep tool schemas focused with on-demand capabilities and to inspect the context your session sends.
It covers runtime activation, tool eviction, and the limits of those mechanisms.

## Get started

Start with a role that has the runtime tools, inspect its current surface, then ask for the capability you need:

```bash
octomind run developer:general
```

```text
/mcp full
Use the capability tool to list the database tools available to this role.
```

Capabilities are bundles from installed taps. Loading fewer optional bundles can reduce tool-schema context; the amount
saved depends on the actual schemas. Static role servers remain outside runtime eviction.

## Configure activation

These are root-level settings; place them before any TOML table in your existing config:

```toml
auto_capabilities = true
mcp_response_tokens_threshold = 20000
max_session_tokens_threshold = 200000
```

| Setting | Shipped default | Use |
|---------|----------------:|-----|
| `auto_capabilities` | `true` | Match fresh user intent to capability triggers before requests |
| `mcp_response_tokens_threshold` | `20000` | Limit individual tool-result context |
| `max_session_tokens_threshold` | `200000` | Configured session context ceiling, also bounded by model capacity |

For history compression and recoverable narrowing of large results, see [Compression](08-compression.md) and [Supervisor
condensation](14-supervisor.md#condense). Capability eviction does not shrink prior conversation text.

### Anatomy of a Capability

Each capability is two kinds of file:

- **`config.toml`** — capability-level metadata, shared across every provider. Holds the **required** `triggers = [...]`
  array (the phrases that drive auto-activation and `discover`) and an **optional** `domains = [...]` array (which roles
  may load it). If `config.toml` is missing or has no non-empty string `triggers` array, the capability fails to
  resolve.
- **`<provider>.toml`** — provider-specific MCP wiring: `[[mcp.servers]]`, `server_refs`, `allowed_tools`, and `[deps]`.
  The provider name comes from the `[capabilities]` config map (`<name> = "<provider>"`) and defaults to the literal
  `default`, so the fallback file is `default.toml`.

Triggers live in `config.toml`, never in the provider file. For example, in a local tap you own, create
`project-tools/capabilities/project-read/config.toml`. First create its directory:

```bash
mkdir -p project-tools/capabilities/project-read
```

```toml
triggers = ["read project files", "inspect repository contents", "find the source file"]
domains = ["developer"]
```

For an installed `octofs` executable on PATH, `project-tools/capabilities/project-read/default.toml` can contain:

```toml
[roles.mcp]
server_refs = ["project-files"]
allowed_tools = ["project-files:view", "project-files:list_files"]

[[mcp.servers]]
name = "project-files"
type = "stdio"
command = "octofs"
args = ["mcp"]
timeout_seconds = 30
tools = []
```

Here `[roles.mcp]` is the capability provider-file schema; it is not a role declaration in your user config. Register
the tap directory containing those files, then start a new session:

```bash
octomind tap myorg/project-tools ./project-tools
octomind run developer:general
```

Named references may be bare (`project-read`), pinned to the baseline tap (`octomind/memory-read`), or pinned to a
connected tap’s organization (`myorg/project-read`). The first connected tap for that organization wins. Provider
overrides in `[capabilities]` use the bare name, even for pinned references. To explicitly select the `default.toml`
provider from the example:

```toml
[capabilities]
project-read = "default"
```

## The `capability` Tool

A built-in `runtime` MCP tool. The `runtime` server hosts `mcp`, `agent`, `skill`, and `capability`; `orchestration`
hosts `tap`, `schedule`, and `monitor`; `core` conditionally exposes `recall`. Planning is supervisor-internal rather
than a tool.

| Action | Description |
|--------|-------------|
| `list` | Show all installed capabilities (in the current domain). Active ones are marked. |
| `discover` | Semantic search: `intent="read project files"` scores caps by trigger similarity, drops anything at or below a 0.2 cosine noise floor, and returns up to the top 5. |
| `enable` | Register and connect a capability's MCP servers. Tools become available to subsequent model requests. |
| `disable` | Remove runtime activation and its tool contributions; shared/static servers remain enabled. |

Pass one arguments object per call to the `capability` MCP tool. The enable/disable examples use the local
`project-read` capability above:

```json
{"action":"list"}
```

```json
{"action":"discover","intent":"read the project source files"}
```

```json
{"action":"enable","name":"project-read"}
```

```json
{"action":"disable","name":"project-read"}
```

`enable` and `disable` are idempotent. The auto-activator and env preloading use this runtime registry. Skill-declared
capabilities use a separate server-loading/refcount path; they are not governed by this four-capability LRU.

**Domains.** Listing, discovery, and new named activation are scoped to the current session's *domain* — the category
part of the active role (`developer` for `developer:general`). A capability with a non-empty `domains` list is only
visible to roles in those domains; an empty list means universal. `list` and `discover` silently omit out-of-domain
caps, and `enable` **hard-fails** for one (returning an error that names the role you'd need to run). See [Domain
Gating](#domain-gating) below.

### What "Activate" Actually Does

For each `[[mcp.servers]]` block in the resolved capability's `<provider>.toml`:

1. Compute a per-server tool filter from the capability's `allowed_tools` — namespace prefixes (`playwright:*`) are
  stripped to bare names (`*`); patterns scoped to other servers are dropped.
2. Branch on whether the server is already in the role's **static** config:
  - **Already static** (the agent manifest’s `capabilities = [...]` brought it in at boot): the server is already
  running, so we don't re-register it. Instead we extend the role's effective per-server filter via
  `runtime_overlay::set_capability_extras` and register this cap's named tools in the global tool map so dispatch can
  route them. Because the server belongs to the role, eviction never tears it down — only this cap's overlay-added tools
  are stripped.
  - **Fully dynamic** (the cap brought the server in at runtime): register it with the dynamic registry and
  `dynamic::enable_server` connects it, fetches its tool list, applies the filter, and registers the resulting tools.
3. The capability is recorded in the active set with its `(server, tools)` records and a fresh `last_used` timestamp.

Capabilities with no `[[mcp.servers]]` block but a non-empty `[deps].require` list are toolchain capabilities (e.g.
`programming-nodejs`): activation runs the dep installers and that *is* the activation. They are tracked by the LRU
registry with an empty server list.

## Deterministic Auto-Activation

Asking the model "do you need a database tool?" before every turn would burn a routing turn for every message. Instead,
Octomind embeds the user's message and matches it against the hand-authored triggers in each capability's `config.toml`.
No LLM in the routing loop.

### When It Runs

Inside `prepare_for_api_call`, during request preparation (activation itself requires fresh user intent):

```text
user message arrives
  → skill activation on the CLI input path
  → optional input pipe
  → prepare_for_api_call
       ├─ supervisor task resolution
       ├─ compression check
       ├─ if config.auto_capabilities:    ← master toggle (default true)
       │     auto_activate_capabilities   ← here
       └─ system message caching
  → API call
```

`auto_capabilities = true` in `default.toml` is the master switch for this whole path. Set it to `false` to disable the
pre-request automatic path. Named enable calls, env preloading, skill-declared capabilities, and the explicit `tap`
capability-intent action remain separate activation paths. The latter accepts this arguments object on the `tap` MCP
tool and evaluates skills/capabilities for the supplied prompt:

```json
{"action":"capability","prompt":"read the project source files"}
```

It is a silent no-op when:

- `config.auto_capabilities` is `false` (the entire call is gated on the master toggle).
- The last message in the session is not a fresh user message (e.g. mid tool loop).
- The cleaned user message has fewer than `MIN_INTENT_NON_WS_CHARS` (8) non-whitespace characters — short
  acknowledgments like `ok` or `do it` are suppressed because they produce noisy embeddings that can clear the threshold
  against an unrelated trigger by coincidence.
- The local embedding model (`muvon/octomind-embed`) is not yet ready (still downloading on first run).
- No eligible inactive capability has triggers and all required environment variables.
- No score clears the gate.

### How It Decides

```text
1. Strip XML blocks (skill injections, <log> pastes, <instructions>, etc.)
   from the user message so pasted content does not drive matches.

2. Bail if the cleaned intent has < 8 non-whitespace chars.

3. Drop out-of-domain capabilities (current role's domain), then keep
   only inactive ones whose required environment variables are set.

4. Embed the (cleaned) intent once.

5. For each remaining capability:
     - Embed its triggers (cached by content hash; free after first turn).
     - Score = mean of top-3 cosines between intent and trigger vectors.

6. Margin gate:
     activate iff   top1 >= 0.45   AND   top1 - top2 >= 0.08.

7. On a hit, register + enable the capability's MCP servers directly.
   Subsequent model requests receive the changed tool surface.
```

| Constant | Value | Purpose |
|----------|-------|---------|
| `AUTO_ACTIVATE_THRESHOLD` | `0.45` | Minimum mean-of-top-3 cosine for automatic activation. |
| `AUTO_ACTIVATE_MARGIN` | `0.08` | Required gap between top-1 and top-2. Prevents flipping a near-tied competitor on. |
| `AUTO_ACTIVATE_TOP_K` | `3` | Number of triggers averaged per capability. Mean-of-top-K smooths a single noisy trigger while still rewarding cap-author-aligned triggers. |

These are compile-time constants in `src/mcp/runtime/capability.rs`; routing fixtures live in
`src/mcp/runtime/capability_inline_tests.rs`.

### Why Margin Matters

Two database capabilities can score above the threshold while remaining too close to choose confidently. The margin gate
makes the system **abstain** in those cases. The user (or the agent on a later turn via `capability(action="discover")`)
provides the disambiguating signal.

### Why Triggers, Not Descriptions

Descriptions are written for humans and tend to use abstract domain language ("PostgreSQL adapter for relational
queries"). User messages are concrete and verbal ("I want to look at the slow query in our Postgres prod").
Mean-of-top-K cosine over hand-authored example triggers ("query a postgres database", "EXPLAIN ANALYZE a slow postgres
query", "look at the postgres schema") puts the cap centroid where users actually live.

## Domain Gating

A capability can declare optional `domains = [...]` in its `config.toml`. The **session domain** is the category part of
the active role: `developer:general` runs in the `developer` domain. The gate rule is simple:

- **Empty `domains`** → universal. Available in every role (typical for filesystem-style utilities).
- **Non-empty `domains`** → the capability is available only when the session domain is in the list.

The capability tool applies domain filtering on these paths; skill-declared server loading is a separate path:

- `capability(action="list")` and `capability(action="discover")` silently omit out-of-domain caps.
- Auto-activation filters out-of-domain caps *before* embedding their triggers — so a `developer:general` message can
  never accidentally flip on a `medical`-domain capability.
- `capability(action="enable")` **hard-fails** for an out-of-domain cap with an error like `Capability 'X' is bound to
  domains ["medical"]; current domain is 'developer'. Run the matching role (e.g. octomind run medical:general) to
  access it.`
- `OCTOMIND_CAPABILITIES` boot-loading (below) goes through the same `enable` path, so it is gated too.

When no domain is set at all (early init, out-of-session tool calls), only universal caps survive — the strict reading
of "a domain-restricted cap needs a known domain context."

## LRU Eviction

The active set has a soft cap of `MAX_ACTIVE_CAPS` capabilities (currently 4). When activating one more would exceed it,
the **least-recently-used** active capability is disabled first to make room. There is no idle timer or domain-shift
eviction in this registry.

### Shared and static servers

Capabilities can share a server with disjoint tool filters. Disabling one removes its recorded tool names and keeps a
referenced server enabled. Overlapping tool names are not a separate per-tool ownership guarantee.

Each `CapState` records `server_tools: Vec<(String, Vec<String>)>` — the precise list of bare tool names *this*
capability registered on each backing server. Eviction (and explicit `disable`) computes a refcount across the active
set:

- **disable server = true** → the server is *not* in the role's static config **and** no other active cap references it.
  The server is marked disabled, its function cache cleared, and the recorded tools are unregistered. This path does not
  guarantee immediate process termination.
- **disable server = false** → another active cap still references this server, **or** the server is declared in the
  role's static config. Only **this cap's** tools are stripped from `TOOL_MAP`; the server stays enabled and its process
  keeps running.

So a server present in the role's static config is **never torn down** by eviction or `disable` — `kill = !static_owned
&& refcount == 0`. The capability only contributed an overlay filter and some tool-map entries to a server the role
already owns; those overlay-added tools are stripped, the server remains enabled.

The decision is per-(capability, server) pair, computed atomically under a single registry write lock so refcounts never
see a partial state. This means tap authors can split a chunky capability into focused sub-capabilities (`filesystem` +
`filesystem-edit`, `memory` + `memory-knowledge`, `codesearch` + `codesearch-structural` + `codesearch-graph`) while
retaining servers referenced by other bundles — even when they all point at the same MCP server binary.

### What "Recently Used" Means

Tool dispatch after activation, not activation order:

- Every execution whose outer dispatcher result is `Ok(McpToolResult)` checks whether the tool came from a
  dynamic-server-backed capability.
- If yes, the first matching capability entry’s `last_used` is bumped to `Instant::now()`.
- A tool-level error is itself an `Ok(McpToolResult { is_error: true, ... })`, so it still refreshes recency. Only an
  outer routing/execution `Err` skips the touch.

The touch is a scan of the process-global active registry. Shared-server use touches the first matching entry, not
necessarily every capability on that server; static-only dispatch does not refresh this dynamic-server hook.

### Eviction sequence

When a new runtime activation arrives at the soft cap, the registry removes one least-recently-used entry, clears its
static-server overlay, and disables its recorded tools. New activation then proceeds. Dependency-only activation uses
the same eviction step. Disabling tools does not uninstall dependency packages.

### Limits

- **Idempotent below the cap.** Cheap no-op until all `MAX_ACTIVE_CAPS` slots are filled.
- **One eviction per activation.** Matches the call pattern (every new activation makes room for itself); no loop is
  needed.
- **Demand-driven only.** No background timer, no idle cleanup. A capability sitting unused at 4/4 stays active forever
  — until a 5th activation pushes the LRU one out.
- **Failure-tolerant.** A failure to disable one server is logged but does not block the new activation. Worst case: the
  cap is removed from the active set while one server stays enabled — preferable to refusing to activate.
- **Static servers are protected.** A server declared in the role's static config is never killed by eviction or
  `disable`, even at refcount 0 — only the cap's overlay-added tools are stripped from `TOOL_MAP`.
- **Pre-loaded boot capabilities are not tracked.** Anything resolved from the agent manifest at boot is merged into the
  role's effective config and behaves as a regular MCP server. The LRU registry only governs runtime-activated caps.

## Operational Notes

- **Logs.** Auto-activation logs at `info` (`· capability auto-activated: 'X' (score 0.NN) — servers: [...]`). Eviction
  logs at `info` (`capability LRU evicted: 'X' (N server-tool-group(s) processed)`). Embedding model warmup and silent
  skips (including the intent-too-short and domain skips) log at `debug`.
- **Discovering what's installed.** `capability(action="list")` shows everything available in the current domain with
  active markers. `capability(action="discover", intent="read project files")` ranks in-domain caps by trigger
  similarity, drops scores at or below the 0.2 noise floor, and returns up to 5. `discover` is embedding-only — there is
  no keyword fallback, so it errors if the embedding model is still downloading.
- **Skills can pull capabilities.** Skill-declared capabilities load servers through their own refcount path, outside
  the runtime LRU. See [Skills](15-skills.md#capabilities-auto-loading).
- **Force-loading at boot.** `OCTOMIND_CAPABILITIES=project-read octomind run developer:general` force-activates the
  listed capabilities at session start. Each comma-delimited item must be an installed capability name or a supported
  tap-qualified reference; there is no fuzzy, tool-name, or provider-name resolution. Activation still passes through
  environment/domain checks and the LRU; failures are logged and skipped.
- **Master toggle.** `auto_capabilities = true` (the default in `default.toml`) controls the whole auto-activation path;
  set it `false` to disable that pre-request path.
- **`MAX_ACTIVE_CAPS = 4`** is a compile-time constant in `src/mcp/runtime/capability.rs`. It bounds runtime-activated
  capabilities; the exact number of exposed tools still depends on each live server schema.
- **Re-activation on next match.** Evicted capabilities can be re-activated immediately if the next user message or
  `enable` call demands them. Eviction removes activation/tool state; trigger embeddings stay cached.

## Token-Cost Intuition

Per turn, the prompt carries the JSON schema for every active tool. The exact token load depends on the live schemas, so
inspect `/mcp full` rather than assuming a fixed per-tool size.

Compare the surface and usage during your session:

```text
/mcp full
/usage
/info
```

These show available tools and measured usage; they do not predict a fixed token saving for a capability. The four-entry
soft cap covers the process-global runtime capability registry, not all tools or isolated per-session bundles. Static
servers and skill-loaded servers can expose additional tools.

## Common questions

**Why did nothing activate?** Use a specific request, inspect debug scores, and check domain and required environment
variables. Near-tied matches abstain; name a known capability to enable it explicitly. `discover` needs embeddings;
`list` and named `enable` do not need a similarity score.

```text
/loglevel debug
Read the project source files using the project-read capability.
/mcp full
/loglevel info
```

**How do I preload the example capability?** After registering the tap above:

```bash
OCTOMIND_CAPABILITIES=project-read octomind run developer:general
```

An env-loaded capability counts toward the runtime LRU. Agent-manifest capabilities become static role servers.

## Where to Look (Code Map)

| Concern | File |
|---------|------|
| Active registry, eviction, scoring, auto-activation | [src/mcp/runtime/capability.rs](../../src/mcp/runtime/capability.rs) |
| Touch hook in tool dispatch | [src/mcp/mod.rs](../../src/mcp/mod.rs) (around the `try_execute_tool_call` site) |
| Auto-activation call site | [src/session/chat/session/api_prep.rs](../../src/session/chat/session/api_prep.rs) → `prepare_for_api_call` |
| Server enable / disable / unregister | [src/mcp/runtime/dynamic.rs](../../src/mcp/runtime/dynamic.rs) |
| Static-server filter extension | [src/config/runtime_overlay.rs](../../src/config/runtime_overlay.rs) → `set_capability_extras` |
| Domain gate | [src/agent/registry.rs](../../src/agent/registry.rs) → `cap_available_in_domain`; [src/mcp/runtime/capability.rs](../../src/mcp/runtime/capability.rs) → `filter_caps_by_domain` |
| Embedding model | [src/embeddings/mod.rs](../../src/embeddings/mod.rs) (`muvon/octomind-embed`, internal local model via octolib) |
| Capability TOML parsing (`config.toml` + `<provider>.toml`) | [src/agent/registry.rs](../../src/agent/registry.rs) → `read_capability_config`, `parse_capability_toml`, `list_all_capabilities` |
| Master toggle / intent gate | [src/session/chat/session/api_prep.rs](../../src/session/chat/session/api_prep.rs); [src/mcp/runtime/skill_auto.rs](../../src/mcp/runtime/skill_auto.rs) → `MIN_INTENT_NON_WS_CHARS` |
| Tap layout (capabilities directory) | [doc/integration/04-tap-system.md](../../doc/integration/04-tap-system.md) |

## See also

- [MCP Tools Reference](07-mcp-tools.md) — full reference for built-in tools.
- [Skills](15-skills.md) — skills can declare capabilities to auto-load.
- [Tap System](../integration/04-tap-system.md) — where capabilities live on disk.
- [Roles](06-roles.md) — base tool surface for each role before runtime activation.
