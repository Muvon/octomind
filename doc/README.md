# Octomind Documentation

Use these guides to install, configure, and operate Octomind's CLI agent runtime, or to contribute to its Rust code.
Start with a terminal session, then follow the guides for automation, integrations, and development.

## Start Here

Install the binary and sign in to an Octomind account to use the shipped OctoHub model default:

```bash
curl -fsSL https://octomind.run/install.sh | bash
octomind login
octomind
```

`octomind login` asks you to confirm a code in the browser and CLI, then stores the hub key in the user-scope
`config/.env` and account credentials in `config/auth.json`. It does not change your model selection. New configs use
`assistant:concierge` as the default tag and `octohub:auto` as the model name. See
[Installation](usage/01-installation.md) and [Quickstart](usage/02-quickstart.md) for the complete setup.

## Configure

Inspect and validate your configuration before adding roles or tools:

```bash
octomind config --show
octomind config --validate
```

The default config is under `~/.local/share/octomind/config/` on Linux/macOS and `%LOCALAPPDATA%/octomind/config/` on
Windows. `OCTOMIND_DATA_DIR` relocates Octomind state; for example:

```bash
OCTOMIND_DATA_DIR="$PWD/.octomind-data" octomind config --show
```

Use [Configuration](usage/03-configuration.md) for file merging and overrides, or [AI
Providers](usage/04-providers.md#bring-your-own-keys) to supply provider credentials yourself.

## Operate

Create or resume a named interactive session:

```bash
octomind run --name project-review
# After exiting, continue the same session:
octomind run --resume project-review
```

Inside a session, discover commands and inspect active tools:

```text
/help
/mcp
/exit
```

For automation, pipe the prompt through stdin and choose `plain` or `jsonl` output:

```bash
printf '%s\n' 'Summarize the responsibilities of an MCP server in three sentences.' | octomind run --format plain
```

`run` takes a role/tag positional argument, not a message argument. See [Daemon &
Hooks](integration/03-daemon-and-hooks.md) for persistent workers and [Workflows](usage/09-workflows.md) for multi-step
runs.

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

## Development Guide

| Document | Description |
|----------|-------------|
| [Building from Source](dev/01-building-from-source.md) | Rust setup and development builds |
| [Architecture](dev/02-architecture.md) | Source modules and internal flows |
| [MCP Server Development](dev/03-mcp-server-development.md) | Building MCP servers for Octomind |
| [Learning Benchmark](dev/05-learning-benchmark.md) | Retrieval and consolidation contract benchmark |

## Common Questions

If the shell cannot find the binary after the default installation, add the installer directory to your current shell's
path:

```bash
export PATH="$HOME/.local/bin:$PATH"
octomind --version
```

If login cannot open a local browser, print the URL instead:

```bash
octomind login --no-browser
```

If a piped command hangs, ensure its producer closes stdin; Octomind reads the complete prompt before starting. For
configuration and credential errors, start with `octomind config --validate` above and the guides below.

## Troubleshooting and Reference

| Document | Description |
|----------|-------------|
| [Common Issues](troubleshooting/01-common-issues.md) | Installation, configuration, provider, and session problems |
| [Migration Guide](troubleshooting/02-migration-guide.md) | Upgrading legacy configurations |
| [CLI Reference](reference/01-cli-reference.md) | CLI subcommands and flags |
| [Session Commands](reference/02-session-commands.md) | Interactive slash commands |
| [Config Reference](reference/03-config-reference.md) | Configuration fields and defaults |
| [Environment Variables](reference/04-environment-variables.md) | Credentials, overrides, and runtime variables |

## See also

- [Architecture](dev/02-architecture.md)
- [CLI Reference](reference/01-cli-reference.md)
- [Common Issues](troubleshooting/01-common-issues.md)
- [GitHub Repository](https://github.com/muvon/octomind)
- [Issues](https://github.com/muvon/octomind/issues)
- [Discussions](https://github.com/muvon/octomind/discussions)
- [Provider Library](https://github.com/muvon/octolib)
- [OctoHub Gateway](https://github.com/Muvon/octohub)
