<div align="center">
  <a href="https://octomind.run" target="_blank">
    <img src="assets/logo.svg" width="640" alt="Octomind — AI Agent Runtime" />
  </a>
  <br /><br />
  <strong>Install agents, not frameworks.</strong><br />
  <em>Open-source runtime for specialist AI agents. One command. Any model. Any domain.</em>
  <br /><br />

  [![License](https://img.shields.io/badge/license-Apache%202.0-7c3aed?style=flat-square)](LICENSE)
  [![Version](https://img.shields.io/crates/v/octomind?style=flat-square&color=7c3aed)](https://crates.io/crates/octomind)
  [![GitHub stars](https://img.shields.io/github/stars/muvon/octomind?style=flat-square&color=7c3aed)](https://github.com/muvon/octomind/stargazers)
  [![Website](https://img.shields.io/badge/website-octomind.run-7c3aed?style=flat-square)](https://octomind.run)

  <br />

  [Documentation](https://octomind.run/docs/) · [Tap Registry](https://github.com/muvon/octomind-tap) · [Website](https://octomind.run)
</div>

---

## Table of Contents

- [The Problem](#the-problem)
- [Five Pillars](#five-pillars)
- [Pillar 1 — Zero Config, Full Flexibility](#pillar-1--zero-config-full-flexibility)
- [Pillar 2 — Sessions That Stay Sharp at Hour 4](#pillar-2--sessions-that-stay-sharp-at-hour-4)
- [Pillar 3 — Cost as a Control Plane](#pillar-3--cost-as-a-control-plane)
- [Pillar 4 — Guardrails: Policy as Code, Not Approval Clicks](#pillar-4--guardrails-policy-as-code-not-approval-clicks)
- [Pillar 5 — Intent-Driven Context: Skills Activate on What You Mean](#pillar-5--intent-driven-context-skills-activate-on-what-you-mean)
- [Quick Start](#quick-start)
- [How It Works](#how-it-works)
- [Power Users — Roles, Workflows, Layers](#power-users--roles-workflows-layers)
- [Embedders — ACP, WebSocket, Daemon](#embedders--acp-websocket-daemon)
- [Installation](#installation)
- [Configuration](#configuration)
- [Architecture](#architecture)
- [Contributing](#contributing)
- [License](#license)

---

## The Problem

Building a specialist AI agent in 2026 means stitching together three different tools, writing glue code nobody wants to own, and praying it holds. You spend 45 minutes wiring MCP servers, system prompts, tool configs, and credentials — every domain, every machine, every time.

- **Config Wars.** No central registry. Skills here, MCP servers there, agent configs nowhere. The community calls it ["solidarity in frustration."](https://dev.to/satinathnit/the-agent-config-wars-why-your-ai-agent-documentation-is-already-obsolete-4d6i)
- **Generic AI hallucinates in expert domains.** ChatGPT writes wrong drug dosages. Lawyers cite cases that don't exist. Multi-agent specialization is now [the default architecture](https://dev.to/aibughunter/ai-agents-in-april-2026-from-research-to-production-whats-actually-happening-55oc) for serious work.
- **One generic assistant for every task.** Rust debugging, blood-test interpretation, contract review — same prompt, same tools. Drift compounds.
- **Sessions break at hour 4.** Naive truncation drops the decisions you need. Quality collapses. You restart.
- **Bills surprise you.** Cursor users posting $7K daily overages. No per-task budget, no kill switch.

Octomind ships specialist agents ready to run — and a runtime that grows with you.

---

## Five Pillars

| Pillar | What it gives you |
|---|---|
| **Zero config, full flexibility** | `octomind run lawyer:sg` works out of the box. Need a different model, MCP server, or guardrail pipe? Same TOML, no framework code. |
| **Sessions stay sharp at hour 4** | Adaptive compaction: cache-aware, structurally preserving. Smaller context = faster responses + lower cost. |
| **Cost as a control plane** | Per-step model selection across many providers. Hard spending caps and cache-aware accounting come for free. |
| **Guardrails: policy as code** | Govern autonomous agents with deterministic scripts — pre-call guards, post-result hooks, post-turn validators. No modal approval clicks. Fits CI. |
| **Intent-driven context** | Skills and capabilities activate only when what you're asking for matches them. Smaller context by default, lower cost, no surprise tools. |

---

## Pillar 1 — Zero Config, Full Flexibility

Most agent tools force a tradeoff: zero-config (Lindy, no-code) or fully customizable (Mastra, LangGraph). Octomind gives you both. `octomind run lawyer:sg` works out of the box. Need a different model, a custom MCP server, a guardrail pipe — all live in TOML, no framework code.

```bash
octomind run developer            # general dev, language skills auto-activate
octomind run doctor:blood         # blood-test interpretation specialist
octomind run doctor:nutrition     # nutrition specialist
```

### What happens when you run a specialist

```
→ Fetches the agent manifest from the tap registry
→ Installs required binaries automatically (skips if already present)
→ Prompts once for any credentials, saves permanently
→ Spins up the right MCP servers for this domain
→ Loads specialist model config, system prompt, tool permissions
→ Ready in ~5 seconds, not 45 minutes
```

This is **packaged expertise** — not a prompt file, not a skill injection. The full stack, configured by the community, ready to run.

### Specialists grow at runtime

Every agent has built-in power tools that let it acquire new capabilities and spawn sub-agents mid-session, without restart:

| Tool | What it does |
|---|---|
| `tap` | Delegate to any specialist role from the tap registry. Foreground for an inline reply or background for long tasks. |
| `mcp` | Enable or disable MCP servers on the fly. Agent picks the server it needs and registers it mid-conversation. |
| `agent` | Spawn a specialist sub-agent for a sub-task. Sub-agent runs, returns, parent continues. |

```
User: "Cross-reference our Postgres metrics with the deployment log"

Agent:
  → mcp.enable(postgres-mcp)        # auto-detected need, no user prompt
  → agent.spawn(log_reader)         # delegates log parsing
  → results merge mid-session
  → mcp.disable(postgres-mcp)       # cleans up
  → presents the analysis
```

Most agent harnesses pre-load every available tool into context. Octomind starts focused for the domain and grows only when needed. **Smaller context, lower cost, faster responses, no surprise tools.** See [Pillar 5](#pillar-5--intent-driven-context-skills-activate-on-what-you-mean) for how activation actually works.

### Add your own taps

```bash
octomind tap yourteam/tap                 # clones github.com/yourteam/octomind-tap
octomind tap yourteam/internal ~/path     # local tap for private agents

octomind run finance:analyst              # available immediately
octomind run security:owasp
```

Each tap is a Git repo. Each agent is one TOML file. Pull requests are contributions.

> Want to publish your expertise? A `doctor:medications`, a `lawyer:us`, a `devops:terraform`. One file, and everyone with that problem gets a specialist instantly. [How to write a tap agent →](https://github.com/muvon/octomind-tap)

---

## Pillar 2 — Sessions That Stay Sharp at Hour 4

Every coding agent degrades after a few hours. Context fills. Decisions get truncated. The agent forgets why it started.

Octomind's adaptive compaction engine runs automatically:

- **Cache-aware** — calculates if compaction is worth it *before* paying for it. Never breaks the prompt-cache hit by accident.
- **Pressure-tiered** — compacts more aggressively as context grows.
- **Structurally preserving** — keeps decisions, file references, errors, dependencies; drops noise.
- **Plan-aware and free-form-aware** — works whether you use the `plan` tool or have a free-form chat.
- **Fully automatic** — you never think about it.

The second-order benefit: smaller context means **fewer tokens, faster responses, lower cost** every turn after compaction fires. The three pillars compound.

> Work on a hard problem for 4 hours. The agent still knows what it decided in hour one.

---

## Pillar 3 — Cost as a Control Plane

Pick the right model for each step. A cheap one for routine research, a frontier one for review — per-role, per-step, mid-session swap. Real-time cost tracking and hard spending caps come for free.

```toml
# Per-role model selection — pay Opus only where it's worth it
[[roles]]
name = "researcher"
model = "openrouter:google/gemini-2.5-flash"   # cheap broad context

[[roles]]
name = "reviewer"
model = "anthropic:claude-opus-4-7"            # precision where it counts

# Hard spending limits — enforced, not advisory
max_request_spending_threshold = 0.50    # USD per request
max_session_spending_threshold = 5.00    # USD per session
```

- Per-role and per-layer model selection across many providers via [octolib](https://github.com/muvon/octolib) — different roles can run on different vendors.
- Mid-session model swap with `/model anthropic:claude-haiku-4-5`.
- Real-time cost tracking per request and per session.
- Cache-aware token accounting (`cache_read_tokens`, `cache_write_tokens` separated from input/output).
- Hard spending thresholds with enforcement — agent stops, falls back, or warns before the bill.

> Cursor users get $7,000 surprise bills. Octomind agents trip a budget and stop, fall back, or warn — before the bill, not after.

---

## Pillar 4 — Guardrails: Policy as Code, Not Approval Clicks

Other agent CLIs make the human the safety layer: every dangerous tool call pops a modal, every file write waits for a click. That works for one developer at the keyboard. It breaks the moment you point an agent at a long-running task, a CI job, or an autonomous loop.

Octomind takes the opposite position. **Policy lives in scripts, not prompts.** Drop a `.agents/guardrails.toml` in your repo and the runtime enforces it deterministically — pre-call, post-result, post-turn.

```toml
# Pre-call deny — block a class of calls before they execute
[[guard]]
match   = "shell(command=^rm\\s+-rf?)"
message = "rm -rf blocked."

# Conditional rule — only fires after the agent ran git status this session
[[guard]]
match   = "shell(command=git push)"
when    = ["+shell(command=git status)"]
message = "Review changes before pushing."

# Post-result hook — non-zero exit injects feedback into the agent's inbox
[[hook]]
match  = "text_editor(path=src/.*\\.rs)"
on     = "success"
script = ".agents/check-clippy.sh"

# Post-turn validator — fires only over the call slice since it last ran
[[validator]]
name   = "tests-pass"
roles  = ["developer"]
script = ".agents/run-tests.sh"
```

- **Guards** — pre-call deny rules. Match by `capability(arg_name=regex)`, gate by history (`+used` / `-unused`), require loaded capabilities (`has = [...]`). The agent never even attempts a blocked call.
- **Hooks** — post-result scripts. Run after each tool returns. Non-zero exit injects stdout into the agent's inbox as a user message — clippy errors, lint failures, format diffs become *automatic corrections without restarting the turn*.
- **Validators** — post-turn scripts. Fire only over the new call-log slice (cursor-based, never re-fires on old activity). Filter by role. Output is wrapped in `<validation>` blocks the agent reads on its next turn. **This is what replaces "approve this change?" prompts in autonomous loops.**

The DSL is richer than competitor lifecycle hooks: capability+arg-regex+history+role+result-regex in one declarative file. No code to compile, no plugin to install. **Designed for full automation: fits CI, daemons, scheduled runs, ACP sub-agents.**

> The world is going autonomous. The choice isn't "ask vs auto" — it's "auto with deterministic policy" vs "auto with hope." Octomind ships the former.

---

## Pillar 5 — Intent-Driven Context: Skills Activate on What You Mean

Every other agent CLI loads every tool, every skill, every instruction pack into context up front. The model sees fifty tool definitions and a wall of system prompts before the user types a single character. **Token bills follow. Cache misses follow. Confused tool selection follows.**

Octomind inverts this. Skills and capabilities sit dormant until the user's intent matches them — then they activate, inject their content, and stay only as long as they're relevant. **Context is a function of what the user is actually trying to do.**

### How activation works (no keyword guessing)

Skills describe what they're for. The runtime matches your prompt against those descriptions and only loads the skills that fit:

- **Meaning, not keywords.** A dedicated embedding model — trained on activation traffic — scores how well your request matches each skill. "Help me refactor this auth flow" and "the login is broken" both find the same skill; "what's the weather" finds none.
- **Hand-authored rules where precision matters.** Skill authors can pin activation to file names, file contents, or exact phrases when they know better than a similarity score.
- **Abstain on near-ties.** When two skills score close, **neither fires.** Better to load nothing than the wrong thing.
- **Calibrated to skip, not guess.** Wrong activations bloat context and waste tokens. The system defaults to silence when in doubt.

### Why this matters

```
Other agent CLIs:                       Octomind:
─────────────────────                   ──────────
1. User starts session                  1. User starts session
2. Load 50 tools into context           2. Load 5 core tools into context
3. Load 30 skills into system prompt    3. Skills sit dormant
4. User types one sentence              4. User types one sentence
5. Model picks tool from a wall         5. Embed model scores → 1 skill matches
                                           → skill content injected
                                           → 6 tools in context, not 80
                                        6. Skill goes silent again when no longer relevant
```

**Smaller context = faster first token, lower cost per turn, fewer wrong tool calls.** The same prompt that costs $0.12 in a preloaded-everything CLI costs $0.03 here.

### What this combines with

- **`mcp.enable` mid-session.** Even when a skill activates, the underlying MCP server only spins up if the skill actually calls it. Inactive servers = zero token cost.
- **Compression interplay (Pillar 2).** A deactivated skill is dropped during compaction — its content is recoverable on next activation, not pinned forever.
- **Guardrails (Pillar 4).** A guard can require `has = ["filesystem-read"]` and only fire when that capability is currently loaded. Policy and activation share the same capability namespace.

> Most "agentic" CLIs pretend context is free. Octomind treats it as the scarcest resource in the system — and only spends it on what the user actually meant.

---

## Quick Start

```bash
# Install (macOS & Linux)
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash

# One API key gets you many providers
export OPENROUTER_API_KEY="your_key"

# Start with a specialist — no setup required
octomind run developer
```

```
octomind v0.29.0 · role: developer · model: openrouter:...
> _
```

You're in an interactive session with a specialist that can read your code, run commands, edit files, and grow capabilities as needed.

---

## How It Works

### Built-in MCP tools (every agent has these)

| Tool | Purpose |
|---|---|
| `plan` | Structured multi-step task tracking |
| `mcp` | Enable/disable MCP servers at runtime |
| `agent` | Spawn specialist sub-agents mid-session |
| `schedule` | Inject messages at future times |
| `skill` | Inject reusable instruction packs from taps |
| `tap` | Delegate to any specialist role from a tap registry |

### Filesystem tools (via [octofs](https://github.com/muvon/octofs))

`view`, `text_editor`, `batch_edit`, `shell`, `semantic_search`, `structural_search`, `workdir` — file operations come from the companion octofs MCP server, included by default in tap formulas that need them.

### Brain (via [octobrain](https://github.com/muvon/octobrain))

`memorize`, `remember`, `forget`, `knowledge`, `relate`, `memory_graph` — long-term memory, knowledge indexing, and relationship graphs. The companion octobrain MCP server is included by default in taps that need persistent context across sessions.

### Providers

Octomind supports many providers — OpenRouter, OpenAI, Anthropic, Google, DeepSeek, Amazon Bedrock, Cloudflare, and more — via [octolib](https://github.com/muvon/octolib). New providers added there become available in Octomind automatically. See [Providers & Models](doc/usage/04-providers.md) for the current list and supported models.

Switch providers mid-session with `/model anthropic:claude-sonnet-4-6`. Mix providers across roles — cheap model for research, best model for execution. Cost tracked separately per provider.

---

## Power Users — Roles, Workflows, Layers

For most users, taps are enough. For teams and power users, the configuration system is deep — **all TOML, no code**.

```toml
# Per-role: independent model, temperature, MCP servers, tools, system prompt
[[roles]]
name = "senior-reviewer"
model = "anthropic:claude-opus-4-7"
temperature = 0.2
[roles.mcp]
server_refs = ["filesystem", "github"]
allowed_tools = ["view", "ast_grep", "create_pr"]

# Workflows — multi-step, each step its own model and toolset
[[workflows]]
name = "deep_review"
[[workflows.steps]]
name = "analyze"
layer = "context_researcher"     # gemini-flash, broad context
[[workflows.steps]]
name = "critique"
layer = "senior_reviewer"        # claude-opus, precision

# Sandbox — lock all writes to current directory
sandbox = true
```

- **Roles** — model, temperature, system prompt, MCP servers, tool permissions per role.
- **Layers** — chained AI sub-agents that run after each response.
- **Guardrails** — deterministic policy (guards, hooks, validators) and input pipes.
- **Workflows** — multi-step orchestrated task runners with validation loops.

See [Configuration Reference](doc/reference/03-config-reference.md) for everything.

---

## Embedders — ACP, WebSocket, Daemon

Octomind isn't just a CLI. It runs in every context an agent needs to live in:

| Mode | Use for |
|---|---|
| Interactive CLI | Daily work, any domain |
| `--format jsonl` pipe | CI/CD pipelines, shell scripts, automation |
| `--daemon` + `send` | Background agents, continuous monitoring, long-running tasks |
| WebSocket server | IDE plugins, web dashboards, external integrations |
| ACP protocol | Multi-agent orchestration, being called by other agents |

```bash
# ACP — drop into any multi-agent system as a sub-agent
octomind acp developer:general

# Non-interactive — single message, plain output
octomind run developer "Explain the auth module" --format plain

# Structured JSON output for pipelines
octomind run developer "List TODO items" --schema todos.json --format jsonl
```

One binary. Every workflow.

---

## Installation

### One-line install

```bash
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash
```

Detects OS and architecture, installs to `~/.local/bin/`. macOS and Linux supported. Single Rust binary, no runtime dependencies.

### Cargo

```bash
cargo install octomind
```

Requires Rust 1.95+. See [Building from Source](doc/dev/01-building-from-source.md).

### Build from source

```bash
git clone https://github.com/muvon/octomind.git
cd octomind
cargo build --release
```

### API keys

```bash
# OpenRouter — recommended, access to many providers with one key
export OPENROUTER_API_KEY="your_key"

# Or any specific provider
export OPENAI_API_KEY="your_key"
export ANTHROPIC_API_KEY="your_key"
export DEEPSEEK_API_KEY="your_key"
```

Add to `~/.bashrc` or `~/.zshrc` for persistence.

### Verify

```bash
octomind --version
octomind config       # generate default config
octomind run          # start your first session
```

---

## Configuration

Config lives at `~/.local/share/octomind/config/config.toml`.

```bash
octomind config --show          # view current config
octomind config --validate      # validate config
```

Key areas:

- **Roles** — model, temperature, system prompt, MCP servers, tool permissions
- **Workflows** — multi-step AI processing with validation loops
- **Guardrails** — deterministic policy (guards, hooks, validators) and input pipes
- **MCP Servers** — external tools and capabilities
- **Spending Limits** — per-request and per-session thresholds

Full reference: [Configuration Reference](doc/reference/03-config-reference.md).

### Session commands

| Command | Description |
|---|---|
| `/help` | Show all commands |
| `/info` | Token usage and costs |
| `/model <provider:model>` | Switch model mid-session |
| `/effort <level>` | Set reasoning effort (low/medium/high/xhigh/max) |
| `/role <name>` | Switch role mid-session |
| `/session` | Manage saved sessions (sessions auto-save) |
| `/exit` | Exit session |

Full list: [Session Commands](doc/reference/02-session-commands.md).

---

## Architecture

One binary. The session is the unit of work. Around it: roles (who's talking), layers and workflows (multi-step orchestration), guardrails with pipes (deterministic pre-processing and policy), adaptive compaction (long-session quality), and MCP servers (tools). All of it driven by a single resolved TOML config — no hardcoded behavior, no framework code to edit.

Embedders pick their surface: interactive CLI, ACP for multi-agent orchestration, WebSocket for IDEs and dashboards, daemon mode for long-running background agents.

See [Architecture](doc/dev/02-architecture.md) for internals.

---

## Contributing

The most impactful contribution isn't code — **it's specialist agents.**

Every domain expert who publishes a specialist makes Octomind useful for an entirely new audience. A cardiologist publishing `doctor:medications`. A tax attorney publishing `lawyer:us`. A security researcher publishing `security:owasp`. One TOML file — and everyone with that problem gets a specialist-grade AI instantly.

- [How to write a tap agent](https://github.com/muvon/octomind-tap)
- [Open issues](https://github.com/muvon/octomind/issues)
- [Building from source](doc/dev/01-building-from-source.md)

---

## Documentation

- [Installation & Setup](doc/usage/01-installation.md)
- [Quickstart](doc/usage/02-quickstart.md)
- [Configuration](doc/usage/03-configuration.md)
- [Providers & Models](doc/usage/04-providers.md)
- [Sessions & Compression](doc/usage/05-sessions.md)
- [Roles](doc/usage/06-roles.md)
- [MCP Tools](doc/usage/07-mcp-tools.md)
- [Workflows](doc/usage/09-workflows.md)
- [Guardrails](doc/usage/18-guardrails.md)
- [Learning](doc/usage/13-learning.md)
- [WebSocket & ACP](doc/integration/01-websocket-server.md)
- [CLI Reference](doc/reference/01-cli-reference.md)
- [Config Reference](doc/reference/03-config-reference.md)

Full index: [doc/README.md](doc/README.md).

---

## License

Apache License 2.0 — see [LICENSE](LICENSE).

---

**Octomind** by [Muvon](https://muvon.io) | [Website](https://octomind.run) | [Documentation](https://octomind.run/docs/)
