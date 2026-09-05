# Cross-Session Learning

Use cross-session learning to carry your rules and grounded project knowledge into later sessions. This guide covers
configuration, inspection, retrieval, and retention for session users.

## Get started

With the shipped configuration, learning is enabled. State a durable rule in a session, then use `/done` to finish the
task and start background extraction:

```bash
octomind run developer:general
```

```text
In this repository, preserve public API names unless I explicitly request a rename.
/done
/learning list
/learning show 1
```

Extraction may produce no record if the evidence does not support a reusable memory. It runs in the background, so
repeat the list after it finishes. `show 1` requires at least one listed record.

The learning system has two phases:

1. **Extraction** — after `/done` (or during auto-compaction), an LLM analyzes the conversation and extracts a small
  number of lessons from your corrections and stated rules.
2. **Active packing** — before each genuine user turn, relevant stored lessons are selected into one bounded runtime
  pack that accompanies specialist requests for that turn.

Each lesson has a **scope** that decides where it lands and how it is retrieved:

- **`scoped`** (the default) — tied to a single project and role. Stored under `learning/{project}/{role_base}/` and
  retrieved by relevance to what you're working on right now.
- **`global`** — a durable, user-wide preference that applies in every project and role. Stored under `learning/_/` and
  hot records are considered for every replacement pack by importance; cold records require lexical matches.

So scoped lessons are organized **project first, then role** (project knowledge stays within the project, the role
filters it further), while global lessons deliberately cross both boundaries. See [Lesson Scope](#lesson-scope) for
details.

## Configuration

Learning is one mechanic of the **supervisor** — the out-of-band control plane around the agent loop — so its config
lives under `[supervisor.learning]`. See [`[supervisor]` in the config
reference](../reference/03-config-reference.md#supervisor) for the sibling sections (gate, plan, condense).

```toml
[supervisor.learning]
enabled = true

[supervisor.learning.evolution]
enabled = false
```

| Field | Description | Default |
|-------|-------------|---------|
| `enabled` | Enable the learning system. | `true` |

Learning does not own a separate model. Extraction, recall, verification, retention, and evolution all use
`[supervisor.model]`, which itself inherits omitted fields from `[model]`.

`[supervisor.learning.evolution]` has one field, `enabled` (default `false`). When enabled, the detached learner may
compile one highest-value grounded memory per extraction into a machine-local skill or guardrail candidate. Candidates
move through shadow, bounded trial, active, and rollback states; generated behavior never overwrites authored skills or
`.agents/guardrails.toml`.

The auto-compaction extraction minimum (3 user messages), the 2,000-token active-pack cap, and its 512-token global-rule
sub-cap are fixed constants, not knobs.

> **Strict config, template-provided values.** `[supervisor]` and its nested `learning`, `gate`, `plan`, and `condense`
> tables are required by deserialization. `LearningConfig::default()` has `enabled = false`, while the shipped template
> explicitly enables it. There is no `[supervisor.learning.model]`; all learning calls inherit `[supervisor.model]`,
> whose omitted fields inherit main `[model]`.

## Memory types

### Orientation memory

Alongside lessons (the procedural *"do / avoid"*), the supervisor stores **orientation** — durable, descriptive
understanding of the subject: how it works, key decisions, constraints. It rides the same backend under `memory_type =
"orientation"` and is recalled as **working assumptions to verify**, never as truth, in the pack’s orientation group. It
is part of learning — on whenever `[supervisor.learning]` is enabled, with fixed injection and decay bounds.

### Long-lived experience memory

A separate detached learner may emit one `memory_type = "experience"` record when a trajectory contains substantial
non-obvious knowledge that would save several searches or failed attempts. The extra call is value-gated:
verified/failed work needs real user plus tool evidence, while an outcome-unknown trajectory must also be large (at
least eight tool results and 8,000 bounded transcript tokens). Routine sessions pay only for the existing short learner.
Generic advice, activity logs, transient status, secrets, exact line numbers, and facts recoverable with one obvious
search are rejected.

An experience is 150–600 words with Objective, Durable knowledge, Outcome and evidence, and Reuse conditions sections.
It carries:

- the external trajectory outcome: `verified`, `failed`, or honestly `unknown`;
- 1–6 addressable `session://<session>/message/<n>` evidence handles, including real user/tool evidence;
- stable IDs of related short lessons or prior memories;
- a separate grounding-verifier verdict before storage. A rejected candidate gets at most one issue-driven repair and
  one final verification, then fails closed.

Failed trajectories may therefore produce failure-labelled experience records, while short user-backed lessons retain
their existing quote-first verification contract.

## Managing Lessons (`/learning`)

The interactive `/learning` command lets you browse and prune lessons for the current role and project:

The list header summarizes hot/cold item and token totals, local/global scope counts, and per-type hot/cold counts.
Individual rows stay compact; use `show` for full provenance and retention metadata.

| Command | Effect |
|---------|--------|
| `/learning` | List lessons (page 1). |
| `/learning list [page]` | List a specific page. 15 lessons per page. |
| `/learning list *pattern*` | Filter by a glob pattern matched against content, title, and tags (e.g. `/learning list *auth*`). Combine with a page number. |
| `/learning show <index>` | Inspect the complete memory body, file path, outcome, evidence handles, and related IDs. Alias: `get`. |
| `/learning delete <index>` | Delete a lesson by its **1-based index** in the current **unfiltered** hot list. Aliases: `rm`, `remove`. |
| `/learning clear` | Delete all hot and cold lessons for the current role + project scope; global rules are untouched. |
| `/learning evolution` | List evolved behavior matching the current project/domain. |
| `/learning evolution show <id>` | Inspect scope, provenance, native artifact, trials, and history. |
| `/learning evolution approve\|reject\|rollback <id>` | Explicitly control a generated behavior lifecycle. |

The unfiltered list covers current scoped hot records followed by global hot records, each sorted by importance. `show`
and `delete` reload that unfiltered list: filtered row numbers are not safe to reuse, and indices may change when
background learning updates the store. Re-list without a filter and inspect the entry before deleting it. `clear` only
wipes the current role+project scope. See [Session Commands](../reference/02-session-commands.md) for the full command
reference.

For example, browse matches, then return to the unfiltered list before inspecting or deleting an entry:

```text
/learning list *auth* 1
/learning list
/learning show 1
/learning delete 1
```

To inspect generated behavior, use the ID returned by the evolution list (replace `CANDIDATE_ID`):

```text
/learning evolution
/learning evolution show CANDIDATE_ID
/learning evolution approve CANDIDATE_ID
```

`approve` moves only a shadow candidate into trial. `reject` rejects a record; `rollback` moves a trial or active record
back to shadow. To remove all scoped hot and cold memories:

```text
/learning clear
```

To reject or roll back a listed behavior, substitute its ID:

```text
/learning evolution reject CANDIDATE_ID
/learning evolution rollback CANDIDATE_ID
```

## Common questions

**Why is a lesson missing?** Extraction is asynchronous and quote-backed rules must pass verification. Recall is
relevance- and budget-limited, so a stored item need not appear in every pack. Inspect the store, then enable debug
logging before a new request to see the actual pack:

```text
/learning list
/loglevel debug
Review the API authentication rules for this repository.
/loglevel info
```

**Why did a filtered delete target another item?** `show` and `delete` use the current unfiltered list. Always run
`/learning list` and inspect the matching unfiltered index immediately before deletion.

**Does exiting guarantee extraction finishes?** CLI exit starts a child process and returns immediately. Closing the
terminal can terminate that child before it stores the memories.

## Retrieval and storage reference

### Lesson Scope

Every lesson is classified as either `scoped` or `global`, and the extraction LLM picks the scope for each one. It is
instructed to be conservative: most lessons are `scoped`, and a lesson only becomes `global` when it is clearly about
*how you work in general* rather than this task, project, or role.

| Scope | Stored in | Retrieved how |
|-------|-----------|---------------|
| `scoped` (default) | `learning/{project}/{role_base}/` | By relevance to your current request (hybrid keyword + embedding search) |
| `global` | `learning/_/` | Reconsidered for each active pack, ranked by importance, within the pack budget; cold records require lexical matches |

A worked example: you tell the agent *"always open a single PR"* while working in project `octofs` as
`developer:general`. That is a general working preference, so the extractor may classify it as a **global** lesson
stored in `learning/_/`. Later you tell it *"in this repo, all API endpoints require bearer auth"* — that is specific to
this project, so a grounded extracted rule is **scoped** and lands in `learning/octofs/developer/` (note the role is
truncated at `:` to its base, `developer`).

### Storage (File Backend)

Scoped lessons are stored as markdown files with YAML frontmatter, one file per lesson, in a project/role directory;
global lessons go in the shared `_` directory:

```text
~/.local/share/octomind/learning/
  ├── octofs/developer/              # scoped: {project}/{role_base}
  │   ├── 20260405143000-bearer-auth-required.md
  │   └── 20260405143001-custom-error-types.md
  └── _/                             # global: cross-project, cross-role
      └── 20260405150000-always-single-pr.md
```

The role component is the **base part before `:`** — a lesson from role `developer:general` is stored under
`developer/`, while the project component is the working directory’s basename.

On macOS and Linux the default data root is `~/.local/share/octomind`; on Windows it is `%LOCALAPPDATA%/octomind`.
`OCTOMIND_DATA_DIR` overrides it, including config and learning storage:

```bash
OCTOMIND_DATA_DIR="$HOME/octomind-personal" octomind run
```

Each file carries the full frontmatter the backend writes, in this exact order:

```text
---
title: "Bearer token auth required for all API endpoints"
content: "Bearer token auth required for all API endpoints"
memory_type: learning
importance: 0.9
confidence: high
tags: [auth, api]
source: "260405-142040-octofs-25e37715"
role: "developer:general"
project: "octofs"
scope: scoped
created: "2026-04-05T14:30:00Z"
related: []
evidence: ["session://260405-142040-octofs-25e37715/message/1"]
outcome: unknown
last_used: ""
use_count: 0
---
```

- `title` is a short summary auto-derived from the first 80 UTF-8 bytes of short-rule content, cut safely at a
  character/word boundary.
- `scope` is `scoped` or `global` and determines which directory the file lives in.
- `last_used` and `use_count` change only when the specialist reports that the memory materially affected its work.
  Recall exposure alone is neutral.

Files are human-readable and editable. Delete a file to remove a lesson — or use the [`/learning`
command](#managing-lessons-learning).

### Extraction

Extraction is triggered by:

- **`/done`** — extracts (if `supervisor.learning.enabled`) regardless of the compression result, and marks the session
  so `/exit` and Ctrl+D don't extract a second time.
- **Auto-compaction** — extracts during compression once the session has at least 3 user messages.
- **Interactive CLI exit** — a detached `octomind distill` child performs extraction when the session ends naturally via
  `/exit`, `/quit`, or Ctrl+D. Skipped if `/done` already extracted during the session.

Extraction runs **detached** (an in-process task for `/done`/compaction, a child process for CLI exit, with in-process
model costs folded into session spending; exit-child spending is separate) and is deliberately strict about what counts
as a lesson:

1. **Decision gate.** The LLM first emits `<decision>LEARN</decision>` or `<decision>NONE</decision>`. On `NONE`,
  short-lesson parsing stops; orientation and experience are evaluated independently.
2. **Mandatory evidence.** Every `<lesson>` must carry an `evidence` attribute quoting the user verbatim. Missing
  evidence is dropped; the quote must match a real user turn and pass a separate support verifier.
3. **At most 3 lessons** per extraction — one strong lesson beats three weak ones.
4. **Only user corrections and user-stated rules qualify** — explicit corrections, declared project
  conventions/preferences/constraints, or a repeated correction of the same mistake. Things the AI figured out on its
  own, one-off debugging steps, generic developer knowledge, and anything derivable by reading the codebase do **not**
  qualify.

Long-lived experiences are evaluated independently from that short-lesson decision. Their cited message handles are
checked structurally, system-managed recall/steer messages are excluded from the transcript, and a separate verifier
rejects unsupported or outcome-inflated records.

Confidence drives importance: `confidence=high` (a direct correction) → `importance 0.9`; anything else (a stated
preference, `confidence=medium`) → `importance 0.6`.

**Dedup and supersede.** The extraction LLM receives a bounded, ID-labelled view of existing scoped and global lessons.
Identical content is skipped. A refinement or reversal removes an older lesson only when the new quote-backed candidate
explicitly names its ID through `supersedes` and both records have the same scope. Similarity alone never deletes a
short user rule.

### Long-run retention

File-backed learning uses a two-watermark hot store with fixed internal token budgets per scope and memory type. The
soft watermark is 80% of the hard bound:

| Memory type | Scoped hard bound | Global hard bound |
|-------------|------------------:|------------------:|
| Short user-backed rules | 16,000 tokens | 4,000 tokens |
| Orientation | 24,000 tokens | 8,000 tokens |
| Experience | 48,000 tokens | 16,000 tokens |

Maintenance runs after detached extraction, never in the user-response hot path. Crossing a bucket’s hard watermark
selects at most one similar orientation/experience pair per scope and memory type as a *candidate* and asks the
supervisor model for a shorter consolidation. Similarity only chooses what to review; it never proves equivalence. A
separate verifier must confirm that the replacement adds no claim, hides no contradiction, preserves
applicability/outcome boundaries, and retains all non-duplicate constraints. Only then is the replacement stored and the
source records archived through atomic file writes and moves. The replacement keeps the source IDs in `related`, unions
their evidence, inherits the lower importance, and does not strengthen confidence or outcome.

Short user-backed rules are never synthesized by this pass because a generated merge would break their quote-first
contract. They continue to change only through explicit, separately verified extraction and `supersedes`.

After that single consolidation attempt, the lowest-utility records move to `.archive/<memory_type>/` until the hot
store is back at 80%. Utility combines bounded importance, direct-use count, confidence, and last-use recency:

`U = 0.55I + 0.15C + 0.15 min(1, ln(1+uses)/ln(11)) + 0.15/(1+age_days/180)`

Here `I` is outcome-adjusted importance in `[0,1]`, `C` is `1` for high confidence and `0.5` otherwise, and age is
measured from `last_used` (falling back to creation time). The logarithm rewards repeated demonstrated use without
letting frequency dominate correctness. Task relevance is deliberately absent from eviction utility because maintenance
has no current task; relevance stays the admission signal during recall.

Cold files are retained losslessly and are excluded from hot embedding recall. A compact append-only catalog keeps their
title, tags, and a short preview; exact lexical matches can page at most two cold records per scoped/global retrieval
into consideration without embedding the archive. Long cold experiences carry their real archive path in the Active
Memory Pack, so the specialist can open the full record. A cold record reported as materially used is automatically
promoted back to its hot scope before its use/outcome metadata is updated. This hysteresis prevents maintenance from
moving one record on every extraction.

Independently of the hard budget, a scoped record that is both weak (`importance <= 0.4`) and older than 90 days also
moves to the same cold archive. Repeated negative outcome credit that lowers importance to `0.1` does the same
immediately. Automatic retention never permanently deletes a file; explicit `/learning delete` and `clear` remain
destructive user actions.

### Active Memory Pack

Every genuine user turn **replaces** the previous runtime pack:

- **First message of the session** — global rules plus a full hybrid scoped recall are considered.
- **Each subsequent new user message** — global rules are reconsidered and scoped recall is embedding-only, with no
  retrieval-prep LLM call.
- **Tool follow-up rounds** — reuse the same pack without another retrieval.

The file backend may rank up to 20 scoped candidates and expands explicit relationships one hop in either direction, but
only items fitting the 2,000-token pack budget under Octomind’s token estimator reach the specialist; global rules may
consume at most 512 of those tokens. Each selected item gets a short pack-local ID (`M1`, `M2`, …). The specialist
reports only IDs that materially affected its answer or action in the hidden supervisor status, and verify-gate outcomes
reinforce or weaken only those used items. Mere exposure receives no credit.

Long experience bodies are represented by a compact card (up to 320 inline tokens) plus the exact `.md` file path,
outcome, evidence handles, and related IDs. The specialist can inspect the full file with its normal local reader when
the card is insufficient; selected records keep their full body on disk even when only the card fits.

The pack is materialized as a system-managed user-role message only around the provider request. It is removed
immediately afterwards, never appended to the session log, never accumulated across turns, and rebuilt automatically on
the next genuine request. If the bounded pack alone would cross the model's usable context ceiling, it is dropped for
that turn rather than blocking the user's task.

### Retrieval (File Backend)

Scoped recall is a **hybrid search**: LLM-extracted keywords and short phrases (sparse) are fused with embedding cosine
similarity (dense) via Reciprocal Rank Fusion (RRF, `k=60`), then reweighted by recency and learned importance. An exact
sparse phrase receives strong credit; when the phrase is absent, at least two selective constituent terms must match,
preventing one generic word from admitting a memory.

Sparse and dense normally receive equal RRF weight. If one of the first three sparse hits has learned importance below
`0.4`, that query is treated as correction-conflicted and sparse weight becomes `0.25`; dense outage always restores
full sparse ordering. One highest-importance sparse candidate may be reserved at rank five when fusion buried it,
preserving identifier and indirect cue recall without letting lexical noise control ranks one through four.

Dense retrieval keeps a short memory as one unchanged embedding input. A long heterogeneous memory is divided at
semantic line/paragraph boundaries into bounded 128-token chunks, with title and tags attached to every chunk; the
memory's dense score is its strongest chunk match. This late interaction keeps small facts from being diluted by
unrelated sections while preserving the legacy score exactly for ordinary one-chunk lessons.

Recency uses a 30-day half-life with up to a +50% boost; importance contributes a bounded 0.75x–1.25x multiplier so
relevance remains primary. Embedding candidates below a `0.2` cosine floor are dropped as noise, and if the embedding
model isn't ready yet the cosine signal is silently skipped. The query-rewrite output is accepted only as 3–5 short
keyword lines; malformed or answer-like responses fail safely to retrieval without rewritten patterns. The rewrite call
runs on the **first** retrieval and after `/done` resets recall; an empty scoped store skips it; follow-up messages use
embedding-only recall.

With `/loglevel debug`, retrieval prints the accepted query-rewrite keywords and the exact final Active Memory Pack
after context-headroom checks, immediately before it is materialized for the provider request. Normal and info logging
keep showing only compact retrieval and pack totals.

## Relationship to Memory

Learning is **separate from external memory MCP tools**:

- **External memory tools** may provide broad context storage — code patterns, architecture, project state, references.
- **Learning** is narrow and structured — actionable facts scored by confidence, extracted from outcomes, with
  deduplication.

Both can coexist. Supervisor learning is always file-backed and owns its verified retention lifecycle; external memory
tools remain independent MCP tools the specialist may use directly. Learning focuses on the corrections and rules you
gave the agent, and surfaces relevant ones automatically.

## Source reference

| Surface | Source |
|---------|--------|
| Defaults and model ownership | [config-templates/default.toml](../../config-templates/default.toml), [src/config/model.rs](../../src/config/model.rs) |
| Extraction, evidence, and exit child | [src/supervisor/learning/extract.rs](../../src/supervisor/learning/extract.rs) |
| Pack and retrieval | [src/supervisor/learning/inject.rs](../../src/supervisor/learning/inject.rs), [src/supervisor/learning/backend/file.rs](../../src/supervisor/learning/backend/file.rs) |
| Retention and evolution | [src/supervisor/learning/retention.rs](../../src/supervisor/learning/retention.rs), [src/supervisor/learning/evolution/runtime.rs](../../src/supervisor/learning/evolution/runtime.rs) |
| Commands and paths | [src/session/chat/session/commands/learning.rs](../../src/session/chat/session/commands/learning.rs), [src/directories.rs](../../src/directories.rs) |

## See also

- [Supervisor](14-supervisor.md)
- [Skills](15-skills.md)
- [Guardrails](18-guardrails.md)
- [Session commands](../reference/02-session-commands.md)
- [Configuration reference](../reference/03-config-reference.md)
