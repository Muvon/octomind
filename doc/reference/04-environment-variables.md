# Environment Variables

Reference for OctoHub login, provider credentials, runtime paths, telemetry, scripts, templates, and platform environment variables.

## API Keys

All API keys are read from environment variables for security. **Never put API keys in config files** — the `octomind config --api-key provider:key` command is intentionally rejected at runtime ("API keys can no longer be set in config file for security reasons") and tells you to export the matching environment variable instead.

Provider authentication is delegated to the underlying LLM layer (octolib). The tables below are a curated view of the most common providers; the [Providers guide](../usage/04-providers.md) documents the complete set. The general rule: **any provider prefix authenticates via its uppercased `<PREFIX>_API_KEY` environment variable** (for example, the `groq:` prefix reads `GROQ_API_KEY`).

### Common providers

| Variable | Provider | Description |
|----------|----------|-------------|
| `OPENROUTER_API_KEY` | OpenRouter | OpenRouter API key ([openrouter.ai](https://openrouter.ai/)) for explicitly selected `openrouter:` models |
| `OPENAI_API_KEY` | OpenAI | OpenAI API key ([platform.openai.com](https://platform.openai.com/)) |
| `ANTHROPIC_API_KEY` | Anthropic | Anthropic API key ([console.anthropic.com](https://console.anthropic.com/)) |
| `DEEPSEEK_API_KEY` | DeepSeek | DeepSeek API key ([platform.deepseek.com](https://platform.deepseek.com/)) |
| `GOOGLE_APPLICATION_CREDENTIALS` | Google (Vertex) | Path to a Google Cloud service-account JSON file |
| `GOOGLE_CREDENTIAL_FILE` | Google (Vertex) | Alternative path to the service-account JSON (tried first, preferred over `GOOGLE_APPLICATION_CREDENTIALS`) |
| `GOOGLE_CLOUD_PROJECT_ID` | Google (Vertex) | GCP project ID used for Vertex routing |
| `GOOGLE_CLOUD_LOCATION` | Google (Vertex) | Vertex region/location (used to build the endpoint) |
| `AWS_BEARER_TOKEN_BEDROCK` | Amazon (Bedrock) | Bedrock service-specific API key. **These are NOT regular AWS access keys** — generate a Bedrock API key, not SigV4 credentials. |
| `AWS_BEDROCK_REGION` | Amazon (Bedrock) | Bedrock region (defaults to `us-east-1` if unset) |
| `CLOUDFLARE_API_TOKEN` | Cloudflare (Workers AI) | Cloudflare Workers AI API token |
| `CLOUDFLARE_ACCOUNT_ID` | Cloudflare (Workers AI) | Cloudflare account ID — **required** alongside `CLOUDFLARE_API_TOKEN`; the provider fails without it. |

### Additional providers

Each of these authenticates with its own `<PREFIX>_API_KEY` variable:

| Variable | Provider prefix | Notes |
|----------|-----------------|-------|
| `CEREBRAS_API_KEY` | `cerebras` | |
| `GROQ_API_KEY` | `groq` | |
| `TOGETHER_API_KEY` | `together` | |
| `FIREWORKS_API_KEY` | `fireworks` | |
| `NVIDIA_API_KEY` | `nvidia` | |
| `MINIMAX_API_KEY` | `minimax` | |
| `MOONSHOT_API_KEY` | `moonshot` | The `kimi` prefix is an alias for `moonshot`. |
| `ZAI_API_KEY` | `zai` | |
| `BYTEPLUS_API_KEY` | `byteplus` | |
| `ALIBABA_API_KEY` | `alibaba` | Defaults to the international endpoint; set `ALIBABA_API_URL` for Token Plan, China or workspace endpoints. |
| `FEATHERLESS_API_KEY` | `featherless` | |
| `OCTOHUB_API_KEY` | `octohub` | |
| `OLLAMA_API_KEY` | `ollama` | Optional — local Ollama typically needs no key. |
| `LOCAL_API_KEY` | `local` | Optional — for self-hosted OpenAI-compatible endpoints. |

> The `cli:` meta-provider (for example `cli:codex/...`) runs a local CLI-backed model and requires **no API key** — credential validation is bypassed for `cli`.

> Some providers also accept an OAuth token as an alternative to an API key: `OPENAI_OAUTH_ACCESS_TOKEN` + `OPENAI_OAUTH_ACCOUNT_ID` (OpenAI) and `ANTHROPIC_OAUTH_ACCESS_TOKEN` (Anthropic). When set, these are used in place of the matching `*_API_KEY`.

### CLI meta-provider (`cli:`) backend variables

The `cli:` provider shells out to a local coding-agent CLI, tuned via backend-specific variables. For the `codex` backend:

| Variable | Description |
|----------|-------------|
| `CODEX_COMMAND` | Path/name of the CLI binary to invoke (default `codex`) |
| `CODEX_REASONING_EFFORT` | Reasoning effort for the codex backend: `low`, `medium`, or `high` |
| `CODEX_SKIP_GIT_CHECK` | Skip codex's git-repo safety check (`true`/`false`) |

(The `CLI_CODEX_*` variants are also recognized.) See [Providers → Local CLI-backed models](../usage/04-providers.md).

### Custom endpoints (API URL overrides)

Most providers accept a `<PREFIX>_API_URL` variable to point at a custom or self-hosted endpoint, including `OPENROUTER_API_URL`, `OPENAI_API_URL`, `ANTHROPIC_API_URL`, `GOOGLE_API_URL`, `AWS_BEDROCK_API_URL`, and `CLOUDFLARE_API_URL`. Leave these unset to use each provider's default endpoint.

Octomind also loads `.env` files from the current directory (see [.env File Support](#env-file-support) below). Variables in `.env` override system environment variables.

## Octomind Configuration

| Variable | Description |
|----------|-------------|
| `OCTOMIND_DATA_DIR` | Override the directory holding **all** octomind state — config, sessions, logs, cache, learning. Default: `~/.local/share/octomind` (Linux/macOS) or `%LOCALAPPDATA%\octomind` (Windows). Redirecting `HOME` does not work on Windows, so this is the portable way to run octomind against a throwaway state directory. |
| `OCTOMIND_CONFIG_PATH` | Override the config **file** path used at load. The value is the path to the primary config TOML; its parent directory becomes the config directory for multi-file merge (all `*.toml` files there are merged). Default file: `~/.local/share/octomind/config/config.toml` (Linux/macOS) or `%LOCALAPPDATA%\octomind\config\config.toml` (Windows). |
| `OCTOMIND_SKILLS` | Comma-delimited **exact skill names** to preload at session start. No aliases, globs, or semantic lookup; unknown names fail individually. |
| `OCTOMIND_CAPABILITIES` | Comma-delimited **exact installed capability names** to force-enable at session start. No provider/tool aliases or fuzzy matching; domain and required-environment gates still apply. |
| `OCTOMIND_API_URL` | Override the account/device-login API base URL, primarily for self-hosted or local development. |
| `OCTOMIND_PANEL_URL` | Override the browser panel origin used to turn account verification URLs into user-facing links. |
| `OCTOMIND_MEDIA_ROOT` | Directory the WebSocket server searches for exactly one attachment file whose name starts with `<id>.`. Default: `/home/octo/.octomind/media`. See [WebSocket Server](../integration/01-websocket-server.md). |
| `OCTOMIND_SHARE_URL` | Base URL of the web viewer used by `/share` (upload endpoint) and `/analyze` (viewer link). Defaults to `https://octomind.run`. Override only when pointing at a self-hosted instance or a local dev server. |
| `OCTOMIND_TELEMETRY` | Set to `0`/`false`/`off`/`no` to disable anonymous usage telemetry for this run, or to any other value to force it on regardless of the config. Unset = follow `telemetry` in the config (default on). See [Telemetry](#telemetry). |
| `DO_NOT_TRACK` | The cross-tool opt-out standard ([consoledonottrack.com](https://consoledonottrack.com)). Any value other than empty/`0`/`false` disables telemetry, and is honoured **before** `OCTOMIND_TELEMETRY` and the config. |
| `RUST_LOG` | Tracing filter (standard `tracing`/`env_logger` syntax, e.g. `RUST_LOG=debug` or `RUST_LOG=octomind=debug`). In CLI mode, setting it turns on the stderr tracing subscriber (unset = only the colored log macros, no tracing emitted). In ACP/WebSocket/daemon modes it overrides the `log_level`-derived filter for the per-mode debug log file. |

## Telemetry

Octomind reports anonymous usage so the CLI can be shaped by evidence rather
than guesses. It is **on by default and prints nothing** — turn it off with
`DO_NOT_TRACK=1`, `OCTOMIND_TELEMETRY=0`, or `telemetry = false` in the config.
Opting out is local and instant: no request is made to announce it, and nothing
is buffered.

**What is sent** — a `start` row per invocation, a `session` row per finished
session, and an `error` row when a command fails:

- subcommand name and the long flag **names** used (never their values)
- CLI version, OS, architecture, install source (brew/cargo/docker/binary)
- whether the run is interactive, in CI, signed in, or a first run
- session shape: agent tag, provider and model id, duration, turns, tool-call
  count, token counts, cost, compression count, MCP server count, outcome, and
  how many times you interrupted it with Ctrl+C
- per-tool call **counts**, per-tool failure **counts**, and per-slash-command
  **counts**. Built-in tool names are sent as themselves; every other (MCP) tool
  is reduced to a fixed category such as `ext:github`, because MCP tool names
  come from your config
- provider failure **counts** by fixed kind (`rate_limit`, `overloaded`, `auth`,
  `context_length`, `server`, `timeout`, `network`) — counts only, never the
  provider's message
- for `octomind workflow`: the workflow's declared `name` (the label inside the
  file, never the path it was loaded from), step count and totals
- a random local install id, generated on your machine, tied to no identity. If
  you are signed in, the event is attributed to your account.

**What is never sent** — your code, prompts, model responses, file paths, tool
arguments, shell commands, environment values, repository names or remotes, and
error messages. Failures are reported only as a fixed slug (`network`,
`timeout`, `io`, `parse`, `other`).

Everything transmitted is a named field on a struct in `src/telemetry.rs`;
there is no free-form field, so nothing else can travel by accident. Events are
buffered in memory and sent once at exit behind a 2-second timeout — telemetry
never delays or fails a command.

## Installation Script

Variables used by `install.sh` for automated/CI environments.

| Variable | Description |
|----------|-------------|
| `GITHUB_TOKEN` | GitHub API token for authenticated installation requests |
| `GH_TOKEN` | Alternative token variable (GitHub CLI convention) |
| `OCTOMIND_INSTALL_DIR` | Override installation directory (default: `~/.local/bin/`) |
| `OCTOMIND_VERSION` | Install a specific version instead of latest |

## OpenRouter-Specific

These attribution headers control how OpenRouter identifies and ranks the app.

| Variable | Default | Description |
|----------|---------|-------------|
| `OPENROUTER_APP_TITLE` | `"Octomind"` | Application title sent to OpenRouter |
| `OPENROUTER_HTTP_REFERER` | `"https://octomind.run"` | HTTP referer sent to OpenRouter |

You normally do not set these yourself: Octomind auto-sets them to the listed defaults at startup (during the `.env` load step, which runs unconditionally even when no `.env` file is present) **only if they are not already defined**. Export your own value to override the default.

## Template Variables

### Substituted in role `system` and `welcome` prompts

These placeholders are resolved by the role prompt processor when a role's `system` or `welcome` text is rendered:

| Variable | Description |
|----------|-------------|
| `{{CWD}}` | Current working directory path |
| `{{ROLE}}` | Active role name |
| `{{DATE}}` | Current date |
| `{{SHELL}}` | User's shell (e.g., `bash`, `zsh`) |
| `{{OS}}` | Operating system name |
| `{{BINARIES}}` | Available development tools and their versions |
| `{{GIT_STATUS}}` | Git repository status (branch, changes) |
| `{{GIT_TREE}}` | Project file tree |
| `{{README}}` | Contents of `README.md` in project root |
| `{{CONTEXT}}` | Project context bundle (README, Git status, tracked tree) |
| `{{SYSTEM}}` | Current system information (shell, OS, working directory, binaries) |

### Shown only by `octomind vars`

`octomind vars` lists current placeholder values for inspection. It exposes the prompt placeholders above **except** `{{ROLE}}` (which the role prompt processor substitutes but `vars` does not list), **plus** the following, which the role prompt processor does **not** substitute (placing `{{HOME}}` in a `system`/`welcome` field leaves it literal):

| Variable | Description |
|----------|-------------|
| `{{HOME}}` | User's home directory path |

## Webhook Hook Environment Variables

Available to hook scripts when processing incoming webhooks.

| Variable | Description |
|----------|-------------|
| `HOOK_NAME` | Name of the hook that triggered |
| `HOOK_METHOD` | HTTP method, always `POST` because other methods are rejected before the script runs |
| `HOOK_PATH` | Request path |
| `HOOK_QUERY` | Query string |
| `HOOK_CONTENT_TYPE` | Content-Type header value |
| `HOOK_SESSION` | Session name the hook is attached to |
| `HOOK_HEADER_*` | Each HTTP header as `HOOK_HEADER_<NAME>` (uppercased, hyphens to underscores) |

## Local Tool and Guardrail Script Variables

These are child-process contracts set by Octomind, not startup configuration:

| Variable | Script surface |
|----------|----------------|
| `OCTOMIND_TOOL_NAME` | Project-local tool name |
| `OCTOMIND_PARAM_<NAME>` | One local-tool parameter, with the parameter name uppercased; complex values are JSON strings |
| `OCTOMIND_WORKDIR` | Local tools, pipes, hooks, validators, and monitors; current session workdir |
| `OCTOMIND_ROLE` | Guardrail pipes and validators; current role |
| `PIPE_NAME`, `PIPE_RUN_COUNT`, `SESSION_MESSAGE_COUNT` | Guardrail `[[pipe]]` identity and per-session counters |
| `OCTOMIND_CAPABILITY`, `OCTOMIND_TOOL`, `OCTOMIND_SUCCESS` | Guardrail `[[hook]]` call metadata; success is `1` or `0` |
| `OCTOMIND_VALIDATOR` | Guardrail `[[validator]]` name |
| `OCTOMIND_MONITOR_ID` | Built-in monitor command identifier |

## Runtime and Platform Variables

| Variable | Effect |
|----------|--------|
| `XDG_RUNTIME_DIR` | On Unix, places session sockets/PID files under `$XDG_RUNTIME_DIR/octomind`; otherwise Octomind uses a per-user system temporary directory. |
| `HF_HUB_CACHE` | Adds an explicit Hugging Face hub cache root when locating the local embedding model. |
| `HF_HOME` | Adds `$HF_HOME/hub` as an embedding-model cache root. |
| `KITTY_WINDOW_ID` | Signals Kitty graphics support for inline image display. |
| `TERM` | A value containing `kitty` also selects the Kitty inline-image protocol. |
| `TERM_PROGRAM` | `ghostty`/`WezTerm` select Kitty graphics; `iTerm.app`/`Tabby`/`vscode` select the iTerm2 image protocol. |

## .env File Support

Octomind automatically loads `.env` files at startup, as an alternative to exporting variables in your shell. This is useful for API keys:

```bash
# .env
OCTOHUB_API_KEY=your-octohub-key
```

Two `.env` locations are loaded, in precedence order (later wins):

1. **User-scope** — `<config_dir>/.env` in the shared config directory (alongside `config.toml`). Shared across all projects.
2. **Project-local** — `./.env` in the current working directory. Overrides the user-scope file.

System environment variables are the base; both `.env` files override them.

Key behaviors:

- **`.env` overrides the system environment.** When a variable is defined in both, the `.env` value wins.
- **Project-local `.env` overrides user-scope `.env`.** A key set in the working directory wins over the same key in the shared config directory.
- **Empty values are treated as "not set."** A variable whose value is empty (or only whitespace) is reported as `NotFound` for API-key source detection — so leaving `OPENROUTER_API_KEY=` empty is the same as not defining it.
- **Source tracking.** The `EnvTracker` records whether each variable came from the system environment (`System`) or the `.env` file (`DotEnv`); this source is shown in debug mode.
