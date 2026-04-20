# Octomind Documentation

**Octomind** is a session-first AI development assistant with built-in MCP tools and multi-provider support.

## Getting Started

1. [Installation](usage/01-installation.md) -- install and set up API keys
2. [Quickstart](usage/02-quickstart.md) -- first session in 5 minutes
3. [Configuration](usage/03-configuration.md) -- customize settings

## Usage Guide

For users working with Octomind day-to-day.

| Document | Description |
|----------|-------------|
| [Installation](usage/01-installation.md) | Install methods, API keys, shell completions |
| [Quickstart](usage/02-quickstart.md) | Zero to productive session in 5 minutes |
| [Configuration](usage/03-configuration.md) | Config files, settings, hierarchy |
| [Providers](usage/04-providers.md) | AI providers, models, caching, costs |
| [Sessions](usage/05-sessions.md) | Interactive sessions, commands, multimodal |
| [Roles](usage/06-roles.md) | Roles, permissions, tool access |
| [MCP Tools](usage/07-mcp-tools.md) | All built-in tools reference |
| [Compression](usage/08-compression.md) | Automatic context compression |
| [Workflows](usage/09-workflows.md) | Multi-step AI processing workflows |
| [Pipelines](usage/14-pipelines.md) | Deterministic script-driven pre-processing |
| [Commands & Layers](usage/10-commands-and-layers.md) | Custom commands, layers, agents, prompts |
| [Structured Output](usage/11-structured-output.md) | JSON Schema output for automation |
| [Editor Integration](usage/12-editor-integration.md) | Neovim, Zed, JetBrains setup |
| [Learning](usage/13-learning.md) | Cross-session adaptive learning |
| [Skills](usage/15-skills.md) | Auto-expanding skills, capabilities, validators |

## Integration Guide

For building on top of Octomind.

| Document | Description |
|----------|-------------|
| [WebSocket Server](integration/01-websocket-server.md) | Remote AI sessions via WebSocket |
| [ACP Protocol](integration/02-acp-protocol.md) | Agent Client Protocol for tool integration |
| [Daemon & Hooks](integration/03-daemon-and-hooks.md) | Background sessions, webhook listeners |
| [Tap System](integration/04-tap-system.md) | Registry for agents, skills, capabilities |

## Development Guide

For contributors to Octomind itself.

| Document | Description |
|----------|-------------|
| [Building from Source](dev/01-building-from-source.md) | Rust setup, build, pre-commit hooks |
| [Architecture](dev/02-architecture.md) | Source modules, internals, patterns |
| [MCP Server Development](dev/03-mcp-server-development.md) | Building new MCP servers |

## Use Cases

Real-world examples showing how to solve practical problems with Octomind.

| Document | Description |
|----------|-------------|
| [CI/CD Code Review](use-cases/01-ci-cd-code-review.md) | Automated code review in pipelines with structured output |
| [Event-Driven Agent](use-cases/02-event-driven-agent.md) | Daemon + webhooks for GitHub, Slack, monitoring |
| [Custom Workflow](use-cases/03-custom-development-workflow.md) | Multi-stage refine-research-validate pipeline |
| [Web Dashboard](use-cases/04-web-dashboard-integration.md) | Embed AI assistant via WebSocket server |
| [Multi-Agent Delegation](use-cases/05-multi-agent-delegation.md) | Parallel specialized agents for complex tasks |
| [Dynamic MCP Servers](use-cases/06-dynamic-mcp-servers.md) | AI self-configuring tools at runtime |
| [Scheduled Tasks](use-cases/07-scheduled-tasks.md) | Timed automation, reminders, periodic checks |
| [Long-Running Development](use-cases/08-long-running-development.md) | Multi-day tasks with session resume |
| [Custom Hooks](use-cases/09-custom-hooks.md) | Write hooks in any language for any integration |
| [Deterministic Pipelines](use-cases/10-deterministic-pipelines.md) | Script-driven context preparation before AI |

## Troubleshooting

| Document | Description |
|----------|-------------|
| [Common Issues](troubleshooting/01-common-issues.md) | Installation, config, MCP, session problems |
| [Migration Guide](troubleshooting/02-migration-guide.md) | Upgrading from legacy config formats |

## Reference

| Document | Description |
|----------|-------------|
| [CLI Reference](reference/01-cli-reference.md) | All CLI commands and flags |
| [Session Commands](reference/02-session-commands.md) | All 23 interactive session commands |
| [Config Reference](reference/03-config-reference.md) | Every configuration field documented |
| [Environment Variables](reference/04-environment-variables.md) | API keys, overrides, template variables |

## Links

- [GitHub Repository](https://github.com/muvon/octomind)
- [Issues](https://github.com/muvon/octomind/issues)
- [Discussions](https://github.com/muvon/octomind/discussions)
- [Provider Library (octolib)](https://github.com/muvon/octolib)

---

**Octomind** -- Session-first AI development assistant

**&copy; 2026 Muvon Un Limited** | Apache License 2.0
