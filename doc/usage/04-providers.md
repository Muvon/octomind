# AI Providers

Choose models as `provider:model` — the recommended OctoHub path needs no API keys, and every direct provider works through environment variables.

## Recommended: OctoHub

OctoHub is the default provider in the shipped configuration. The shortest setup is:

```bash
octomind login
octomind
```

`octomind login` starts a device-authorization flow, displays a short code, and opens a browser approval page. After approval, Octomind stores the returned `OCTOHUB_API_KEY` in the user-scope `.env` and keeps the account session in `auth.json`. You do not need API keys or accounts with individual model providers, and free models are included.

The default model is `octohub:auto`. To choose another model exposed by the gateway for one session, pass an explicit model override:

```bash
octomind run -m 'octohub:<model>'
```

Replace `<model>` with the gateway model identifier. The OctoHub client accepts any non-empty model name; the gateway decides whether that model is available to the credential.

[OctoHub](https://github.com/Muvon/octohub) is open source and can be self-hosted. Point Octomind at another deployment with `OCTOHUB_API_URL`; set `OCTOHUB_API_KEY` as well when that deployment requires authentication:

```bash
export OCTOHUB_API_URL="https://your-octohub.example"
export OCTOHUB_API_KEY="your_key"
octomind run -m octohub:auto
```

### Purpose routing with `octohub:auto`

When an OctoHub deployment configures its `[auto]` routing section, `octohub:auto` selects a real model from the request purpose. Octomind sends `X-Model-Purpose` on every model request, using exactly three values:

| Purpose | Octomind profile |
|---------|------------------|
| `main` | `[model]` |
| `supervisor` | `[supervisor.model]` |
| `compression` | `[compression.model]` |

The shipped configuration uses `octohub:auto` for all three:

```toml
[model]
name = "octohub:auto"

[supervisor.model]
name = "octohub:auto"

[compression.model]
name = "octohub:auto"
```

All supervisor mechanics, including learning, share the `supervisor` purpose. Providers other than OctoHub ignore the purpose header. An OctoHub deployment without `[auto]` routing treats `auto` as an unknown model.

## Bring Your Own Keys

You can skip `octomind login`, export credentials for a direct provider, and select a matching model:

```bash
export OPENROUTER_API_KEY="your_key"
octomind run -m 'openrouter:<model>'
```

Replace `<model>` with an identifier accepted by the selected provider. To make the selection persistent, edit the main profile:

```toml
[model]
name = "openai:gpt-5.6-sol"
# name = "anthropic:claude-sonnet-4-6"
# name = "openrouter:moonshotai/kimi-k3"
# name = "deepseek:deepseek-chat"
# name = "ollama:glm-5.3"
```

### Credential storage

Provider credentials are environment-only. Configuration-file API-key fields are rejected, including `octomind config --api-key ...`.

Octomind loads credentials from three sources, with later sources overriding earlier ones:

1. Process environment
2. User-scope `<config_dir>/.env`
3. Project-local `./.env`

An empty or whitespace-only value is treated as unset. A project `.env` can therefore select different credentials from the user-scope file, but credentials should not be committed to the repository.

### Supported direct providers

The table reflects the provider prefixes accepted by the pinned `octolib` dependency. Endpoint variables are optional overrides unless the row says otherwise.

| Provider | Prefix | Credential and routing variables | Endpoint override |
|----------|--------|----------------------------------|-------------------|
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` | `OPENROUTER_API_URL` |
| OpenAI | `openai` | `OPENAI_API_KEY` | `OPENAI_API_URL` |
| Anthropic | `anthropic` | `ANTHROPIC_API_KEY` | `ANTHROPIC_API_URL` |
| Google Vertex AI | `google-vertex` | `GOOGLE_VERTEX_CREDENTIAL_FILE` or `GOOGLE_APPLICATION_CREDENTIALS`; optional `GOOGLE_VERTEX_PROJECT_ID`, `GOOGLE_VERTEX_LOCATION` | `GOOGLE_VERTEX_API_URL` |
| Google AI Studio | `google-studio` | `GOOGLE_STUDIO_API_KEY` | `GOOGLE_STUDIO_API_URL` |
| Amazon Bedrock | `amazon` | `AWS_BEARER_TOKEN_BEDROCK`; optional `AWS_BEDROCK_REGION` | `AWS_BEDROCK_API_URL` |
| Cloudflare Workers AI | `cloudflare` | `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` | `CLOUDFLARE_API_URL` |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY` | — |
| Cerebras | `cerebras` | `CEREBRAS_API_KEY` | `CEREBRAS_API_URL` |
| Groq | `groq` | `GROQ_API_KEY` | `GROQ_API_URL` |
| Together | `together` | `TOGETHER_API_KEY` | — |
| Fireworks | `fireworks` | `FIREWORKS_API_KEY` | `FIREWORKS_API_URL` |
| NVIDIA | `nvidia` | `NVIDIA_API_KEY` | `NVIDIA_API_URL` |
| MiniMax | `minimax` | `MINIMAX_API_KEY` | `MINIMAX_API_URL` |
| Moonshot / Kimi | `moonshot` or `kimi` | `MOONSHOT_API_KEY` | — |
| Z.AI | `zai` | `ZAI_API_KEY` | `ZAI_API_URL` |
| BytePlus | `byteplus` | `BYTEPLUS_API_KEY` | `BYTEPLUS_API_URL` |
| Alibaba Model Studio | `alibaba` | `ALIBABA_API_KEY` | `ALIBABA_API_URL` |
| Featherless | `featherless` | `FEATHERLESS_API_KEY` | `FEATHERLESS_API_URL` |
| Hetzner | `hetzner` | `HETZNER_API_KEY` | `HETZNER_API_URL` |
| OpenCode Zen | `opencode-zen` | `OPENCODE_API_KEY` | `OPENCODE_ZEN_API_URL` |
| OpenCode Go | `opencode-go` | `OPENCODE_API_KEY` | `OPENCODE_GO_API_URL` |
| xAI | `xai` | `XAI_API_KEY` | `XAI_API_URL` |
| OctoHub | `octohub` | `OCTOHUB_API_KEY` when required | `OCTOHUB_API_URL` |
| Ollama | `ollama` | `OLLAMA_API_KEY` is optional | `OLLAMA_API_URL` |
| Local OpenAI-compatible endpoint | `local` | `LOCAL_API_KEY` is optional | `LOCAL_API_URL` |

The historical `google:` prefix is not accepted by the current provider factory; use `google-vertex:` or `google-studio:`. The `kimi:` prefix is an alias for `moonshot:` and uses `MOONSHOT_API_KEY`.

OpenRouter attribution defaults are set at startup only when absent: `OPENROUTER_APP_TITLE=Octomind` and `OPENROUTER_HTTP_REFERER=https://octomind.run`. Export either variable to override it.

## Local CLI-Backed Models

The special `cli` meta-provider executes a local agent CLI and skips provider credential validation. Its format is `cli:<backend>/<model>`:

```toml
[model]
name = "cli:codex/<model>"
```

Known backends are `codex`, `claude`, `cursor`, and `gemini`; any other backend name uses the generic CLI adapter. The executable defaults to the backend name, except Cursor defaults to `cursor-agent`. Configure a backend with `CLI_<BACKEND>_COMMAND`, `CLI_<BACKEND>_EXTRA_ARGS`, `CLI_<BACKEND>_MODEL_FLAG`, and `CLI_<BACKEND>_PROMPT_FLAG`.

The Codex backend also accepts these compatibility variables:

```bash
export CODEX_COMMAND="codex"
export CODEX_REASONING_EFFORT="medium"  # low | medium | high
export CODEX_SKIP_GIT_CHECK="false"
```

## Switch Models

Override only the current invocation:

```bash
octomind run -m 'anthropic:claude-sonnet-4-6'
```

Change the active session:

```text
/model openai:gpt-5.6-sol
/model anthropic:claude-sonnet-4-6
/model octohub:auto
```

Or edit `[model].name` for the persistent default. Role, supervisor, and compression profiles can override the main profile as described in [Configuration](03-configuration.md#model-profiles-and-purposes).

## Diagnose Provider Setup

Run `octomind config --show` to inspect the current model and the credential rows it exposes. It reports Octomind sign-in separately from manually exported OctoHub credentials.

Common failures:

- `Invalid model format`: include both parts of `provider:model`.
- `Unknown provider`: use a prefix from the table above.
- Missing credentials: set the variables for the selected prefix or run `octomind login` for the default OctoHub path.
- OctoHub authentication rejected: run `octomind login` again to replace the machine's stored gateway credential.

## See Also

- [Configuration](03-configuration.md) — model profiles and precedence
- [Environment Variables](../reference/04-environment-variables.md) — broader runtime-variable reference
- [Compression](08-compression.md) — compression profile behavior
