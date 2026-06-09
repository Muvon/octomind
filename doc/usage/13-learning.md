# Cross-Session Learning

Octomind can extract and reuse lessons across sessions — mistakes corrected, patterns that worked, user preferences — so the same issues aren't repeated.

## Overview

The learning system has two phases:
1. **Extraction** — after `/done` (or during auto-compaction), an LLM analyzes the conversation and extracts a small number of lessons from your corrections and stated rules.
2. **Injection** — at the start of a session (and as the conversation continues) relevant stored lessons are injected as a user-role message the model reads before responding.

Each lesson has a **scope** that decides where it lands and how it is retrieved:

- **`scoped`** (the default) — tied to a single project and role. Stored under `learning/{project}/{role_base}/` and retrieved by relevance to what you're working on right now.
- **`global`** — a durable, user-wide preference that applies in every project and role. Stored under `learning/_/` and injected once per session by importance, with no relevance gating.

So scoped lessons are organized **project first, then role** (project knowledge stays within the project, the role filters it further), while global lessons deliberately cross both boundaries. See [Lesson Scope](#lesson-scope) for details.

## Configuration

Learning is one mechanic of the **supervisor** — the out-of-band control plane around the agent loop — so its config lives under `[supervisor.learning]`. (Earlier versions used a top-level `[learning]` table; that is a **breaking** rename with no migration.) See [`[supervisor]` in the config reference](../reference/03-config-reference.md#supervisor) for the sibling sections (orientation, detectors, gate).

```toml
[supervisor.learning]
enabled = true
model = "anthropic:claude-haiku-4-5"
backend = "file"
min_messages_for_intermediate = 3
max_inject = 5
```

| Field | Description | Default |
|-------|-------------|---------|
| `enabled` | Enable the learning system. | `true` |
| `model` | Model for extraction and retrieval-prep LLM calls. Use a cheap model. | `anthropic:claude-haiku-4-5` |
| `backend` | `"file"` (default) or `"mcp"` for external memory tools. | `"file"` |
| `min_messages_for_intermediate` | Minimum user messages before intermediate learning triggers during auto-compaction. | `3` |
| `max_inject` | Maximum lessons injected per tier per retrieval. | `5` |

> **Strict config, template-provided values.** The supervisor config is strict: the `[supervisor]` section and its `[supervisor.learning]` table are **required** — removing them is a hard parse error, not a silent fall-back. Within `[supervisor.learning]`, an *omitted field* still takes the code default (e.g. `enabled` → `false` (learning OFF), `model` → the dated build `anthropic:claude-haiku-4-5-20251001`). Learning is on out of the box only because the shipped template sets `enabled = true` explicitly. See [Supervisor](14-supervisor.md) for the sibling mechanics.

### Orientation memory

Alongside lessons (the procedural *"do / avoid"*), the supervisor stores **orientation** — durable, descriptive understanding of the subject: how it works, key decisions, constraints. It rides the same backend under `memory_type = "orientation"` and is recalled as **working assumptions to verify**, never as truth, under its own `## Orientation` heading. Configure it under `[supervisor.orientation]` (`enabled`, `max_inject`, `decay_days`).

## How It Works

### Lesson Scope

Every lesson is classified as either `scoped` or `global`, and the extraction LLM picks the scope for each one. It is instructed to be conservative: most lessons are `scoped`, and a lesson only becomes `global` when it is clearly about *how you work in general* rather than this task, project, or role.

| Scope | Stored in | Retrieved how |
|-------|-----------|---------------|
| `scoped` (default) | `learning/{project}/{role_base}/` | By relevance to your current request (hybrid keyword + embedding search) |
| `global` | `learning/_/` | Once per session, ranked by importance, with no relevance gating — they always apply |

A worked example: you tell the agent *"always open a single PR"* while working in project `octofs` as `developer:general`. That is a general working preference, so it becomes a **global** lesson and lands in `learning/_/`. Later you tell it *"in this repo, all API endpoints require bearer auth"* — that is specific to this project, so it is **scoped** and lands in `learning/octofs/developer/` (note the role is truncated at `:` to its base, `developer`).

### Storage (File Backend)

Scoped lessons are stored as markdown files with YAML frontmatter, one file per lesson, in a project/role directory; global lessons go in the shared `_` directory:

```
~/.local/share/octomind/learning/
  ├── octofs/developer/              # scoped: {project}/{role_base}
  │   ├── 20260405143000-bearer-auth-required.md
  │   └── 20260405143001-custom-error-types.md
  └── _/                             # global: cross-project, cross-role
      └── 20260405150000-always-single-pr.md
```

The role component is the **base part before `:`** — a lesson from role `developer:general` is stored under `developer/`, matching how role tags are sent to MCP servers.

Each file carries the full frontmatter the backend writes, in this exact order:

```markdown
---
title: "Bearer token auth required for all API endpoints"
content: "Bearer token auth required for all API endpoints"
memory_type: learning
importance: 0.9
confidence: high
tags: [auth, api]
source: "260405-142040-octofs-25e37715"
role: "developer"
project: "octofs"
scope: scoped
created: "2026-04-05T14:30:00Z"
---
```

- `title` is a short summary auto-derived from the first ~80 characters of the content (trimmed to a word boundary).
- `scope` is `scoped` or `global` and determines which directory the file lives in.

Files are human-readable and editable. Delete a file to remove a lesson — or use the [`/learning` command](#managing-lessons-learning).

### Extraction

Extraction is triggered by:
- **`/done`** — extracts (if `supervisor.learning.enabled`) regardless of the compression result, and marks the session so `/exit` and Ctrl+D don't extract a second time.
- **Auto-compaction** — extracts during compression if the session has enough user messages (configurable via `min_messages_for_intermediate`).
- **Session exit** — fire-and-forget extraction when the session ends naturally via `/exit`, `/quit`, or Ctrl+D. Skipped if `/done` already extracted during the session.

Extraction always runs **detached** (a background task with no cost tracked against your session) and is deliberately strict about what counts as a lesson:

1. **Decision gate.** The LLM first emits `<decision>LEARN</decision>` or `<decision>NONE</decision>`. On `NONE`, extraction stops immediately and nothing is parsed.
2. **Mandatory evidence.** Every `<lesson>` must carry an `evidence` attribute quoting the user verbatim. A lesson with no (or empty) evidence is silently dropped.
3. **At most 3 lessons** per extraction — one strong lesson beats three weak ones.
4. **Only user corrections and user-stated rules qualify** — explicit corrections, declared project conventions/preferences/constraints, or a repeated correction of the same mistake. Things the AI figured out on its own, one-off debugging steps, generic developer knowledge, and anything derivable by reading the codebase do **not** qualify.

Confidence drives importance: `confidence=high` (a direct correction) → `importance 0.9`; anything else (a stated preference, `confidence=medium`) → `importance 0.6`.

**Dedup and supersede.** The extraction LLM receives the existing lessons (both scoped and global) so it can avoid duplicates. Before storing, within the same scope: an identical-content lesson is skipped, and a refinement — a new lesson with **more than 60% word overlap** against an existing one — *supersedes* it (the old file is deleted, the new one written). This is why a hand-edited near-duplicate can disappear after the next extraction.

### Injection

Injected lessons are added as a **user-role message** (under a `##` heading) that the model reads before it responds — not appended to the system prompt. Injection happens in two moments:

- **First message of the session** — the global tier (ranked by importance) plus a full hybrid scoped recall are retrieved and injected under `## Lessons from Past Sessions`.
- **Each subsequent new user message** — an embedding-only scoped recall runs and any *newly* relevant lessons are injected under `## Additional Relevant Lessons`. Lessons already injected earlier in the session are deduped out, so nothing is repeated.

There is therefore a small per-turn cost on new user messages (the scoped recall), not a one-time cached append. Tool follow-up rounds within a single turn do not trigger another recall.

### Retrieval (File Backend)

Scoped recall is a **hybrid search**: LLM-extracted keywords (sparse substring ranking) are fused with BGE-small embedding cosine similarity (dense) via Reciprocal Rank Fusion (RRF, `k=60`), then recency-reweighted (30-day half-life, up to a +50% boost so fresh lessons edge out stale ones among already-relevant matches). Embedding candidates below a `0.2` cosine floor are dropped as noise, and if the embedding model isn't ready yet the cosine signal is silently skipped — keyword ranking alone still returns results. The LLM keyword-prep call runs only on the **first** retrieval of a session; follow-up messages use embedding-only recall (no extra LLM call).

### Managing Lessons (`/learning`)

The interactive `/learning` command lets you browse and prune lessons for the current role and project:

| Command | Effect |
|---------|--------|
| `/learning` | List lessons (page 1). |
| `/learning list [page]` | List a specific page. 15 lessons per page. |
| `/learning list *pattern*` | Filter by a glob pattern matched against content, title, and tags (e.g. `/learning list *auth*`). Combine with a page number. |
| `/learning delete <index>` | Delete a lesson by its **1-based index** from the last list. Aliases: `rm`, `remove`. |
| `/learning clear` | Delete **all** lessons for the current role + project scope. |

The list (and therefore delete indexing) covers the current scoped lessons followed by the global lessons, in a stable order. `clear` only wipes the current role+project scope. See [Session Commands](../reference/02-session-commands.md) for the full command reference.

## MCP Backend

For projects using external memory tools (e.g. octobrain), configure the MCP backend with field mapping:

```toml
[supervisor.learning]
enabled = true
model = "anthropic:claude-haiku-4-5"
backend = "mcp"

[supervisor.learning.store]
tool = "memorize"
[supervisor.learning.store.field_map]
content = "content"        # required by memorize
title = "title"            # required by memorize — short summary
memory_type = "memory_type"
importance = "importance"
confidence = "source"      # remapped to octobrain's source trust tier (see below)
tags = "tags"
role = "role"
project = "project"

[supervisor.learning.retrieve]
tool = "remember"
[supervisor.learning.retrieve.field_map]
query = "query"            # the LLM-prepared search query (or raw intent)
memory_type = "memory_types" # always sent as ["learning"] to match octobrain schema
role = "role"
project = "project"
limit = "limit"            # octobrain max is 5
```

Each entry in `field_map` maps a canonical learning field to the MCP tool's actual argument name. Set a value to `""` to omit that field. Missing entries are also omitted. Store and retrieve have separate field maps because MCP tools have different argument schemas.

**Mappable canonical keys differ by endpoint:**

- **store** can map any lesson field: `content`, `title`, `memory_type`, `importance`, `confidence`, `tags`, `source`, `role`, `project`, `scope`, `created`.
- **retrieve** can map only these five: `query`, `role`, `project`, `limit`, `memory_type`. `memory_type` is always sent as the array `["learning"]` regardless of the value.

**Value remapping.** When `confidence` is mapped, the value sent is **not** the literal `"high"`/`"medium"` string — it is remapped to a trust tier: `high` → `"user_confirmed"`, anything else → `"agent_inferred"`. This is what makes `confidence = "source"` line up with octobrain's source field.

**MCP backend limitations:**

- **Deletion is not supported** — `delete` always errors. Manage lessons through the MCP tool directly. (This also means `/learning delete`/`clear` won't work against an MCP backend.)
- The internal "all lessons" and "all global lessons" reads used for dedup/supersede during extraction issue a wildcard query (`["*"]`) with a hardcoded `limit = 100`, and rely on the tool returning the existing lessons. Global lessons are queried with empty `role`/`project` — the MCP server owns the scoping semantics.

### `McpEndpointConfig`

Both `store` and `retrieve` use the same structure:

| Field | Type | Description |
|-------|------|-------------|
| `tool` | `String` | MCP tool name (e.g. `"memorize"`, `"remember"`) |
| `field_map` | `HashMap<String, String>` | Maps canonical learning fields to the tool's argument names |

## Relationship to Memory

Learning is **separate from memory** (octobrain, CLAUDE.md, etc.):

- **Memory** is broad context storage — code patterns, architecture, project state, references.
- **Learning** is narrow and structured — actionable facts scored by confidence, extracted from outcomes, with deduplication.

Both can coexist. Learning focuses on the corrections and rules you gave the agent, and surfaces the relevant ones into future sessions automatically.
