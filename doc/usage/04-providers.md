# AI Providers

Octomind talks to language models through a unified interface implemented in [octolib](https://github.com/muvon/octolib). octolib wires **20 network providers plus a special `cli` meta-provider** (21 prefixes total), so you can switch models without changing how Octomind works. New providers are added in octolib and automatically become available here.

You pick a model with the `provider:model` format and supply that provider's API key via an **environment variable** (or a `.env` file). That is all most setups need.

> Quick check: run `octomind config --show` at any time to see which provider keys Octomind has detected and where they came from (system environment vs `.env` file).

## Model Format

All models use `provider:model` format:

```toml
model = "openrouter:anthropic/claude-sonnet-4"
model = "openai:gpt-4o"
model = "anthropic:claude-sonnet-4"
model = "deepseek:deepseek-chat"
```

## API Keys Are Environment-Only

API keys can **only** be provided through environment variables (or a `.env` file in the current working directory). You cannot put keys in the config file — that was removed for security, and `octomind config --api-key ...` is always rejected with a message telling you to `export <PROVIDER>_API_KEY=...` instead.

The general pattern is `<PROVIDER>_API_KEY`, with an optional `<PROVIDER>_API_URL` to override the base URL. See the per-provider sections below and [doc/reference/04-environment-variables.md](../reference/04-environment-variables.md) for the complete list.

### Using a `.env` file

Instead of `export`-ing variables, you can drop a `.env` file in your working directory:

```bash
# .env
OPENROUTER_API_KEY=your_key
ANTHROPIC_API_KEY=your_key
```

Octomind loads `.env` on startup and it **overrides** the process environment. An **empty** value is treated as "not set", so a blank `KEY=` line will not satisfy a required key. `octomind config --show` reports whether each detected key came from the system environment or from `.env`.

## Supported Providers

The most common providers are documented in detail below. The full set of provider prefixes octolib recognizes is:

`openrouter`, `openai`, `anthropic`, `google`, `amazon`, `cloudflare`, `deepseek`, `cerebras`, `groq`, `together`, `fireworks`, `nvidia`, `minimax`, `moonshot` (alias `kimi`), `zai`, `byteplus`, `featherless`, `octohub`, `ollama`, `local` — plus the special `cli` meta-provider for local CLI-backed models.

### OpenRouter (recommended)

Access many providers through a single API key. Best for flexibility and model switching.

```bash
export OPENROUTER_API_KEY="your_key"
```

```toml
model = "openrouter:anthropic/claude-sonnet-4"
model = "openrouter:openai/gpt-4o"
model = "openrouter:google/gemini-2.5-flash-preview"
```

Get a key at [openrouter.ai](https://openrouter.ai/).

> When Octomind starts, it sets two OpenRouter attribution headers if they are not already defined: `OPENROUTER_APP_TITLE=Octomind` and `OPENROUTER_HTTP_REFERER=https://octomind.run`. These identify the app to OpenRouter for ranking/attribution; override them by exporting your own values.

### OpenAI

Direct access to GPT models.

```bash
export OPENAI_API_KEY="your_key"
```

```toml
model = "openai:gpt-4o"
model = "openai:gpt-4o-mini"
```

### Anthropic

Direct access to Claude models. Supports prompt caching for cost savings.

```bash
export ANTHROPIC_API_KEY="your_key"
```

```toml
model = "anthropic:claude-sonnet-4"
model = "anthropic:claude-haiku-4-5"
```

### Google (Vertex AI)

Gemini models via Google Cloud.

```bash
# Preferred: point at a service-account JSON file
export GOOGLE_CREDENTIAL_FILE="/path/to/service-account.json"
# Standard Google fallback (also accepted)
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
# Optional project / region overrides
export GOOGLE_CLOUD_PROJECT_ID="your-project"
export GOOGLE_CLOUD_LOCATION="us-central1"
```

```toml
model = "google:gemini-2.5-flash-preview"
```

octolib reads `GOOGLE_CREDENTIAL_FILE` first, then falls back to `GOOGLE_APPLICATION_CREDENTIALS`. Requires a Google Cloud project with the Vertex AI API enabled.

### Amazon (Bedrock)

AWS-hosted models. Bedrock uses a **service-specific API key**, not your regular AWS access keys.

```bash
export AWS_BEARER_TOKEN_BEDROCK="your-bedrock-api-key"
export AWS_BEDROCK_REGION="us-east-1"   # optional, defaults to us-east-1
```

```toml
model = "amazon:anthropic.claude-v2"
```

These are **not** standard AWS access keys (`AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` will not authenticate). Create a Bedrock API key in the AWS console under **IAM → Security credentials → Create service-specific credential → Amazon Bedrock**.

### Cloudflare (Workers AI)

Edge inference with low latency.

```bash
export CLOUDFLARE_API_TOKEN="your_token"
export CLOUDFLARE_ACCOUNT_ID="your_account_id"
```

```toml
model = "cloudflare:@cf/meta/llama-3.1-8b-instruct"
```

octolib's Cloudflare provider reads `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`.

### DeepSeek

Cost-effective models with context caching.

```bash
export DEEPSEEK_API_KEY="your_key"
```

```toml
model = "deepseek:deepseek-chat"
```

### Other providers

The following providers also work via the same `<PROVIDER>_API_KEY` pattern. Each accepts an optional `<PROVIDER>_API_URL` base-URL override unless noted.

| Provider | Format | Env var(s) |
|----------|--------|------------|
| Cerebras | `cerebras:model` | `CEREBRAS_API_KEY` |
| Groq | `groq:model` | `GROQ_API_KEY` |
| Together | `together:model` | `TOGETHER_API_KEY` |
| Fireworks | `fireworks:model` | `FIREWORKS_API_KEY` |
| NVIDIA | `nvidia:model` | `NVIDIA_API_KEY` |
| MiniMax | `minimax:model` | `MINIMAX_API_KEY` |
| Moonshot (Kimi) | `moonshot:model` / `kimi:model` | `MOONSHOT_API_KEY` |
| Z.AI | `zai:model` | `ZAI_API_KEY` |
| BytePlus | `byteplus:model` | `BYTEPLUS_API_KEY` |
| Featherless | `featherless:model` | `FEATHERLESS_API_KEY` |
| OctoHub | `octohub:model` | `OCTOHUB_API_KEY` |
| Ollama (local) | `ollama:model` | `OLLAMA_API_KEY` (optional; defaults to local endpoint) |
| Local / custom | `local:model` | `LOCAL_API_KEY`, `LOCAL_API_URL` |

`moonshot` and `kimi` are aliases for the same provider.

### Local CLI-backed models (`cli`)

The special `cli` meta-provider runs a **local command-line agent** instead of calling a network API. The model string is `cli:<backend>/<model>`, where `<backend>` is one of `codex`, `claude`, `cursor`, `gemini`, or a generic command.

```toml
model = "cli:codex/gpt-5"
```

Because the model runs through a local CLI, **no API key is required** — Octomind skips credential validation entirely for the `cli` provider. Behavior is tuned with backend-specific environment variables, for example for the codex backend:

```bash
export CODEX_COMMAND="codex"            # path/name of the CLI binary
export CODEX_REASONING_EFFORT="medium"  # low | medium | high
export CODEX_SKIP_GIT_CHECK="false"
```

## Provider Comparison

Capability flags below come directly from octolib's provider implementations. "Caching" means the provider reports prompt-cache support; "Structured Output" means it advertises structured/JSON output.

| Provider | Format | Caching | Vision | Structured Output |
|----------|--------|---------|--------|-------------------|
| OpenRouter | `openrouter:provider/model` | Model-dependent | Model-dependent | Model-dependent (defaults to Yes for unknown models) |
| OpenAI | `openai:model` | Yes (automatic, model-dependent) | Yes (GPT-4o+) | Yes |
| Anthropic | `anthropic:model` | Yes (all Claude models) | Yes (Claude 3 and Claude 4) | Model-dependent (via octolib reference capabilities) |
| Google | `google:model` | Yes (Gemini 2.5+/3) | Yes (Gemini) | Yes |
| Amazon | `amazon:model` | Model-dependent | Yes (Claude models) | Model-dependent |
| Cloudflare | `cloudflare:model` | No | Limited | Yes |
| DeepSeek | `deepseek:model` | Yes | No | Yes |

OpenAI caching is automatic (server-side, no client cache markers) for most text models; the pro-tier and audio variants (`gpt-5-pro`, `gpt-5.2-pro`, `gpt-audio`) are excluded.

## Model Selection Strategy

| Use Case | Recommended | Why |
|----------|-------------|-----|
| Main development | `anthropic:claude-sonnet-4` | Best coding, caching support |
| Fast queries / layers | `openai:gpt-4o-mini` | Fast, cheap |
| Compression decisions | `openai:gpt-5-mini` | Current default for `[compression.decision].model` |
| Research / exploration | `openrouter:google/gemini-2.5-flash-preview` | Large context, fast |
| Cost-effective | `deepseek:deepseek-chat` | Lowest cost |

The compression-decision model is configured separately from your main model under `[compression.decision].model`. The shipped default is `openai:gpt-5-mini`; `anthropic:claude-haiku-4-5` is a fine cheaper alternative. See [doc/usage/08-compression.md](08-compression.md) for details.

## Model Resolution

When several places specify a model, Octomind resolves which one actually runs in this priority order:

1. CLI override — `octomind run -m provider:model`
2. The `model` declared by the active role/agent definition (a plain `[[roles]]` entry, or a tap agent's manifest role)
3. The root config `model` — which a `[taps]` entry replaces for a tap agent's tag

> A plain `[[roles]]` entry's `model` is honored directly for `octomind run <role>` (CLI `--model` still wins). A `[taps]` override applies only to tap agents and acts at the `config.model` tier. See [Configuration](03-configuration.md).

## Request Tuning

A few root-config knobs control how Octomind makes provider calls (defaults shown):

```toml
request_timeout_seconds = 300   # per-request timeout
max_retries = 1                 # retry attempts on transient failures
retry_timeout = 30              # seconds between retries
```

The compression sub-pipeline has its own `max_retries`/`retry_timeout`; see [doc/reference/03-config-reference.md](../reference/03-config-reference.md).

## Prompt Caching

Providers that support caching can reduce cost by reusing repeated context (system prompt, tool definitions, prior turns):

- **Anthropic**: caching for all Claude models. The system prompt and tool definitions are marked with the 1h cache TTL; cache writes cost ~1.25x and reads ~0.1x of normal input tokens.
- **OpenAI / Google / DeepSeek**: automatic, server-side caching that the client cannot control (no TTL or cache markers are sent).
- **OpenRouter**: depends on the underlying model (Anthropic models routed via OpenRouter use the same 1h cache markers).

For Anthropic, no configuration is required — caching is always on. The 1h TTL only takes effect for Anthropic (and Anthropic-routed OpenRouter); other providers' caching is server-side regardless of client settings.

### Idle cache keepalive (Anthropic-only, opt-in)

For long-running or idle sessions, Octomind can ping the provider to keep an idle prompt cache warm so the next message still hits the cache:

```toml
cache_keepalive_enabled = false          # opt-in; default off
cache_keepalive_max_idle_seconds = 1800  # stop pinging after this many idle seconds (cap 86400)
```

This is **Anthropic-only**. The ping interval is provider-driven (about 54 minutes for the 1h cache TTL), not configurable here. Keepalive pings consume tokens, and their cost is folded into the session cost.

## Cost Tracking

Every request tracks token usage and cost:

```
/info     # Session overview
/report   # Detailed per-request breakdown
```

Set spending limits:
```toml
max_session_spending_threshold = 5.0   # USD per session
max_request_spending_threshold = 1.0   # USD per request
```

## Switching Models

Change model mid-session:

```
/model openai:gpt-4o
/model anthropic:claude-sonnet-4
```

Or override at startup:
```bash
octomind run -m anthropic:claude-sonnet-4
```

## Troubleshooting

**"Invalid model format"**: Must be `provider:model`. Example: `openrouter:anthropic/claude-sonnet-4`.

**"API key not found"** / credentials missing: API keys come only from environment variables or `.env` — they cannot be set in the config file (`octomind config --api-key` is rejected). Set the provider's `<PROVIDER>_API_KEY` and run `octomind config --show` to confirm Octomind detects it. Remember `cli:` models need no key.

**"Provider does not support structured output for model ..."**: Most providers support structured output — OpenAI, Google, DeepSeek, and Cloudflare always do; OpenRouter and Anthropic depend on the specific model (resolved via octolib reference capabilities). If you hit this error, switch to a model octolib recognizes as structured-output capable.

**Amazon Bedrock auth failures**: Bedrock needs `AWS_BEARER_TOKEN_BEDROCK` (a service-specific Bedrock API key), not `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`. Set `AWS_BEDROCK_REGION` if your models are outside `us-east-1`.

**Google Vertex AI issues**: Ensure `GOOGLE_CREDENTIAL_FILE` (or `GOOGLE_APPLICATION_CREDENTIALS`) points to a valid service-account JSON file and the Vertex AI API is enabled for your project.

## See Also

- [doc/usage/03-configuration.md](03-configuration.md) — config file structure and roles
- [doc/reference/04-environment-variables.md](../reference/04-environment-variables.md) — complete environment-variable reference
- [doc/usage/08-compression.md](08-compression.md) — compression and the compression-decision model
