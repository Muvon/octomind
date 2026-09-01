# Quickstart

Start an interactive Octomind session through OctoHub, then learn the commands you need day-to-day.

## Start in Three Commands

```bash
# Install
curl -fsSL https://octomind.run/install.sh | bash

# Authorize the CLI in your browser; no provider API keys are needed
octomind login

# Start an interactive session in the current directory
octomind
```

The first command installs the binary. Login stores an OctoHub gateway credential in Octomind's user configuration directory. The final command is equivalent to `octomind run`: it creates the default configuration if none exists, resolves the default `assistant:concierge` tap agent, and uses the shipped `octohub:auto` model profile.

The first use of a tap agent fetches its manifest and dependencies, so it requires network access. Once the prompt appears, ask Octomind to inspect, explain, change, or verify the project in the directory where you started it.

## Bring Your Own Key Instead

Login is optional. Export a provider credential and select that provider's model explicitly:

```bash
export OPENROUTER_API_KEY="your_key"
octomind run -m 'openrouter:<model>'
```

Replace `<model>` with a model identifier accepted by the provider. See [AI Providers](04-providers.md#bring-your-own-keys) for the full provider and environment-variable table.

## Try a First Task

Enter a request at the session prompt:

```text
Explain how this project is structured and identify the best starting point for a new contributor.
```

Octomind can use the tools allowed by the active role. The shipped default agent can inspect the project, execute commands, edit files, and delegate work.

## Essential Session Commands

| Command | Purpose |
|---------|---------|
| `/help` | Show the commands available to the active role |
| `/info` | Show session, token, and cost details |
| `/status [agents\|monitors\|jobs]` | Show background activity |
| `/model <provider:model>` | Change the session model |
| `/image <path>` | Attach an image to the next message |
| `/done` | Finalize the current task with memorization and summarization |
| `/clear` | Clear the terminal |
| `/copy` | Copy the last response |
| `/exit` | Exit the session; `Ctrl+D` also exits interactive input |

Use `/help` as the authority for the commands exposed in the current session.

## Choose a Role or Tap Agent

The optional positional argument to `octomind run` is a tag:

- A plain name such as `assistant` selects a local `[[roles]]` entry.
- A `category:variant` tag such as `developer:general` resolves an agent from the configured taps.

```bash
# Configured default tag
octomind

# Explicit local role
octomind run assistant

# Registry agent
octomind run developer:general
```

See [Roles](06-roles.md) for role configuration and the [Tap System](../integration/04-tap-system.md) for registry resolution.

## Name and Resume Sessions

```bash
# Create a named session, or resume it when it already exists
octomind run --name my-feature

# Resume a named session
octomind run --resume my-feature

# Open the interactive recent-session picker
octomind run --resume

# Resume the most recent session for this working directory
octomind run --resume-recent
```

## Run Non-Interactively

`--format` switches `octomind run` to stdin-driven operation. The accepted formats are `plain` and `jsonl`:

```bash
echo "Explain the authentication module" | \
  octomind run developer:general --format plain

echo "List TODO items" | \
  octomind run developer:general --format jsonl
```

## Next Steps

- [Configuration](03-configuration.md) — customize models, roles, tools, and limits
- [AI Providers](04-providers.md) — choose OctoHub or configure provider credentials
- [Sessions](05-sessions.md) — manage interactive and persistent sessions
- [MCP Tools](07-mcp-tools.md) — understand the tool surface
