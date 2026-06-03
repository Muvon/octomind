# Environment Variables Reference

## API Keys

All API keys are read from environment variables for security. **Never put API keys in config files** — the `octomind config --api-key provider:key` command is intentionally rejected at runtime ("API keys can no longer be set in config file for security reasons") and tells you to export the matching environment variable instead.

Provider authentication is delegated to the underlying LLM layer (octolib). The tables below are a curated view of the most common providers; the [Providers guide](../usage/04-providers.md) documents the complete set. The general rule: **any provider prefix authenticates via its uppercased `<PREFIX>_API_KEY` environment variable** (for example, the `groq:` prefix reads `GROQ_API_KEY`).

### Common providers

| Variable | Provider | Description |
|----------|----------|-------------|
| `OPENROUTER_API_KEY` | OpenRouter | OpenRouter API key ([openrouter.ai](https://openrouter.ai/)). Recommended one-key-many-models entry point. |
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
| `OCTOMIND_CONFIG_PATH` | Override the config **file** path used at load. The value is the path to the primary config TOML; its parent directory becomes the config directory for multi-file merge (all `*.toml` files there are merged). Default file: `~/.local/share/octomind/config/config.toml` (Linux/macOS) or `%LOCALAPPDATA%\octomind\config\config.toml` (Windows). |
| `OCTOMIND_SKILLS` | Comma-delimited skill names to preload at session start (e.g., `programming-rust,git-workflow`). Skills are activated permanently without evaluating declarative rules. |
| `OCTOMIND_CAPABILITIES` | Comma-delimited capability names to force-enable at session start (e.g., `cron,docker`). Bypasses the auto-activation embedding pipeline; capabilities are loaded deterministically regardless of intent matching. Already-active entries are no-ops. |
| `OCTOMIND_SHARE_URL` | Base URL of the web viewer used by `/share` (upload endpoint) and `/analyze` (viewer link). Defaults to `https://octomind.run`. Override only when pointing at a self-hosted instance or a local dev server. |
| `RUST_LOG` | Tracing filter (standard `tracing`/`env_logger` syntax, e.g. `RUST_LOG=debug` or `RUST_LOG=octomind=debug`). In CLI mode, setting it turns on the stderr tracing subscriber (unset = only the colored log macros, no tracing emitted). In ACP/WebSocket/daemon modes it overrides the `log_level`-derived filter for the per-mode debug log file. |

## Installation Script

Variables used by `install.sh` for automated/CI environments.

| Variable | Description |
|----------|-------------|
| `GITHUB_TOKEN` | GitHub API token to avoid rate limits during installation |
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
| `{{CONTEXT}}` | Session context (for layer system prompts) |
| `{{SYSTEM}}` | Parent system prompt (for layer system prompts) |

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
| `HOOK_METHOD` | HTTP method (GET, POST, etc.) |
| `HOOK_PATH` | Request path |
| `HOOK_QUERY` | Query string |
| `HOOK_CONTENT_TYPE` | Content-Type header value |
| `HOOK_SESSION` | Session name the hook is attached to |
| `HOOK_HEADER_*` | Each HTTP header as `HOOK_HEADER_<NAME>` (uppercased, hyphens to underscores) |

## .env File Support

Octomind automatically loads a `.env` file from the working directory at startup, as an alternative to exporting variables in your shell. This is useful for project-specific API keys:

```bash
# .env
OPENROUTER_API_KEY=sk-or-v1-...
ANTHROPIC_API_KEY=sk-ant-...
```

Key behaviors:

- **`.env` overrides the system environment.** When a variable is defined in both, the `.env` value wins.
- **Empty values are treated as "not set."** A variable whose value is empty (or only whitespace) is reported as `NotFound` for API-key source detection — so leaving `OPENROUTER_API_KEY=` empty is the same as not defining it.
- **Source tracking.** The `EnvTracker` records whether each variable came from the system environment (`System`) or the `.env` file (`DotEnv`); this source is shown in debug mode.
