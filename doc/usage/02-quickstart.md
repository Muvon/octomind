# Quickstart: Getting Started with the Octomind CLI Agent

Install Octomind, choose where its models come from, and start an interactive session. Then learn the commands you need
day-to-day.

## Start in Two Commands

```bash
# Install
curl -fsSL https://octomind.run/install.sh | bash

# Start an interactive session in the current directory
octomind
```

The first command installs the binary. The second is equivalent to `octomind run`. On the first interactive launch,
before any config exists, it asks where to get models:

| Choice | What happens |
|--------|--------------|
| Octomind hub (recommended) | Runs the browser sign-in from `octomind login` and keeps `octohub:auto` for all three models. |
| A provider from the list | Asks for any missing key with hidden input, saves it to the user `.env`, and lets you pick models. |
| Skip for now | Writes the default config unchanged (`octohub:auto`). |

Providers whose key is already in your environment are listed first. After you pick the main model, setup asks whether
the supervisor and compression should use it too; answer `n` to choose a cheaper model for each. Esc on the first
screen works like Skip.

Startup then resolves the default `assistant:concierge` tap agent. Run `octomind configure` to change providers or models
later. Non-interactive runs (piped input, `--format`, `--daemon`) create the default config without prompting.

The first use of a tap agent fetches its manifest and dependencies, so it requires network access. Once the prompt
appears, ask Octomind to inspect, explain, change, or verify the project in the directory where you started it.

## Bring Your Own Key

The hub is optional. Run `octomind configure`, pick your provider, enter the key, and pick the models. See
[AI Providers](04-providers.md#bring-your-own-keys) to set the key and models by hand.

## Try a First Task

Enter a request at the session prompt:

```text
Explain how this project is structured and identify the best starting point for a new contributor.
```

Octomind can use the tools enabled for the active role. Inspect the current tools before requesting file edits, shell
commands, or delegation; a tap's fetched manifest determines which capabilities are available:

```text
/mcp
```

## Essential Session Commands

| Command | Purpose |
|---------|---------|
| `/help` | Show the commands available to the active role |
| `/info` | Show session, token, and cost details |
| `/status [agents\|monitors\|jobs]` | Show background activity |
| `/model <provider:model>` | Change the session model |
| `/image <path>` | Attach an image to the next message |
| `/done` | Force context compression and start lesson extraction when learning is enabled |
| `/clear` | Clear the terminal |
| `/copy [scope]` | Copy the last response, or `assistant`/`user`/`all` |
| `/reply [N]` | Quote an earlier reply into the prompt to answer it point by point; `>` at a line start picks single lines |
| `/exit` | Exit the session; `Ctrl+D` also exits interactive input |

For example, inspect a session, finish a task, and then exit:

```text
/help
/info
/status
/done
/exit
```

See [Sessions](05-sessions.md) for model switching and image attachment examples.

## Choose a Role or Tap Agent

The optional positional argument to `octomind run` is a tag:

- A plain name such as `assistant` selects a local `[[roles]]` entry.
- A `category:variant` tag such as `developer:general` resolves an agent from the configured taps.
- A domain name such as `developer` starts that domain's orchestrator (`developer:chief`) from the configured taps, unless a local role has the same name.

```bash
# Configured default tag
octomind

# Registry agent
octomind run developer:general
```

Plain names require a configured local role. See [Roles](06-roles.md) for a complete local-role example and the [Tap
System](../integration/04-tap-system.md) for registry resolution.

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

`--format` switches `octomind run` to stdin-driven operation. Use `plain` for text or `jsonl` for events. The positional
argument is the agent tag, so pipe the prompt through stdin:

```bash
echo "Explain the authentication module" | \
  octomind run developer:general --format plain

echo "List TODO items" | \
  octomind run developer:general --format jsonl
```

Piped stdin also selects non-interactive operation without `--format`, using plain output. Empty stdin is an error.

## Common Questions

**Why does startup need network access?** Tap resolution fetches agent definitions and may run dependency setup; model
requests also need access to the selected provider. For a failed login on a headless machine:

```bash
octomind login --no-browser
```

**Why does a command say no input was provided?** A non-interactive run needs a nonempty stdin prompt:

```bash
printf '%s\n' 'Summarize this project.' | octomind run --format plain
```

## See also

- [Configuration](03-configuration.md) — customize models, roles, tools, and limits
- [AI Providers](04-providers.md) — choose OctoHub or configure provider credentials
- [Sessions](05-sessions.md) — manage interactive and persistent sessions
- [MCP Tools](07-mcp-tools.md) — understand the tool surface
