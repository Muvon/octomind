<div align="center">
  <a href="https://octomind.run" target="_blank">
    <img src="assets/logo.svg" width="640" alt="Octomind — AI Coding Agent Runtime" />
  </a>
  <br /><br />
  <strong>The CLI-first AI coding agent runtime.</strong><br />
  <em>Pipe it, schedule it, embed it. One binary, multiple model providers, MCP-native — built for autonomous work, not just chat.</em>
  <br /><br />

  [![License](https://img.shields.io/badge/license-Apache%202.0-7c3aed?style=flat-square)](LICENSE)
  [![Version](https://img.shields.io/crates/v/octomind?style=flat-square&color=7c3aed)](https://crates.io/crates/octomind)
  [![Coverage](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fmuvon%2Foctomind%2Fbadges%2Fcoverage.json&style=flat-square)](https://github.com/muvon/octomind/actions/workflows/ci.yml)
  [![GitHub stars](https://img.shields.io/github/stars/muvon/octomind?style=flat-square&color=7c3aed)](https://github.com/muvon/octomind/stargazers)
  [![Website](https://img.shields.io/badge/website-octomind.run-7c3aed?style=flat-square)](https://octomind.run)

  <br />

  [Documentation](https://octomind.run/docs/) · [Tap Registry](https://github.com/muvon/octomind-tap) · [Website](https://octomind.run)
</div>

---

Octomind is an open-source AI agent client: the model calls MCP tools to do real work — read and write files, run shells, search code, delegate to sub-agents. The same runtime supports several entry points: the same session runs **interactively**, **piped through stdin**, as a **background daemon**, over **WebSocket**, or as an **ACP sub-agent** inside another agent's stack. Models, tools, roles, guardrails, budgets — all of it is TOML, no framework code.

```bash
# Interactive
octomind run developer:general

# Piped — CI, scripts, automation
echo "Explain the auth module" | octomind run developer:general --format plain

# Daemon — long-running; send from another terminal on the same machine
echo "watch the build" | octomind run --name watcher --daemon --format jsonl
octomind send --name watcher "run the test suite"
```

## Table of Contents

- [Quick Start](#quick-start)
- [Benchmarks — Real PRs, Held-Out Tests](#benchmarks--real-prs-held-out-tests)
- [Why Octomind?](#why-octomind)
- [One Binary, Five Surfaces](#one-binary-five-surfaces)
- [Guardrails — Policy as Code](#guardrails--policy-as-code)
- [Cost as a Control Plane](#cost-as-a-control-plane)
- [Sessions That Stay Sharp at Hour 4](#sessions-that-stay-sharp-at-hour-4)
- [Intent-Driven Context](#intent-driven-context)
- [Specialists & Taps](#specialists--taps)
- [Built-in MCP Tools](#built-in-mcp-tools)
- [Power Users — Roles, Workflows, Layers](#power-users--roles-workflows-layers)
- [Installation](#installation)
- [Configuration](#configuration)
- [Architecture](#architecture)
- [Contributing](#contributing)
- [Documentation](#documentation)
- [License](#license)

---

## Quick Start

```bash
# Install (macOS & Linux) — single Rust binary; taps install their own tool dependencies
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash

# Sign in — models included, no API keys to manage
octomind login

# Start with a specialist — first use may install tools and request credentials
octomind run developer:general
```

```text
        Octomind v0.50.1
        Role: developer:general · Model: octohub:auto
        ~/your/project
> _
```

You're in a session with an agent that can read your code, run commands, edit files, and grow capabilities as needed. Plain-line interface with markdown rendering and [shell completions](doc/reference/01-cli-reference.md) — no TUI to learn, works over SSH, in `tmux`, in CI logs.

`octomind login` connects you to [Octomind Cloud](https://octomind.run/cloud) — a subscription that includes model access through the octohub gateway, so there's nothing to configure. Prefer your own keys? Skip login entirely and [bring any provider](#model-access): OpenRouter, Anthropic, OpenAI, DeepSeek, Ollama, and more. Cloud is the baseline; BYOK is always a first-class path.

> `developer:general` (and `lawyer:sg`, `doctor:blood`, …) come from the built-in default tap [`muvon/tap`](https://github.com/muvon/octomind-tap), not your local config. The config's own default tag is `assistant:concierge`, so plain `octomind run` starts that. The banner above is illustrative (the real one renders a pixel icon to the left of the text block).

Other installs: `cargo install octomind` (Rust 1.95+) or [build from source](#installation).

---

## Benchmarks — Real PRs, Held-Out Tests

We benchmark on [octobench](https://github.com/Muvon/octobench): **25 tasks harvested from merged pull requests** across python, php, rust, c++, and js. Each agent works in the pre-fix repo; held-out tests from the merged fix decide pass or fail. These are the published July 30, 2026 results, not a measurement of the current checkout:

| | solved | judge Σ / 2500 | cost | wall time |
|---|---|---|---|---|
| **octomind + glm-5.2** | **24/25** | **2264** | $63.43 | 3.6h |
| claude code + claude-opus-5 | 23/25 | 2262 | $81.79 | 6.7h |
| codex + gpt-5.6-sol | 21/25 | 2127 | $14.86 | 1.0h |
| opencode + glm-5.2 | 19/25 | 2093 | $129.54 | 3.3h |

- **The harness matters.** opencode used the same model and endpoint. octomind solved 24 vs 19 at roughly half the cost. Octomind used a staged tap and a binary override with the unfinished-handback pre-gate; this measures the complete setup, not individual features.
- **Worst-case pricing, still ahead.** glm-5.2 ran without prompt caching (every token at list price) while Opus billed ~97% of context re-reads at 1/10 cache rates — and octomind still led on solves, cost, and wall time.
- **Reproducible.** Full per-case table, run artifacts, and reproduction guide: [BENCHMARK.md @ 8aa3968](https://github.com/Muvon/octobench/blob/8aa39684ff6103782aacb1bd79ea98e96e50d6cf/BENCHMARK.md). The story behind the benchmark: [blog post](https://octomind.run/blog/coding-agent-benchmark-real-prs).

---

## Why Octomind?

The runtime gives you controls for the work that continues after you leave the keyboard:

- **Autonomy needs policy.** Enforce pre-call rules and feed script failures back to the agent with
  [Guardrails](doc/usage/18-guardrails.md).
- **Share a working setup.** A [tap](doc/integration/04-tap-system.md) packages agent instructions, dependencies,
  and tool access. Start a specialist with `octomind run developer:general`.
- **Choose a specialist.** Use different [roles](doc/usage/06-roles.md) for debugging, research, or review, with
  their own instructions, model overrides, and tools.
- **Keep long tasks moving.** [Compression](doc/usage/08-compression.md) reduces accumulated context while
  retaining task knowledge and the live exchange.
- **Track spending.** Configure request and session thresholds and inspect costs with `/info`.
  See [Cost](#cost-as-a-control-plane).
- **Load context on demand.** [Skills](doc/usage/15-skills.md) and
  [capabilities](doc/integration/04-tap-system.md#capabilities) can activate from your input.

| Pillar | What it gives you |
|---|---|
| **Zero config, full flexibility** | `octomind run lawyer:sg` works out of the box. Need a different model, MCP server, or guardrail pipe? Same TOML, no framework code. |
| **Sessions stay sharp at hour 4** | Adaptive compaction: cache-aware, structurally preserving. Smaller context = faster responses + lower cost. |
| **Cost as a control plane** | Per-step model selection across many providers. Spending thresholds and cache-aware accounting come for free. |
| **Guardrails: policy as code** | Govern autonomous agents with deterministic scripts — pre-call guards, post-result hooks, post-turn validators. No modal approval clicks. Fits CI. |
| **Intent-driven context** | Skills and capabilities can activate through rules, semantic matching, or explicit requests. Smaller context by default, lower cost, no surprise tools. |

---

## One Binary, Five Surfaces

The same session engine, exposed however your workflow needs it:

| Mode | Use for |
|---|---|
| [Interactive CLI](doc/usage/05-sessions.md) | Daily work, any domain |
| [`octomind run --format plain`](doc/reference/01-cli-reference.md) pipe | CI/CD pipelines, shell scripts, automation |
| [Daemon + send](doc/integration/03-daemon-and-hooks.md) | Background agents, continuous monitoring, long-running tasks |
| [WebSocket server](doc/integration/01-websocket-server.md) (`octomind server`) | IDE plugins, web dashboards, external integrations |
| [ACP protocol](doc/integration/02-acp-protocol.md) (`octomind acp`) | Multi-agent orchestration, being called by other agents |

```bash
# ACP — drop into any multi-agent system as a sub-agent
octomind acp developer:general

# Non-interactive — the message is read from stdin (pipe it in), output as plain text
echo "Explain the auth module" | octomind run developer:general --format plain

# Structured JSONL output for pipelines
echo "List TODO items" | octomind run developer:general --format jsonl

# Daemon — keep alive; run send in another terminal on the same machine
echo "first task" | octomind run --name watcher --daemon --format jsonl
octomind send --name watcher "now run the test suite"

# Structured output — create the schema first (requires a supporting model)
cat > todos.schema.json <<'JSON'
{
  "type": "object",
  "properties": {"items": {"type": "array", "items": {"type": "string"}}},
  "required": ["items"],
  "additionalProperties": false
}
JSON
echo "List TODO items as JSON" | octomind run developer:general --format jsonl --schema todos.schema.json
```

`octomind run` has **no message argument**: its positional argument is a role or tap tag. Piped stdin runs
non-interactively, defaulting to `plain`; `--format plain` or `--format jsonl` selects the output format.
At a terminal, `--format` without piped input errors unless you also use `--daemon`. Without `--format`,
a terminal starts an interactive session. `server` and `acp` do not take `--format`.

`--daemon` keeps the process alive; it does not detach it from your terminal. Use another terminal for `octomind send`.
For schema requirements and model support, see [Structured Output](doc/usage/11-structured-output.md).

See [WebSocket Server](doc/integration/01-websocket-server.md), [ACP Protocol](doc/integration/02-acp-protocol.md), and [Daemon & Hooks](doc/integration/03-daemon-and-hooks.md) for the integration modes.

One binary. Every workflow.

---

## Guardrails — Policy as Code

A long-running task, CI job, or autonomous loop needs repeatable rules for tool execution and validation.

**Policy lives in TOML rules and scripts.** Drop a `.agents/guardrails.toml` in your repo and the runtime enforces it deterministically — pre-call, post-result, post-turn.

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
```

- **Guards** — pre-call deny rules. Match by `capability(arg_name=regex)`, gate by history (`+used` / `-unused`), require loaded capabilities (`has = [...]`). A matching call returns a denial instead of executing.
- **Hooks** — post-result scripts. Run after matching tool results. Non-zero exit injects stdout into the agent's inbox as a user message — clippy errors, lint failures, format diffs become *automatic corrections without restarting the turn*.
- **Validators** — post-turn scripts. Their `when` history filters inspect calls since the previous run; without `when`, they can run every turn. Filter by role and response text. Output is wrapped in `<validation>` blocks the agent reads on its next turn. **This is what replaces "approve this change?" prompts in autonomous loops.**

The DSL combines capability+arg-regex+history+role+result-regex in one declarative file. No code to compile, no plugin to install. **Designed for full automation: fits CI, daemons, scheduled runs, ACP sub-agents.** Complete hook and validator script examples: [Guardrails](doc/usage/18-guardrails.md).

> The world is going autonomous. The choice isn't "ask vs auto" — it's "auto with deterministic policy" vs "auto with hope." Octomind ships the former.

---

## Cost as a Control Plane

Pick the right model for each step. A cheap one for routine research, a frontier one for review — per-role, per-step, mid-session swap. Real-time cost tracking and spending thresholds come for free.

```toml
# Example spending thresholds — both default to 0.0 (disabled)
max_request_spending_threshold = 0.50    # USD per user request, including its tool loop
max_session_spending_threshold = 5.00    # USD per session

# Per-role model selection — pay Opus only where it's worth it
[[roles]]
name = "researcher"
system = "Research the supplied material and explain your findings with evidence."
welcome = "Send the material you want researched."
[roles.model]
name = "openrouter:google/gemini-2.5-flash"   # cheap broad context

[[roles]]
name = "reviewer"
system = "Review the supplied changes for correctness and explain concrete defects."
welcome = "Send the changes you want reviewed."
[roles.model]
name = "anthropic:claude-opus-4-7"            # precision where it counts
```

- Per-role and per-workflow-step model selection across many providers — OpenRouter, OpenAI, Anthropic, Google, DeepSeek, Amazon Bedrock, Cloudflare, and more — via [octolib](https://github.com/muvon/octolib). Different roles can run on different vendors; available providers depend on the octolib version linked into your binary. See [Providers & Models](doc/usage/04-providers.md) for the current list and supported models.
- Mid-session model swap with `/model anthropic:claude-haiku-4-5`. Mix providers across roles — cheap model for research, best model for execution. Session totals include usage across model switches.
- Real-time cost tracking per request and per session.
- Cache-aware token accounting (`cache_read_tokens`, `cache_write_tokens` separated from input/output).
- Thresholds use already-recorded costs, so a provider call can take you past the configured amount. A **session threshold**
  prompts at an interactive terminal and stops piped or ACP/WebSocket work; accepting resets the spending checkpoint.
  A **request threshold** stops the current request. See [Configuration](doc/usage/03-configuration.md).

> Both thresholds are off by default. Set them explicitly; they are continuation checks, not prepaid billing limits.

---

## Sessions That Stay Sharp at Hour 4

Long tasks fill the context window with tool output, intermediate attempts, and decisions you still need.

Octomind's adaptive compaction engine runs automatically:

- **Cache-aware** — calculates if compaction is worth it *before* paying for it. Accounts for cache invalidation and rewrite costs.
- **Growth-aware** — adjusts the compression target using measured growth and the context ceiling.
- **Structurally preserving** — retains critical knowledge, selected analysis findings, and the live exchange.
- **Adaptively plan-aware** — the [supervisor](doc/usage/14-supervisor.md) tracks complex work externally while focused tasks remain plan-free.
- **Fully automatic** — you never think about it.

The benefit: smaller context reduces later input tokens. Compression itself consumes tokens and can invalidate the cache.

Sessions also persist: `octomind run --name my-feature` saves as you go, `octomind run --resume my-feature` (or `--resume-recent`) picks up where you left off — including multi-day tasks. Details: [Compression](doc/usage/08-compression.md), [Sessions](doc/usage/05-sessions.md).

> Use `/done` at a task boundary to force compression and start background [learning](doc/usage/13-learning.md).

---

## Intent-Driven Context

Your role determines the tools loaded at startup. Additional skills and capabilities can activate as the task develops.
Skills inject instructions and load their required capabilities; they remain active until forgotten or cleared at a task
boundary. **Context follows both your starting role and the work you ask for.**

### How activation works

- **Semantic rules.** An internal embedding model scores your request against authored `semantic(...)` phrases for skills
  and trigger phrases for capabilities. Skill descriptions alone do not trigger automatic activation.
- **Hand-authored rules where precision matters.** Skill authors can pin activation to file names, file contents, or exact phrases when they know better than a similarity score.
- **Abstain on semantic near-ties.** The top semantic candidate needs a sufficient lead. Deterministic rule matches
  still activate independently; several can match one message.
- **Calibrated to skip, not guess.** Wrong activations bloat context and waste tokens. The system defaults to silence when in doubt.

### Why this matters

```text
1. Start a session → load the role's configured tools
2. Send a task → evaluate inactive skills' rules and capability triggers
3. A skill matches → load its required capabilities and inject its instructions
4. Work continues → active skills survive automatic compression
5. Forget a skill → release its capability references and request compression
```

**Keeping unused instructions out of context leaves more room for the task.**

It compounds with the rest:

- **`mcp` mid-session.** Enabling a server connects it and exposes its tools. Skill activation also enables required
  capability servers when credentials are available; it does not wait for the first tool call.
- **Compression interplay.** A deactivated skill is dropped during compaction — its content is recoverable on next activation, not pinned forever.
- **Guardrails.** A guard can require `has = ["filesystem-read"]` and only fire when that capability is currently loaded. Policy and activation share the same capability namespace.

Details: [Skills](doc/usage/15-skills.md), [Token Efficiency](doc/usage/16-token-efficiency.md).

> Start with a focused role, then add skills and capabilities as the task needs them.

---

## Specialists & Taps

`octomind run <tag>` resolves a **specialist** — a packaged agent with its model config, system prompt, MCP servers, and tool permissions. Not a prompt file, not a skill injection — the full stack, configured by the community, ready to run.

```bash
octomind run developer:general    # general dev, language skills auto-activate
octomind run doctor:blood         # blood-test interpretation specialist
octomind run doctor:nutrition     # nutrition specialist
```

What happens when you run a specialist:

```text
→ Fetches the agent manifest from the tap registry
→ Installs required binaries automatically (skips if already present)
→ Resolves required credentials; interactive setup can prompt and persist them
→ Spins up the right MCP servers for this domain
→ Loads specialist model config, system prompt, tool permissions
→ Starts the session once setup completes
```

### Specialists grow at runtime

Roles granted the `runtime` and `orchestration` servers can acquire capabilities and delegate work mid-session:

| Tool | What it does |
|---|---|
| `tap` | Delegate work in the background to any specialist role from the tap registry. |
| `mcp` | Enable or disable MCP servers on the fly. Agent picks the server it needs and registers it mid-conversation. |
| `agent` | Register and enable dynamic agents; call the resulting `agent_<name>` tool to execute one. |

For example, these are model tool calls (not shell commands):

```text
agent({"action":"add","name":"log_reader","description":"Summarize supplied logs","system":"Summarize the log text supplied in the task."})
agent({"action":"enable","name":"log_reader"})
agent_log_reader({"task":"Summarize this deployment log: 09:00 deploy started; 09:02 health check passed."})
agent({"action":"disable","name":"log_reader"})
```

Octomind starts with the role's configured toolset and can add capabilities while it works. **Smaller context, lower cost, faster responses, no surprise tools.** See [Intent-Driven Context](#intent-driven-context) for how activation actually works.

### Add your own taps

```bash
# Scaffold and register a local tap with a known starter tag
octomind tap init yourteam/tap --agent finance:analyst
octomind run finance:analyst

# On another machine, after publishing github.com/yourteam/octomind-tap:
# octomind tap yourteam/tap

# Or register an existing local tap directory
octomind tap yourteam/internal ./octomind-tap
```

Each tap is a Git repo. Each agent is one TOML file. See [Tap System](doc/integration/04-tap-system.md) for
manifest fields, dependency setup, and publishing. Pull requests are contributions.

> Want to publish your expertise? A `doctor:medications`, a `lawyer:us`, a `devops:terraform`. One file, and everyone with that problem gets a specialist instantly. [How to write a tap agent →](https://github.com/muvon/octomind-tap)

---

## Built-in MCP Tools

Octomind is an **MCP client** for stdio and Streamable HTTP servers, with OAuth support. It also routes built-in
tools internally. See [MCP Tools](doc/usage/07-mcp-tools.md) for server configuration and tool schemas.

The runtime can expose these according to the active role and configuration. Planning is not a model-callable tool: the supervisor owns it externally, while `/plan` remains a read-only display command.

| Tool | Purpose |
|---|---|
| `mcp` | Enable/disable MCP servers at runtime |
| `agent` / `agent_<name>` | Manage dynamic agents / execute an enabled agent |
| `schedule` | Inject messages at future times |
| `monitor` | React to event-stream scripts without active polling |
| `skill` | Inject reusable instruction packs from taps |
| `tap` | Delegate to any specialist role from a tap registry |
| `capability` | Discover, enable, and disable domain tool bundles |
| `recall` | Retrieve archived context blocks when attention or governance is enabled |

### Filesystem tools (via [octofs](https://github.com/muvon/octofs))

`view`, `text_editor`, `batch_edit`, `extract_lines`, `shell`, `workdir` — file operations come from the companion
octofs MCP server. `view` also lists directories and searches content. Tool exposure depends on the tap's
capabilities; see [MCP Tools](doc/usage/07-mcp-tools.md).

### Brain (via [octobrain](https://github.com/muvon/octobrain))

`memorize`, `remember`, `forget`, `knowledge` — persistent memory and knowledge indexing. `memorize` can also link
memories through `related_to`. Taps supply octobrain through memory and knowledge capabilities.
See [MCP Tools](doc/usage/07-mcp-tools.md); the supervisor's [learning](doc/usage/13-learning.md) uses its own file store.

> **`core`, `orchestration`, `runtime`, and `agent` are the four built-in MCP servers** shipped in the default config. The `filesystem` (octofs) and `brain` (octobrain) servers are supplied by tap formulas — a freshly generated config won't list them.

### Local project tools

Drop executable scripts with a `# @description` header into `<workdir>/.agents/tools/`; they're auto-discovered as MCP
tools for that project. Include a shebang for direct execution. See [Local Tools](doc/usage/17-local-tools.md).

---

## Power Users — Roles, Workflows, Layers

For most users, taps are enough. For teams and power users, the configuration system is deep — **all TOML, no code**.

This custom role uses octofs. Install it through a filesystem capability or follow the
[MCP Tools setup](doc/usage/07-mcp-tools.md), then add the role and server to your config:

```toml
# Sandbox — OS write restrictions with state/system exceptions; see the config reference
sandbox = true

# Per-role: independent model, temperature, MCP servers, tools, system prompt
[[roles]]
name = "senior-reviewer"
system = "Read the project files and report concrete correctness defects with file references."
welcome = "Describe the change you want reviewed."
[roles.model]
name = "anthropic:claude-opus-4-7"
temperature = 0.2
[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["view"]

# Requires octofs on PATH. Edit an existing filesystem entry if you already have one.
[[mcp.servers]]
name = "filesystem"
type = "stdio"
command = "octofs"
args = ["mcp"]
timeout_seconds = 30
tools = []
```

```bash
octomind run senior-reviewer

# Workflows — multi-step, each step its own model and toolset
# After creating deep_review.toml using the Workflows guide:
echo "Review the auth module" | octomind workflow deep_review.toml
```

- **[Roles](doc/usage/06-roles.md)** — model overrides, system prompt, MCP servers, tool permissions per role.
- **[Layers](doc/usage/10-commands-and-layers.md)** — ACP subprocess stages, also used by `/run` commands;
  they do not run automatically after every response.
- **[Guardrails](doc/usage/18-guardrails.md)** — deterministic policy (guards, hooks, validators) and input pipes.
- **[Workflows](doc/usage/09-workflows.md)** — task runners with sequential, parallel, conditional, and loop steps.
- **Supervisor** — out-of-band planning, loop/no-progress detection, completion verification, tool-output condensation, and cross-session learning. See [Supervisor](doc/usage/14-supervisor.md).

See [Configuration Reference](doc/reference/03-config-reference.md) for everything.

---

## Installation

### One-line install

```bash
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash
```

Detects OS and architecture and installs to `~/.local/bin/` by default. macOS and Linux are supported.
The agent is one Rust binary; external MCP tools have their own dependencies. See [Installation](doc/usage/01-installation.md).

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

### Model access

**Option A — Octomind Cloud.** Sign in to obtain gateway model access:

```bash
octomind login
```

Device-code sign-in (like `gh auth login`). This stores a server-issued octohub gateway key locally — the default config already sets `name = "octohub:auto"` under `[model]`, so you're done. Learn more: [octomind.run/cloud](https://octomind.run/cloud).

**Option B — bring your own keys.** Octomind is fully open source and works standalone with supported providers:

```bash
# OpenRouter — access to many providers with one key
export OPENROUTER_API_KEY="your_key"

# Or any specific provider
export OPENAI_API_KEY="your_key"
export ANTHROPIC_API_KEY="your_key"
export DEEPSEEK_API_KEY="your_key"
```

Add your chosen key to `~/.bashrc` or `~/.zshrc` for persistence. To run without Octomind Cloud, edit the existing
model tables in `config.toml`; the supervisor and compression profiles also default to `octohub:auto`:

```toml
[model]
name = "openrouter:google/gemini-2.5-flash"

[supervisor.model]
name = "openrouter:google/gemini-2.5-flash"

[compression.model]
name = "openrouter:google/gemini-2.5-flash"
```

Use the key for the provider you select. See [Providers & Models](doc/usage/04-providers.md) for role and tap overrides.

### Verify

```bash
octomind --version
octomind config       # generate default config
octomind run          # start your first session
```

### Common questions

**Command not found after installing?** Add the default install directory to your shell's PATH:

```bash
export PATH="$HOME/.local/bin:$PATH"
octomind --version
```

**Missing gateway key with your own provider key set?** Update all three model tables shown above and check role
overrides. More fixes: [Common Issues](doc/troubleshooting/01-common-issues.md).

---

## Configuration

On macOS/Linux, config lives at `~/.local/share/octomind/config/config.toml`; on Windows, it lives under
`%LOCALAPPDATA%/octomind/config/`. `OCTOMIND_DATA_DIR` overrides the data root. See
[Environment Variables](doc/reference/04-environment-variables.md).

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
- **Telemetry** — anonymous usage stats, on by default

Full reference: [Configuration Reference](doc/reference/03-config-reference.md).

### Telemetry

Octomind reports anonymous usage — which commands, tools and models get used,
plus timings, token counts and error kinds. Never your code, prompts, file
paths, tool arguments or environment values. Turn it off any of three ways:

```bash
export DO_NOT_TRACK=1           # the cross-tool standard, honoured first
export OCTOMIND_TELEMETRY=0     # for subsequent commands in this shell
# or set `telemetry = false` in config.toml
```

Exact field list: [Telemetry](doc/reference/04-environment-variables.md#telemetry).

### Session commands

| Command | Description |
|---|---|
| `/help` | Show all commands |
| `/info` | Token usage and costs |
| `/status [agents\|monitors\|jobs]` | Current agents and background activity |
| `/model anthropic:claude-haiku-4-5` | Switch model mid-session |
| `/effort high` | Set reasoning effort (low/medium/high/xhigh/max) |
| `/role developer:general` | Switch to a configured role or tap tag |
| `/new Review auth` | Start a fresh session; the title is optional |
| `/done` | Force context compression and start background learning extraction when enabled |
| `/exit` | Exit session |

Full list: [Session Commands](doc/reference/02-session-commands.md).

---

## Architecture

One binary. The session is the unit of work. Around it: roles (who's talking), layers and workflows (multi-step orchestration), guardrails with pipes (deterministic pre-processing and policy), adaptive compaction (long-session quality), and MCP servers (tools). Configured through a resolved TOML configuration; internal algorithms also use fixed runtime rules.

Embedders pick their surface: interactive CLI, ACP for multi-agent orchestration, WebSocket for IDEs and dashboards,
daemon mode for long-running background agents. See [Editor Integration](doc/usage/12-editor-integration.md).

See [Architecture](doc/dev/02-architecture.md) for internals.

---

## Contributing

The most impactful contribution isn't code — **it's specialist agents.**

Every domain expert who publishes a specialist makes Octomind useful for an entirely new audience. A cardiologist publishing `doctor:medications`. A tax attorney publishing `lawyer:us`. A security researcher publishing `security:owasp`. One TOML file — and everyone with that problem gets a specialist-grade AI instantly.

- [How to write a tap agent](https://github.com/muvon/octomind-tap)
- [Open issues](https://github.com/muvon/octomind/issues)
- [Building from source](doc/dev/01-building-from-source.md)
- [Contributing guide](CONTRIBUTING.md)

---

## Documentation

- [Installation & Setup](doc/usage/01-installation.md)
- [Quickstart](doc/usage/02-quickstart.md)
- [Configuration](doc/usage/03-configuration.md)
- [Providers & Models](doc/usage/04-providers.md)
- [Sessions](doc/usage/05-sessions.md)
- [Compression](doc/usage/08-compression.md)
- [Roles](doc/usage/06-roles.md)
- [MCP Tools](doc/usage/07-mcp-tools.md)
- [Workflows](doc/usage/09-workflows.md)
- [Commands & Layers](doc/usage/10-commands-and-layers.md)
- [Structured Output](doc/usage/11-structured-output.md)
- [Editor Integration](doc/usage/12-editor-integration.md)
- [Token Efficiency](doc/usage/16-token-efficiency.md)
- [Local Tools](doc/usage/17-local-tools.md)
- [Guardrails](doc/usage/18-guardrails.md)
- [Skills](doc/usage/15-skills.md)
- [Supervisor](doc/usage/14-supervisor.md)
- [Learning](doc/usage/13-learning.md)
- [WebSocket Server](doc/integration/01-websocket-server.md)
- [ACP Protocol](doc/integration/02-acp-protocol.md)
- [Daemon & Hooks](doc/integration/03-daemon-and-hooks.md)
- [Tap System](doc/integration/04-tap-system.md)
- [CLI Reference](doc/reference/01-cli-reference.md)
- [Config Reference](doc/reference/03-config-reference.md)

Links above target the docs in this checkout. The [hosted docs site](https://octomind.run/docs/) is also available. Full index: [doc/README.md](doc/README.md).

---

## License

Apache License 2.0 — see [LICENSE](LICENSE).

---

**Octomind** by [Muvon](https://muvon.io) | [Website](https://octomind.run) | [Documentation](https://octomind.run/docs/)
