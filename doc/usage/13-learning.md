# Cross-Session Learning

Octomind can extract and reuse short lessons and grounded long-lived experiences across sessions, so corrections, architectural discoveries, outcomes, and failed approaches do not need to be rediscovered.

## Overview

The learning system has two phases:
1. **Extraction** — after `/done` (or during auto-compaction), an LLM analyzes the conversation and extracts a small number of lessons from your corrections and stated rules.
2. **Active packing** — before each genuine user turn, relevant stored lessons are selected into one bounded runtime pack that accompanies specialist requests for that turn.

Each lesson has a **scope** that decides where it lands and how it is retrieved:

- **`scoped`** (the default) — tied to a single project and role. Stored under `learning/{project}/{role_base}/` and retrieved by relevance to what you're working on right now.
- **`global`** — a durable, user-wide preference that applies in every project and role. Stored under `learning/_/` and reconsidered for every replacement pack by importance, with no relevance gating.

So scoped lessons are organized **project first, then role** (project knowledge stays within the project, the role filters it further), while global lessons deliberately cross both boundaries. See [Lesson Scope](#lesson-scope) for details.

## Configuration

Learning is one mechanic of the **supervisor** — the out-of-band control plane around the agent loop — so its config lives under `[supervisor.learning]`. (Earlier versions used a top-level `[learning]` table; that is a **breaking** rename with no migration.) See [`[supervisor]` in the config reference](../reference/03-config-reference.md#supervisor) for the sibling sections (gate, plan, condense).

```toml
[supervisor.learning]
enabled = true
model = "anthropic:claude-haiku-4-5"
backend = "file"
```

| Field | Description | Default |
|-------|-------------|---------|
| `enabled` | Enable the learning system. | `true` |
| `model` | Model for extraction and retrieval-prep LLM calls. Use a cheap model. | `anthropic:claude-haiku-4-5` |
| `backend` | `"file"` (default) or `"mcp"` for external memory tools. | `"file"` |

Intermediate-learning cadence (3 user messages), the 2,000-token active-pack cap, and its 512-token global-rule sub-cap are fixed constants, not knobs.

> **Strict config, template-provided values.** The supervisor config is strict: the `[supervisor]` section and its `[supervisor.learning]` table are **required** — removing them is a hard parse error, not a silent fall-back. Within `[supervisor.learning]`, an *omitted field* still takes the code default (e.g. `enabled` → `false` (learning OFF), `model` → the dated build `anthropic:claude-haiku-4-5-20251001`). Learning is on out of the box only because the shipped template sets `enabled = true` explicitly. See [Supervisor](14-supervisor.md) for the sibling mechanics.

### Orientation memory

Alongside lessons (the procedural *"do / avoid"*), the supervisor stores **orientation** — durable, descriptive understanding of the subject: how it works, key decisions, constraints. It rides the same backend under `memory_type = "orientation"` and is recalled as **working assumptions to verify**, never as truth, under its own `## Orientation` heading. It is part of learning — on whenever `[supervisor.learning]` is enabled, with fixed injection and decay bounds.

### Long-lived experience memory

A separate detached learner may emit one `memory_type = "experience"` record when a trajectory contains substantial non-obvious knowledge that would save several searches or failed attempts. The extra call is value-gated: verified/failed work needs real user plus tool evidence, while an outcome-unknown trajectory must also be large (at least eight tool results and 8,000 bounded transcript tokens). Routine sessions pay only for the existing short learner. Generic advice, activity logs, transient status, secrets, exact line numbers, and facts recoverable with one obvious search are rejected.

An experience is 150–600 words with Objective, Durable knowledge, Outcome and evidence, and Reuse conditions sections. It carries:

- the external trajectory outcome: `verified`, `failed`, or honestly `unknown`;
- 1–6 addressable `session://<session>/message/<n>` evidence handles, including real user/tool evidence;
- stable IDs of related short lessons or prior memories;
- a separate grounding-verifier verdict before storage. A rejected candidate gets at most one issue-driven repair and one final verification, then fails closed.

Failed trajectories may therefore produce failure-labelled experience records, while short user-backed lessons retain their existing quote-first verification contract.

## How It Works

### Lesson Scope

Every lesson is classified as either `scoped` or `global`, and the extraction LLM picks the scope for each one. It is instructed to be conservative: most lessons are `scoped`, and a lesson only becomes `global` when it is clearly about *how you work in general* rather than this task, project, or role.

| Scope | Stored in | Retrieved how |
|-------|-----------|---------------|
| `scoped` (default) | `learning/{project}/{role_base}/` | By relevance to your current request (hybrid keyword + embedding search) |
| `global` | `learning/_/` | Reconsidered for each active pack, ranked by importance, with no relevance gating — they always apply |

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
related: []
evidence: []
outcome: unknown
last_used: ""
use_count: 0
---
```

- `title` is a short summary auto-derived from the first ~80 characters of the content (trimmed to a word boundary).
- `scope` is `scoped` or `global` and determines which directory the file lives in.
- `last_used` and `use_count` change only when the specialist reports that the
  memory materially affected its work. Recall exposure alone is neutral.

Files are human-readable and editable. Delete a file to remove a lesson — or use the [`/learning` command](#managing-lessons-learning).

### Extraction

Extraction is triggered by:
- **`/done`** — extracts (if `supervisor.learning.enabled`) regardless of the compression result, and marks the session so `/exit` and Ctrl+D don't extract a second time.
- **Auto-compaction** — extracts during compression once the session has at least 3 user messages.
- **Session exit** — fire-and-forget extraction when the session ends naturally via `/exit`, `/quit`, or Ctrl+D. Skipped if `/done` already extracted during the session.

Extraction always runs **detached** (a background task with no cost tracked against your session) and is deliberately strict about what counts as a lesson:

1. **Decision gate.** The LLM first emits `<decision>LEARN</decision>` or `<decision>NONE</decision>`. On `NONE`, extraction stops immediately and nothing is parsed.
2. **Mandatory evidence.** Every `<lesson>` must carry an `evidence` attribute quoting the user verbatim. A lesson with no (or empty) evidence is silently dropped.
3. **At most 3 lessons** per extraction — one strong lesson beats three weak ones.
4. **Only user corrections and user-stated rules qualify** — explicit corrections, declared project conventions/preferences/constraints, or a repeated correction of the same mistake. Things the AI figured out on its own, one-off debugging steps, generic developer knowledge, and anything derivable by reading the codebase do **not** qualify.

Long-lived experiences are evaluated independently from that short-lesson decision. Their cited message handles are checked structurally, system-managed recall/steer messages are excluded from the transcript, and a separate verifier rejects unsupported or outcome-inflated records.

Confidence drives importance: `confidence=high` (a direct correction) → `importance 0.9`; anything else (a stated preference, `confidence=medium`) → `importance 0.6`.

**Dedup and supersede.** The extraction LLM receives a bounded, ID-labelled
view of existing scoped and global lessons. Identical content is skipped. A
refinement or reversal removes an older lesson only when the new quote-backed
candidate explicitly names its ID through `supersedes` and both records have
the same scope. Similarity alone never deletes a short user rule.

### Long-run retention

File-backed learning uses a two-watermark hot store with fixed internal token
budgets per scope and memory type. The soft watermark is 80% of the hard bound:

| Memory type | Scoped hard bound | Global hard bound |
|-------------|------------------:|------------------:|
| Short user-backed rules | 16,000 tokens | 4,000 tokens |
| Orientation | 24,000 tokens | 8,000 tokens |
| Experience | 48,000 tokens | 16,000 tokens |

Maintenance runs after detached extraction, never in the user-response hot
path. Crossing the hard watermark selects at most one similar
orientation/experience pair as a *candidate* and asks the learning model for a
shorter consolidation. Similarity only chooses what to review; it never proves
equivalence. A separate verifier must confirm that the replacement adds no
claim, hides no contradiction, preserves applicability/outcome boundaries, and
retains all non-duplicate constraints. Only then is the replacement stored and
the sources moved atomically to cold storage. The replacement keeps the source
IDs in `related`, unions their evidence, inherits the lower importance, and
does not strengthen confidence or outcome.

Short user-backed rules are never synthesized by this pass because a generated
merge would break their quote-first contract. They continue to change only
through explicit, separately verified extraction and `supersedes`.

After that single consolidation attempt, the lowest-utility records move to
`.archive/<memory_type>/` until the hot store is back at 80%. Utility combines
bounded importance, direct-use count, confidence, and last-use recency:

`U = 0.55I + 0.15C + 0.15 min(1, ln(1+uses)/ln(11)) + 0.15/(1+age_days/180)`

Here `I` is outcome-adjusted importance in `[0,1]`, `C` is `1` for high
confidence and `0.5` otherwise, and age is measured from `last_used` (falling
back to creation time). The logarithm rewards repeated demonstrated use without
letting frequency dominate correctness. Task relevance is deliberately absent
from eviction utility because maintenance has no current task; relevance stays
the admission signal during recall.

Cold files are retained losslessly and are
excluded from hot embedding recall. A compact append-only catalog keeps their
title, tags, and a short preview; exact lexical matches can page at most two
cold records into a request without embedding the archive. Long cold
experiences carry their real archive path in the Active Memory Pack, so the
specialist can open the full record. A cold record reported as materially used
is automatically promoted back to its hot scope before its use/outcome metadata
is updated. Moving a file back manually has the same effect. This hysteresis
prevents maintenance from moving one record on every extraction.

Independently of the hard budget, a scoped record that is both weak
(`importance <= 0.4`) and older than 90 days also moves to the same cold
archive. Repeated negative outcome credit that lowers importance to `0.1` does
the same immediately. Automatic retention never permanently deletes a file;
explicit `/learning delete` and `clear` remain destructive user actions.

### Active Memory Pack

Every genuine user turn **replaces** the previous runtime pack:

- **First message of the session** — global rules plus a full hybrid scoped recall are considered.
- **Each subsequent new user message** — global rules are reconsidered and scoped recall is embedding-only, with no retrieval-prep LLM call.
- **Tool follow-up rounds** — reuse the same pack without another retrieval.

The file backend may rank up to 20 scoped candidates and expands explicit relationships one hop in either direction, but only items fitting the exact 2,000-token pack budget reach the specialist; global rules may consume at most 512 of those tokens. Each selected item gets a short pack-local ID (`M1`, `M2`, …). The specialist reports only IDs that materially affected its answer or action in the hidden supervisor status, and verify-gate outcomes reinforce or weaken only those used items. Mere exposure receives no credit.

Long experience bodies are represented by a compact card (up to 320 inline tokens) plus the exact `.md` file path, outcome, evidence handles, and related IDs. The specialist can inspect the full file with its normal local reader when the card is insufficient; the full record is never silently lost to the injection budget.

The pack is materialized as a system-managed user-role message only around the provider request. It is removed immediately afterwards, never appended to the session log, never accumulated across turns, and rebuilt automatically on the next genuine request. If the bounded pack alone would cross the model's usable context ceiling, it is dropped for that turn rather than blocking the user's task.

### Retrieval (File Backend)

Scoped recall is a **hybrid search**: LLM-extracted keywords and short phrases
(sparse) are fused with embedding cosine similarity (dense) via Reciprocal Rank
Fusion (RRF, `k=60`), then reweighted by recency and learned importance. An
exact sparse phrase receives strong credit; when the phrase is absent, at least
two selective constituent terms must match, preventing one generic word from
admitting a memory.

Sparse and dense normally receive equal RRF weight. If one of the first three
sparse hits has learned importance below `0.4`, that query is treated as
correction-conflicted and sparse weight becomes `0.25`; dense outage always
restores full sparse ordering. One highest-importance sparse candidate may be
reserved at rank five when fusion buried it, preserving identifier and indirect
cue recall without letting lexical noise control ranks one through four.

Recency uses a 30-day half-life with up to a +50% boost; importance contributes
a bounded 0.75x–1.25x multiplier so relevance remains primary. Embedding
candidates below a `0.2` cosine floor are dropped as noise, and if the embedding
model isn't ready yet the cosine signal is silently skipped. The query-rewrite
output is accepted only as 3–5 short keyword lines; malformed or answer-like
responses fail safely to retrieval without rewritten patterns. The rewrite call
runs only on the **first** retrieval of a session; follow-up messages use
embedding-only recall.

### Managing Lessons (`/learning`)

The interactive `/learning` command lets you browse and prune lessons for the current role and project:

| Command | Effect |
|---------|--------|
| `/learning` | List lessons (page 1). |
| `/learning list [page]` | List a specific page. 15 lessons per page. |
| `/learning list *pattern*` | Filter by a glob pattern matched against content, title, and tags (e.g. `/learning list *auth*`). Combine with a page number. |
| `/learning show <index>` | Inspect the complete memory body, file path, outcome, evidence handles, and related IDs. Alias: `get`. |
| `/learning delete <index>` | Delete a lesson by its **1-based index** from the last list. Aliases: `rm`, `remove`. |
| `/learning clear` | Delete all hot and cold lessons for the current role + project scope; global rules are untouched. |

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

- **store** can map any lesson field: `content`, `title`, `memory_type`, `importance`, `confidence`, `tags`, `source`, `role`, `project`, `scope`, `created`, `related`, `evidence`, `outcome`, `last_used`, `use_count`.
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
