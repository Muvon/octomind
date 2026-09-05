# AI Providers

Choose and authenticate models using `provider:model`. This guide is for users configuring OctoHub, direct providers,
local endpoints, or a CLI-backed model.

## Start with OctoHub

OctoHub is the default provider in the shipped configuration. The shortest setup is:

```bash
octomind login
octomind
```

`octomind login` starts a device-authorization flow, displays a short code, and opens a browser approval page. After
approval, Octomind stores `OCTOHUB_API_KEY` in `<data-dir>/config/.env` and the account session in
`<data-dir>/config/auth.json`. You do not need separate credentials for models accessed through that gateway.

The default model is `octohub:auto`. To choose another model exposed by the gateway for one session, pass an explicit
model override:

```bash
octomind run -m 'octohub:<model>'
```

Replace `<model>` with the gateway model identifier. The OctoHub client accepts any non-empty model name; the gateway
decides whether that model is available to the credential.

For an existing alternative OctoHub deployment, set `OCTOHUB_API_URL` to its base URL and `OCTOHUB_API_KEY` to the
credential issued by that deployment. For example, if your gateway listens locally on port 8080:

```bash
export OCTOHUB_API_URL="http://localhost:8080"
export OCTOHUB_API_KEY="your_key"
octomind run -m octohub:auto
```

### Purpose routing with `octohub:auto`

Octomind attaches `X-Model-Purpose` to provider requests so a gateway can route `octohub:auto` by purpose. It uses
exactly three values:

| Purpose | Octomind profile |
|---------|------------------|
| `main` | `[model]` |
| `supervisor` | `[supervisor.model]` |
| `compression` | `[compression.model]` |

The shipped configuration uses `octohub:auto` for all three. These are edits to existing profiles, not a complete
replacement config; preserve their other fields:

```toml
[model]
name = "octohub:auto"

[supervisor.model]
name = "octohub:auto"

[compression.model]
name = "octohub:auto"
```

All supervisor mechanics, including learning, share the `supervisor` purpose. The gateway controls the routing and model
availability; Octomind does not implement its routing policy.

## Bring Your Own Keys

You can skip `octomind login`. For example, export your OpenAI credential:

```bash
export OPENAI_API_KEY="your_key"
```

Replace `your_key` with your credential. In the generated config, change `name` in all three existing profiles while
keeping their other settings. A main-model override alone leaves the shipped internal profiles on OctoHub:

```toml
[model]
name = "openai:gpt-5.6-sol"

[supervisor.model]
name = "openai:gpt-5.6-sol"

[compression.model]
name = "openai:gpt-5.6-sol"
```

Then validate and start a session:

```bash
octomind config --validate
octomind run -m openai:gpt-5.6-sol
```

Use a model your provider account can access. An explicit role model can override the main default; check
[Roles](06-roles.md) if a different model appears.

### Environment File Precedence

Provider clients read credentials from the environment. `octomind config --api-key` refuses to save a key; put
credentials in the environment or a `.env` file.

Octomind loads credentials from three sources, with later sources overriding earlier ones:

1. Process environment
2. User-scope `<data-dir>/config/.env`
3. Project-local `./.env`

A project `.env` can select different credentials from the user file. For example, put this line in `.env`, using your
real key, and keep the file out of version control:

```bash
OPENAI_API_KEY="your_key"
```

Remove stale assignments from later files if you want an exported value to take effect. Empty-value handling varies by
provider; do not use a blank assignment as a credential fallback. `OCTOMIND_CONFIG_PATH` does not change the user
credential directory.

## Local Endpoint URLs

`local:` defaults to `http://localhost:11434/v1/chat/completions`. `ollama:` defaults to
`https://ollama.com/v1/chat/completions`, so set an explicit endpoint to use a local Ollama server:

```bash
export OLLAMA_API_URL="http://localhost:11434/v1/chat/completions"
octomind run -m ollama:glm-5.3
```

This requires that model to be available on your running server. Configure supervisor and compression too if all model
requests should stay local. Use the full endpoint path for `LOCAL_API_URL` and `OLLAMA_API_URL`; `OCTOHUB_API_URL` takes
a base URL. The `openai:` adapter uses the Responses API; use `local:` for a local Chat Completions-compatible endpoint.

## Local CLI-Backed Models

The special `cli` meta-provider executes a local agent CLI and skips provider credential validation. Its format is
`cli:<backend>/<model>`:

```toml
[model]
name = "cli:codex/gpt-5.6-sol"
```

Choose a model accepted by your installed backend, and edit the existing `[model]` table. The backend
must be installed and authenticated separately. Known adapters are `codex`, `claude`, `cursor`, and `gemini`; other
backend names use the generic adapter. The executable defaults to the backend name, except Cursor defaults to
`cursor-agent`.

| Variable pattern | Effect |
|------------------|--------|
| `CLI_<BACKEND>_COMMAND` | Executable name or path; backend name is uppercase in the variable |
| `CLI_<BACKEND>_EXTRA_ARGS` | Extra arguments split on whitespace, without shell quote parsing |
| `CLI_<BACKEND>_MODEL_FLAG` | Override the adapter's model flag |
| `CLI_<BACKEND>_PROMPT_FLAG` | Override the adapter's prompt flag |

The Codex backend also accepts these compatibility variables:

```bash
export CODEX_COMMAND="codex"
export CODEX_REASONING_EFFORT="medium"  # low | medium | high
export CODEX_SKIP_GIT_CHECK="false"
octomind run -m cli:codex/gpt-5.6-sol
```

`CLI_CODEX_COMMAND` and `CLI_CODEX_REASONING_EFFORT` take precedence over their `CODEX_` aliases. The internal profiles
still need their own working providers.

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

Or edit `[model].name` for the persistent default. Role, supervisor, and compression profiles can override the main
profile as described in [Configuration](03-configuration.md#model-profiles-and-purposes).

## Diagnose Provider Setup

Inspect the loaded settings and perform a small provider request:

```bash
octomind config --show
printf '%s\n' 'Reply with OK.' | octomind run --format plain
```

`config --show` reports Octomind sign-in separately from a manually exported gateway key. Its credential rows are
incomplete and include legacy Google/AWS variable names; use the provider reference below for actual inputs. Config
validation checks configuration and model names, not remote authentication or account access.

Common failures:

- `Invalid model format`: include both parts of `provider:model`.
- `Unsupported provider`: use a prefix from the provider reference below.
- Missing credentials: set the variables for the selected prefix or run `octomind login` for the default OctoHub path.
- OctoHub authentication rejected: force a new login to replace the machine's stored gateway credential:

```bash
octomind login --force
```

## Provider Reference

The table reflects `octolib` 0.35.3, locked in `Cargo.lock`. Endpoint variables are optional overrides.

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
| Meta | `meta` | `META_API_KEY`, falling back to `MODEL_API_KEY` when absent | `META_API_URL` |
| OpenCode Zen | `opencode-zen` | `OPENCODE_API_KEY` | `OPENCODE_ZEN_API_URL` |
| OpenCode Go | `opencode-go` | `OPENCODE_API_KEY` | `OPENCODE_GO_API_URL` |
| xAI | `xai` | `XAI_API_KEY` | `XAI_API_URL` |
| OctoHub | `octohub` | `OCTOHUB_API_KEY` when required | `OCTOHUB_API_URL` |
| Ollama | `ollama` | `OLLAMA_API_KEY` is optional | `OLLAMA_API_URL` |
| Local OpenAI-compatible endpoint | `local` | `LOCAL_API_KEY` is optional | `LOCAL_API_URL` |

The historical `google:` prefix is not accepted by the current provider factory; use `google-vertex:` or
`google-studio:`. The `kimi:` prefix is an alias for `moonshot:` and uses `MOONSHOT_API_KEY`.

For another prefix, follow the same export-and-profile procedure above with the corresponding variable and model.
OpenRouter attribution defaults to `Octomind` and `https://octomind.run` only when the variables are absent:

```bash
export OPENROUTER_APP_TITLE="My terminal assistant"
export OPENROUTER_HTTP_REFERER="https://example.com"
```

## See also

- [Configuration](03-configuration.md) — model profiles and precedence
- [Environment Variables](../reference/04-environment-variables.md) — broader runtime-variable reference
- [Compression](08-compression.md) — compression profile behavior
