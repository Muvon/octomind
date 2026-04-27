# Cross-Session Learning

Octomind can extract and reuse lessons across sessions — mistakes corrected, patterns that worked, user preferences — so the same issues aren't repeated.

## Overview

The learning system has two phases:
1. **Extraction** — after `/done` (or during auto-compaction), an LLM analyzes the conversation and extracts generalizable lessons.
2. **Injection** — at session start, stored lessons for the current project and role are injected into the system prompt.

Lessons are scoped by **project first, then role** — project knowledge stays within the project, role filters further.

## Configuration

```toml
[learning]
enabled = false
model = "anthropic:claude-haiku-4-5-20251001"
backend = "file"
min_messages_for_intermediate = 3
max_inject = 5
```

| Field | Description | Default |
|-------|-------------|---------|
| `enabled` | Enable the learning system. | `false` |
| `model` | Model for extraction and retrieval LLM calls. Use a cheap model. | `anthropic:claude-haiku-4-5-20251001` |
| `backend` | `"file"` (default) or `"mcp"` for external memory tools. | `"file"` |
| `min_messages_for_intermediate` | Minimum user messages before intermediate learning triggers during auto-compaction. | `3` |
| `max_inject` | Maximum lessons injected into the system prompt per session. | `5` |

## How It Works

### Storage (File Backend)

Lessons are stored as markdown files with YAML frontmatter:

```
~/.local/share/octomind/learning/{project}/{role}/
  ├── 20260405143000-bearer-auth-required.md
  └── 20260405143001-custom-error-types.md
```

Each file:
```markdown
---
content: "Bearer token auth required for all API endpoints"
memory_type: learning
importance: 0.8
confidence: high
tags: [auth, api]
source: "260405-142040-octofs-25e37715"
role: "developer"
project: "octofs"
created: "2026-04-05T14:30:00Z"
---
```

Files are human-readable and editable. Delete a file to remove a lesson.

### Extraction

Triggered by:
- **`/done`** — always extracts, regardless of compression result.
- **Auto-compaction** — extracts during compression if the session has enough user messages (configurable via `min_messages_for_intermediate`).
- **Session exit** — fire-and-forget extraction when the session ends naturally (not via `/done`). Runs asynchronously with no cost tracking. Skipped if `/done` already extracted during the session.

The extraction LLM receives the conversation transcript plus existing lessons (for deduplication) and outputs structured `<lesson>` tags with confidence and tags.

### Injection

At session start, all lessons for the current `{project}/{role}` are read and appended to the system prompt under `## Lessons from Past Sessions`. They are cached with the system message — zero per-turn overhead.

## MCP Backend

For projects using external memory tools (e.g. octobrain), configure the MCP backend with field mapping:

```toml
[learning]
enabled = true
model = "anthropic:claude-haiku-4-5-20251001"
backend = "mcp"

[learning.store]
tool = "memorize"
[learning.store.field_map]
content = "content"        # required by memorize
title = "title"            # required by memorize — short summary
memory_type = "memory_type"
importance = "importance"
confidence = "source"      # maps confidence → octobrain's source trust tier
tags = "tags"
role = "role"
project = "project"

[learning.retrieve]
tool = "remember"
[learning.retrieve.field_map]
query = "query"            # string or array of search terms
memory_type = "memory_types" # passed as ["learning"] array to match octobrain schema
role = "role"
project = "project"
limit = "limit"            # octobrain max is 5
```

Each entry in `field_map` maps a canonical learning field to the MCP tool's actual argument name. Set a value to `""` to omit that field. Missing entries are also omitted.

Store and retrieve have separate field maps because MCP tools have different argument schemas.

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

Both can coexist. Learning focuses on what the agent got wrong or right and forces those lessons into every future session via the system prompt.
