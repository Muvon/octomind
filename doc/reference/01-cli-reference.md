# CLI Reference

Use this reference to launch sessions, configure Octomind, and automate workflows from your shell. It covers every CLI
argument, including hidden internal commands.

## Synopsis

```bash
octomind [COMMAND]
octomind COMMAND --help
```

Options belong after their subcommand; there are no inherited application options. Boolean switches default to `false`,
optional values to unset, and repeatable options to an empty list unless stated otherwise.

The subcommand is optional. Bare `octomind` behaves as `octomind run` with the configured default tag.

| Command | Purpose |
|---------|---------|
| `run` | Start an interactive or non-interactive AI session (the main command). |
| `login` | Sign in to an Octomind account and store the minted OctoHub key. |
| `server` | Start a WebSocket server for remote sessions. See [WebSocket Server](../integration/01-websocket-server.md). |
| `acp` | Run as an Agent Client Protocol agent over stdio for editor integration. See [ACP Protocol](../integration/02-acp-protocol.md). |
| `config` | Create, validate, display, or upgrade configuration. See [Config Reference](03-config-reference.md). |
| `tap` | Add a registry tap (agent source), list active taps, or scaffold a new tap with `tap init`. |
| `untap` | Remove a previously added tap. |
| `vars` | Show placeholder variables and their resolved values. |
| `send` | Inject a message into a running named session. |
| `workflow` | List tap workflows or run one by name/local TOML file. See [Workflows](../usage/09-workflows.md). |
| `completion` | Generate shell completion scripts. |
| `complete` | Hidden: print cached completion candidates. |
| `distill` | Hidden: extract memories from a disposable transcript snapshot. |
| `help [COMMAND]...` | Clap-generated help, including nested commands such as `help tap init`. |

The global config file lives at `~/.local/share/octomind/config/config.toml` on macOS and Linux
(`%LOCALAPPDATA%\octomind\config\config.toml` on Windows). Override the load path with `OCTOMIND_CONFIG_PATH`, or
relocate the data tree with `OCTOMIND_DATA_DIR`. See [Environment Variables](04-environment-variables.md) for load/save
path differences.

### TAG resolution

`run`, `server`, and `acp` take an optional `TAG`:

- A **role name** (e.g. `assistant`) — matched against `[[roles]]` in your config.
- A **registry agent tag** in `category:variant` form (e.g. `developer:general`) — resolved through your installed [taps](../integration/04-tap-system.md).
- Omitted — uses the `default` tag from config.

Model resolution priority, highest first: explicit runtime override > role profile > tap name mapping > the required
main `[model]`.

Only `run` and `workflow` accept `--format`; neither restricts its value through clap. `jsonl` selects JSON Lines; other
strings use the ordinary output path.

## `octomind login`

Sign in through the browser-confirmed device flow and store the minted OctoHub key in Octomind's environment file.

| Flag | Description |
|------|-------------|
| `--force` | Sign in again even when the current machine already has an account session. |
| `--no-browser` | Print the confirmation URL instead of attempting to open it. |

```bash
octomind login
octomind login --force --no-browser
```

## `octomind config [OPTIONS]`

Create, validate, display, or upgrade configuration. With no flags, see the example note below.

Loading happens before command dispatch and may create or migrate config even with `--show` or `--validate`.

**Mutating flags** (apply changes, then save to the loaded config path):

| Flag | Description |
|------|-------------|
| `--model MODEL` | Set root-level model (`provider:model` format). |
| `--log-level <none\|info\|debug>` | Set the log level (case-insensitive); any other value errors. |
| `--mcp-providers <a,b,c>` | **Replace** the MCP server list: clears all configured servers, then adds each named one as a `builtin` server (timeout 30s). |
| `--mcp-server <name,key=value,...>` | Add or update one MCP server. See [the `--mcp-server` format details](01-cli-reference.md) below. |
| `--system TEXT` | Write/reset the legacy root `system` field. Current session prompts come from `[[roles]].system`, so prefer editing the role instead. |
| `--markdown-enable BOOL` | Enable or disable markdown rendering. |
| `--markdown-theme THEME` | Set the markdown theme (must be one of the themes from `--list-themes`). |

**Inspect / maintenance flags:**

| Flag | Description |
|------|-------------|
| `--show` | Display a selected summary (model, logging, rendering, roles, credentials, and MCP), not a full TOML dump. |
| `--validate` | Validate the configuration without making changes. |
| `--list-themes` | List the available markdown themes. |
| `--upgrade` | Upgrade and rewrite the standard config file when migration is needed. |

**Note on `--api-key`:** the parser accepts an `--api-key provider:key` argument, but the mutation path rejects it at
runtime — API keys can never be stored in the config file for security reasons. Set the provider's environment variable
instead, using the credential example in [Environment Variables](04-environment-variables.md). See [Environment
Variables](04-environment-variables.md) for credential variables.

Inspection precedence is `--list-themes`, `--show`, `--validate`, then `--upgrade`; the first matching mode returns
before mutations (including the `--api-key` rejection). Value options take one value. `--markdown-enable` requires
`true` or `false`; `--system default` clears the legacy prompt.

```bash
octomind config --model octohub:auto --log-level debug
octomind config --markdown-enable true --markdown-theme dark
octomind config --system default
octomind config --mcp-providers core,runtime,orchestration,agent
octomind config --upgrade
```

`--api-key PROVIDER:KEY` has no short flag and takes one string; use environment credentials instead.

### `--mcp-server` format

`--mcp-server name,key=value,...` — the first comma-separated token is the server name; the rest are `key=value` pairs:

| Key | Meaning |
|-----|---------|
| `type` | `http`, `stdio`, or `builtin` (default `http`). |
| `url` | Endpoint URL — **required** for `http`. |
| `command` | Executable to launch — **required** for `stdio`. |
| `args` | Space-separated arguments for a `stdio` command. |
| `timeout` / `timeout_seconds` | Per-operation timeout in seconds (default `30`); tool-call progress resets the idle deadline. |

```bash
# HTTP server
octomind config --mcp-server "search,url=http://localhost:9000,timeout=60"

# stdio server
octomind config --mcp-server "files,type=stdio,command=octofs"
```

**Examples:**

```bash
# Create a default config (only if none exists; otherwise reports
# the current state with no changes — it does NOT regenerate).
octomind config

# Show current settings
octomind config --show

# Validate config
octomind config --validate

# List themes
octomind config --list-themes
```

## `octomind run [TAG]`

Start an interactive or non-interactive AI session.

| Flag | Short | Description |
|------|-------|-------------|
| `TAG` | | Role name (e.g. `assistant`) or registry agent tag `category:variant` (e.g. `developer:general`). Uses the default tag if omitted. |
| `--name NAME` | `-n` | Create a named session, or resume it if it already exists. |
| `--resume [SESSION]` | `-r` | Resume by name; in an interactive TTY, bare `--resume` opens the recent-session picker |
| `--resume-recent` | | Resume the most recent session for the current directory |
| `--format FORMAT` | | Output mode: use `plain` or `jsonl`. Unset by default; non-`jsonl` strings take the plain path. |
| `--model MODEL` | `-m` | Override model (`provider:model` format) |
| `--daemon` | | Keep the session alive for injected messages. Pair with `--format jsonl` for headless use; TTY use remains interactive-capable. |
| `--sandbox` | | Restrict filesystem writes to the working directory. See [Sandbox](#sandbox). |
| `--hook NAME` | | Activate webhook hook(s) by name (defined in `[[hooks]]` config). Repeatable. See [Daemon & Hooks](../integration/03-daemon-and-hooks.md). |
| `--schema PATH` | | Path to a JSON Schema **object** file. Constrains the model's output to match it (structured output). The resolved model must support structured output, or the run fails fast. See note below. |

**Interactivity and `--format`:** `--format` is unset by default. If it is omitted and stdin is a TTY, the session runs
**interactively**. If `--format` is given (`plain` or `jsonl`) **or** stdin is piped, the session runs
**non-interactively**, reading the input from stdin. Internally, an unset format resolves to `plain`.

`run` has no message argument: its only positional argument is `TAG`. With `--format`, a non-daemon run requires
nonempty piped stdin; a TTY produces an error. Resuming without an explicit tag restores the saved role.

**Daemon mode:** `--daemon` keeps the session alive after a turn. Pair it with `--format jsonl` for a headless event
stream. When attached to a TTY without `--format`, startup uses an empty initial input and the terminal stays
interactive. Inject further messages with [`octomind send --name <name>`](#octomind-send).

**Structured output (`--schema`):** pass a path to a JSON Schema **object** file to constrain the model's output. The
schema applies to every assistant reply for the session's lifetime — across multi-turn sessions, resumes, and daemon
mode — while tool calls still flow normally underneath. If the resolved model reports no structured-output support, the
run fails before the provider request. The schema is a runtime-only override and is not persisted, so pass it again when
resuming. Most useful with `--format jsonl`. A ready-to-use example ships at
[`config-templates/todos.schema.json`](../../config-templates/todos.schema.json).

**Examples:**

```bash
# Interactive session with the default tap agent
octomind run

# Explicit default tap agent
octomind run assistant:concierge

# Registry agent (category:variant)
octomind run developer:general

# Non-interactive: pipe message via stdin
echo "Explain the auth module" | octomind run assistant:concierge --format plain

# Named session
octomind run --name feature-auth

# Resume session
octomind run --resume feature-auth
octomind run --resume-recent

# Daemon mode with webhook
echo "Wait for build events" | octomind run --name ci-watcher --daemon --format jsonl --hook github-push

# Main-purpose model override
octomind run -m anthropic:claude-sonnet-4-6

# Structured output — constrain replies to a JSON Schema (structured-output models only)
echo "List the top 3 TODOs" | octomind run developer:general --format jsonl --schema config-templates/todos.schema.json
```

## `octomind server [TAG]`

Start a WebSocket server for remote AI sessions.

| Flag | Short | Description |
|------|-------|-------------|
| `TAG` | | Role name or registry agent tag `category:variant` |
| `--host HOST` | | Bind address (default: `127.0.0.1`) |
| `--port PORT` | `-p` | Port (default: `8080`) |
| `--sandbox` | | Restrict filesystem writes to the working directory. See [Sandbox](#sandbox). |
| `--allow-origin ORIGIN` | | Permit a browser `Origin` header; repeatable. Unlisted origins are rejected, while native clients without `Origin` do not need an entry. |

**Examples:**

```bash
octomind server
octomind server --host 0.0.0.0 --port 9090
octomind server developer:general --sandbox
octomind server --allow-origin http://localhost:3000
```

## `octomind acp [TAG]`

Run as Agent Client Protocol agent over stdio (for editor integration).

| Flag | Short | Description |
|------|-------|-------------|
| `TAG` | | Role name or registry agent tag `category:variant` |
| `--name NAME` | `-n` | Session name (used when client creates a session) |
| `--resume SESSION` | `-r` | Resume a specific session by name |
| `--resume-recent` | | Resume the most recent session |
| `--model MODEL` | `-m` | Override model (`provider:model` format) |
| `--sandbox` | | Restrict filesystem writes to the working directory. See [Sandbox](#sandbox). |
| `--hook NAME` | | Repeatable; parsed into ACP options, but ACP does not currently start webhook listeners. |

**Examples:**

```bash
octomind acp
octomind acp developer:general --sandbox
octomind acp assistant:concierge -m openai:gpt-5.6-luna
```

## `octomind tap [TAP] [PATH]`

Add or list registry taps (Homebrew-style agent sources). Replace `myorg/my-tap` and the local path below with your own
tap. `myorg/my-tap` resolves to the repository `myorg/octomind-my-tap`.

| Argument | Description |
|----------|-------------|
| `TAP` | Tap identifier (`user/repo` format). Omit to list all taps. |
| `PATH` | Optional local path. If provided, symlinks instead of cloning from GitHub. |

**Examples:**

```bash
# List all taps
octomind tap

# Add tap from GitHub
octomind tap myorg/my-tap

# Add local tap (symlink)
octomind tap myorg/my-tap /path/to/local/tap
```

## `octomind tap init <TAP>`

Create a new tap repository from the default tap's scaffold (`scaffolds/tap/` in
[muvon/octomind-tap](https://github.com/muvon/octomind-tap)). Renders the template, validates it, runs `git init`, and
registers the directory as a local tap — the starter agent is runnable immediately.

| Argument | Description |
|----------|-------------|
| `TAP` | New tap identifier (`user/repo` format). |
| `--agent DOMAIN:SPEC` | Starter agent tag. Domain defaults to the repo name, spec to the installed scaffold default; pass `--agent` to make it explicit. |
| `--dir DIR` | Destination directory. Defaults to `./octomind-<repo>`. |

**Examples:**

```bash
# Scaffold ./octomind-team, validate, git init, register as local tap
octomind tap init acme/team --agent team:assistant

# Then run the starter agent
octomind run team:assistant

# Custom starter agent and destination
octomind tap init acme/team --agent legal:contracts --dir ~/work/acme-tap
```

The destination must be missing or an empty directory. Rendering fails if any scaffold token remains unresolved or the
validation command declared by the installed scaffold fails.

## `octomind untap <TAP>`

Remove a previously added tap. `TAP` is required; use an identifier listed by `octomind tap`.

```bash
octomind untap myorg/my-tap
```

## `octomind vars`

Show all placeholder variables and their current values.

| Flag | Short | Description |
|------|-------|-------------|
| `--preview` | `-p` | Show a short preview (up to 3 lines) of each placeholder value |
| `--expand` | `-e` | Show full expanded values for placeholders |

With no flag, `vars` runs in **list** mode (names + descriptions). If both flags are given, `--expand` takes precedence
over `--preview`.

```bash
octomind vars
octomind vars --preview
octomind vars --expand
```

Displays the placeholder set `octomind vars` reports: `{{DATE}}`, `{{SHELL}}`, `{{OS}}`, `{{BINARIES}}`, `{{CWD}}`,
`{{HOME}}`, `{{SYSTEM}}` (complete system info), `{{CONTEXT}}` (README + git status + git tree), `{{GIT_STATUS}}`,
`{{GIT_TREE}}`, and `{{README}}`. (`{{ROLE}}` is substituted in role prompts but is **not** among the values `vars`
lists.)

## `octomind send`

Inject a message into a running named session.

Works against any running session that has started its inject listener (typically a session launched with `--daemon`,
but not exclusively). The message reaches the session over a per-OS transport:

- **Unix:** a Unix domain socket at `<run_dir>/<stem>.sock` (long names are shortened and suffixed with an eight-digit SHA-256 prefix) (run dir is `$XDG_RUNTIME_DIR/octomind/`, or `<system tmp>/octomind-<uid>/` when that variable is unset).
- **Windows:** a named pipe `\\.\pipe\octomind-<name>`.

The session replies `ok` on successful delivery; any other reply is treated as an error and reported.

| Flag | Short | Description |
|------|-------|-------------|
| `--name NAME` | `-n` | Name of the running session to send to (required) |
| `MESSAGE` | | Message text. If omitted, reads from stdin. |

```bash
echo "Check build status" | octomind send --name ci-watcher
octomind send --name ci-watcher "Check build status"
```

## `octomind workflow [NAME|FILE]`

Run a multi-step workflow defined in a TOML file.

| Flag | Short | Description |
|------|-------|-------------|
| `NAME|FILE` | | Tap workflow name or local TOML path. Omit to list available tap workflows. |
| `--dry-run` | | Validate and print the execution plan without running any steps |
| `--format <FORMAT>` | | `jsonl` streams one `assistant` event per step + a final aggregated `cost` event to stdout |

Running a workflow reads input from stdin; listing workflows and `--dry-run` do not. Public tap workflows may reference
only public tap roles; local files are not subject to that restriction. Per-step assistant responses, progress, cost,
and token stats are written to **stderr**. When executing a target, **stdout** receives output only for `--dry-run` (the
execution plan) or `jsonl`. Listing without a target also prints to stdout. With `--format jsonl`, stdout streams one
`{"type":"assistant","content":...,"step":"<name>","session_id":""}` line **as each step completes** (the final result
is simply the last one) followed by one `{"type":"cost",...}` line with aggregated tokens/cost. These are the same event
shapes `octomind run --format jsonl` emits, with an extra `step` field identifying the originating step (omitted in
`run` output). Workflows have no single resumable session, so `session_id` is empty. See
[Workflows](../usage/09-workflows.md).

Create `myflow.toml` in your working directory with this runnable one-step definition:

```toml
name = "refine"

[[steps]]
name = "refine"
role = "task_refiner"
prompt = "{{input}}"
```

```bash
octomind workflow
echo "build a JSON-to-CSV CLI in Rust" | octomind workflow myflow.toml
echo "build a JSON-to-CSV CLI in Rust" | octomind workflow myflow.toml --format jsonl
octomind workflow myflow.toml --dry-run
```

## `octomind completion <SHELL>`

Generate shell completion scripts.

| Argument | Description |
|----------|-------------|
| `SHELL` | Target shell: `bash`, `zsh`, `fish`, `powershell`, `elvish` |

| Shell | Command |
|-------|---------|
| Bash | `octomind completion bash > ~/.local/share/bash-completion/completions/octomind` |
| Zsh | `octomind completion zsh > ~/.zfunc/_octomind` |
| Fish | `octomind completion fish > ~/.config/fish/completions/octomind.fish` |
| PowerShell | `octomind completion powershell > octomind.ps1` |
| Elvish | `octomind completion elvish > octomind.elv` |

Create the destination directory before redirecting output. For Zsh:

```bash
mkdir -p ~/.zfunc
octomind completion zsh > ~/.zfunc/_octomind
```

Add `~/.zfunc` to Zsh’s `fpath` before `compinit` in your shell configuration.

Dynamic agent-tag completion for `octomind run <TAB>` and tap-workflow completion for `octomind workflow <TAB>` are
injected only into the **bash**, **zsh**, and **fish** scripts through the hidden `complete` helper. PowerShell and
Elvish remain static.

## Hidden internal commands

These commands are omitted from normal help but remain callable. `complete` reads cached tap data; an unknown subcommand
prints nothing. `distill` reads and deletes its snapshot before parsing and extracting memories.

| Command / argument | Required / default | Meaning |
|---|---|---|
| `complete SUBCOMMAND` | Required | `run` lists agent tags and configured roles; `workflow` lists public workflows. |
| `distill --messages PATH` | Required | JSON array of session messages; consumed and deleted. |
| `distill --role ROLE` | `""` | Originating role. |
| `distill --project PROJECT` | `""` | Project storage scope. |
| `distill --session SESSION` | `""` | Originating session name. |
| `distill --outcome OUTCOME` | `unknown` | `verified`, `failed`, or `unknown` completion evidence. |

```bash
octomind complete run
octomind complete workflow
# A disposable empty snapshot demonstrates the input shape without inventing a transcript.
printf '[]\n' > /tmp/octomind-empty-transcript.json
octomind distill --messages /tmp/octomind-empty-transcript.json --role developer:general \
  --project demo --session demo --outcome unknown
```

## Sandbox

When enabled, the sandbox restricts writes with Landlock on Linux or Seatbelt on macOS while retaining the state/temp
exceptions defined by each backend. It is active if **either** the config `sandbox` setting **or** the `--sandbox` flag
is set, and it applies only to `run`, `server`, and `acp` — all other subcommands ignore both. Other platforms log that
the sandbox is unsupported.

```bash
octomind run developer:general --sandbox
```

## Common questions

- **Why does `--format` reject startup?** Pipe a nonempty prompt; redirected empty stdin also fails in daemon mode.
- **Why does bare `--resume` fail in a script?** Supply a session name; the picker requires an interactive TTY.
- **Why does `send` fail?** The receiving session must still be running with its inject listener active. A saved
  session alone cannot receive messages.
- **Why does a hook fail?** `--hook` names must exist in `[[hooks]]`; see
  [Daemon & Hooks](../integration/03-daemon-and-hooks.md) for a complete hook configuration.

## Help and version

| Flag | Short | Description |
|------|-------|-------------|
| `--help` | `-h` | Root or subcommand help. |
| `--version` | `-V` | Root version flag; not inherited by subcommands. |

```bash
octomind --version
octomind help tap init
octomind run --help
```

## Source map

CLI registration: [main.rs](../../src/main.rs). Argument structs and behavior: [run.rs](../../src/commands/run.rs),
[config.rs](../../src/commands/config.rs), [server.rs](../../src/commands/server.rs),
[acp.rs](../../src/commands/acp.rs), [tap.rs](../../src/commands/tap.rs), [untap.rs](../../src/commands/untap.rs),
[login.rs](../../src/commands/login.rs), [send.rs](../../src/commands/send.rs), [vars.rs](../../src/commands/vars.rs),
[workflow.rs](../../src/commands/workflow.rs), [complete.rs](../../src/commands/complete.rs),
[distill.rs](../../src/commands/distill.rs). Workflow file schema: [schema.rs](../../src/workflow/schema.rs).
State paths: [directories.rs](../../src/directories.rs).

## See also

- [Session Commands](02-session-commands.md)
- [Config Reference](03-config-reference.md)
- [Environment Variables](04-environment-variables.md)
- [Workflows](../usage/09-workflows.md)
