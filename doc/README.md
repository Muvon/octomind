# Octomind Documentation

Octomind is an open-source, CLI-first AI agent runtime: one binary for coding sessions, scheduled automation, and multi-agent workflows with any model provider.

MCP-native, with persistent sessions, configurable roles, embedded and event-driven agents, and an out-of-band supervisor. Built for autonomous work, not just chat.

## Start Here

The recommended path needs no third-party provider account or manually managed API key:

```bash
curl -fsSL https://octomind.run/install.sh | bash
octomind login
octomind
```

`octomind login` authorizes the CLI in your browser and configures the default OctoHub gateway. If you prefer to supply provider credentials yourself, see [AI Providers](usage/04-providers.md#bring-your-own-keys).

1. [Installation](usage/01-installation.md) — install Octomind and choose an authentication path
2. [Quickstart](usage/02-quickstart.md) — start a session and learn the essential commands
3. [Configuration](usage/03-configuration.md) — understand files, model profiles, roles, and overrides
4. [AI Providers](usage/04-providers.md) — use OctoHub or bring your own credentials

## Usage Guide

| Document | Description |
|----------|-------------|
| [Installation](usage/01-installation.md) | Recommended setup, alternative installs, and shell completions |
| [Quickstart](usage/02-quickstart.md) | First session, common commands, and non-interactive use |
| [Configuration](usage/03-configuration.md) | Config locations, merging, models, roles, and MCP servers |
| [Providers](usage/04-providers.md) | OctoHub, provider credentials, model selection, and local CLI backends |
| [Sessions](usage/05-sessions.md) | Interactive sessions, persistence, and multimodal input |
| [Roles](usage/06-roles.md) | Roles, prompts, permissions, and tool access |
| [MCP Tools](usage/07-mcp-tools.md) | Built-in tools and runtime tool management |
| [Compression](usage/08-compression.md) | Automatic context compression |
| [Workflows](usage/09-workflows.md) | Multi-step AI processing workflows |
| [Commands & Layers](usage/10-commands-and-layers.md) | Custom commands, layers, agents, and prompts |
| [Structured Output](usage/11-structured-output.md) | JSON Schema output for automation |
| [Editor Integration](usage/12-editor-integration.md) | Neovim, Zed, and JetBrains setup |
| [Learning](usage/13-learning.md) | Cross-session adaptive learning |
| [Supervisor](usage/14-supervisor.md) | Completion checks, planning, condensation, and learning control |
| [Skills](usage/15-skills.md) | Auto-activating skills and validators |
| [Token Efficiency](usage/16-token-efficiency.md) | Context and capability efficiency |
| [Local Tools](usage/17-local-tools.md) | Project-local scripts exposed as MCP tools |
| [Guardrails](usage/18-guardrails.md) | Deterministic project policies and hooks |

## Integration Guide

| Document | Description |
|----------|-------------|
| [WebSocket Server](integration/01-websocket-server.md) | Remote sessions over WebSocket |
| [ACP Protocol](integration/02-acp-protocol.md) | Agent Client Protocol integration |
| [Daemon & Hooks](integration/03-daemon-and-hooks.md) | Long-running sessions and webhook listeners |
| [Tap System](integration/04-tap-system.md) | Agent, skill, capability, and workflow registries |

## Development Guide

| Document | Description |
|----------|-------------|
| [Building from Source](dev/01-building-from-source.md) | Rust setup and development builds |
| [Architecture](dev/02-architecture.md) | Source modules and internal flows |
| [MCP Server Development](dev/03-mcp-server-development.md) | Building MCP servers for Octomind |

## Use Cases

| Document | Description |
|----------|-------------|
| [CI/CD Code Review](use-cases/01-ci-cd-code-review.md) | Automated review with structured output |
| [Event-Driven Agent](use-cases/02-event-driven-agent.md) | Daemon sessions driven by webhooks |
| [Custom Workflow](use-cases/03-custom-development-workflow.md) | Multi-stage development workflows |
| [Web Dashboard](use-cases/04-web-dashboard-integration.md) | Embedding sessions through WebSocket |
| [Multi-Agent Delegation](use-cases/05-multi-agent-delegation.md) | Delegating work to specialized agents |
| [Dynamic MCP Servers](use-cases/06-dynamic-mcp-servers.md) | Runtime tool-server configuration |
| [Scheduled Tasks](use-cases/07-scheduled-tasks.md) | Timed messages and recurring work |
| [Long-Running Development](use-cases/08-long-running-development.md) | Named sessions and resume workflows |
| [Custom Hooks](use-cases/09-custom-hooks.md) | Script-backed webhook integration |

## Troubleshooting and Reference

| Document | Description |
|----------|-------------|
| [Common Issues](troubleshooting/01-common-issues.md) | Installation, configuration, provider, and session problems |
| [Migration Guide](troubleshooting/02-migration-guide.md) | Upgrading legacy configurations |
| [CLI Reference](reference/01-cli-reference.md) | CLI subcommands and flags |
| [Session Commands](reference/02-session-commands.md) | Interactive slash commands |
| [Config Reference](reference/03-config-reference.md) | Configuration fields and defaults |
| [Environment Variables](reference/04-environment-variables.md) | Credentials, overrides, and runtime variables |

## Project Links

- [GitHub Repository](https://github.com/muvon/octomind)
- [Issues](https://github.com/muvon/octomind/issues)
- [Discussions](https://github.com/muvon/octomind/discussions)
- [Provider Library](https://github.com/muvon/octolib)
- [OctoHub Gateway](https://github.com/Muvon/octohub)
