# Environment Variables Reference

## API Keys

All API keys are read from environment variables for security. Never put API keys in config files.

| Variable | Provider | Description |
|----------|----------|-------------|
| `OPENROUTER_API_KEY` | OpenRouter | OpenRouter API key ([openrouter.ai](https://openrouter.ai/)) |
| `OPENAI_API_KEY` | OpenAI | OpenAI API key ([platform.openai.com](https://platform.openai.com/)) |
| `ANTHROPIC_API_KEY` | Anthropic | Anthropic API key ([console.anthropic.com](https://console.anthropic.com/)) |
| `DEEPSEEK_API_KEY` | DeepSeek | DeepSeek API key ([platform.deepseek.com](https://platform.deepseek.com/)) |
| `GOOGLE_APPLICATION_CREDENTIALS` | Google | Path to Google Cloud credentials JSON file |
| `AWS_ACCESS_KEY_ID` | Amazon | AWS access key for Bedrock |
| `AWS_SECRET_ACCESS_KEY` | Amazon | AWS secret key for Bedrock |
| `AWS_REGION` | Amazon | AWS region for Bedrock |
| `CLOUDFLARE_API_TOKEN` | Cloudflare | Cloudflare Workers AI token |

Octomind also loads `.env` files from the current directory via `dotenvy`. Variables in `.env` override system environment variables.

## Octomind Configuration

| Variable | Description |
|----------|-------------|
| `OCTOMIND_CONFIG_PATH` | Override config directory path (default: `~/.local/share/octomind/config/`) |
| `OCTOMIND_SKILLS` | Comma-delimited skill names to preload at session start (e.g., `programming-rust,git-workflow`). Skills are activated permanently without evaluating declarative rules. |
| `OCTOMIND_CAPABILITIES` | Comma-delimited capability names to force-enable at session start (e.g., `cron,docker`). Bypasses the auto-activation embedding pipeline; capabilities are loaded deterministically regardless of intent matching. Already-active entries are no-ops. |

## Installation Script

Variables used by `install.sh` for automated/CI environments.

| Variable | Description |
|----------|-------------|
| `GITHUB_TOKEN` | GitHub API token to avoid rate limits during installation |
| `GH_TOKEN` | Alternative token variable (GitHub CLI convention) |
| `OCTOMIND_INSTALL_DIR` | Override installation directory (default: `~/.local/bin/`) |
| `OCTOMIND_VERSION` | Install a specific version instead of latest |

## OpenRouter-Specific

| Variable | Default | Description |
|----------|---------|-------------|
| `OPENROUTER_APP_TITLE` | `"Octomind"` | Application title sent to OpenRouter |
| `OPENROUTER_HTTP_REFERER` | `"https://octomind.muvon.io"` | HTTP referer sent to OpenRouter |

## Template Variables

Available in `system`, `welcome`, and `system_prompt` config fields. Use `octomind vars` to see current values.

| Variable | Description |
|----------|-------------|
| `{{CWD}}` | Current working directory path |
| `{{ROLE}}` | Active role name |
| `{{DATE}}` | Current date |
| `{{SHELL}}` | User's shell (e.g., `bash`, `zsh`) |
| `{{OS}}` | Operating system name |
| `{{BINARIES}}` | Available binary tools on PATH |
| `{{GIT_STATUS}}` | Git repository status (branch, changes) |
| `{{README}}` | Contents of `README.md` in project root |
| `{{CONTEXT}}` | Session context (for layer system prompts) |
| `{{SYSTEM}}` | Parent system prompt (for layer system prompts) |

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

Octomind automatically loads `.env` files from the working directory. This is useful for project-specific API keys:

```bash
# .env
OPENROUTER_API_KEY=sk-or-v1-...
ANTHROPIC_API_KEY=sk-ant-...
```

The `EnvTracker` system tracks whether each variable came from the system environment or `.env` file, which can be seen in debug mode.
