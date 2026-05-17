# AI Providers

Octomind supports 7 AI providers through a unified interface. Provider support is implemented in [octolib](https://github.com/muvon/octolib) -- new providers are added there and automatically become available in Octomind.

All provider types are re-exported from `src/providers.rs`:
- `AiProvider` (trait), `ProviderFactory`
- `OpenRouterProvider`, `OpenAiProvider`, `AnthropicProvider`, `GoogleVertexProvider`
- `AmazonBedrockProvider`, `CloudflareWorkersAiProvider`, `DeepSeekProvider`
- `GenericToolCall`, `StructuredOutputRequest` — tool/schema types
- `ModelPricing`, `ProviderExchange`, `ThinkingBlock`, `TokenUsage` — metadata types

## Model Format

All models use `provider:model` format:

```toml
model = "openrouter:anthropic/claude-sonnet-4"
model = "openai:gpt-4o"
model = "anthropic:claude-sonnet-4"
model = "deepseek:deepseek-chat"
```

## Supported Providers

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
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/credentials.json"
```

```toml
model = "google:gemini-2.5-flash-preview"
```

Requires a Google Cloud project with Vertex AI API enabled.

### Amazon (Bedrock)

AWS-hosted models.

```bash
export AWS_ACCESS_KEY_ID="your_key"
export AWS_SECRET_ACCESS_KEY="your_secret"
export AWS_REGION="us-east-1"
```

```toml
model = "amazon:anthropic.claude-v2"
```

### Cloudflare (Workers AI)

Edge inference with low latency.

```bash
export CLOUDFLARE_API_TOKEN="your_token"
```

```toml
model = "cloudflare:@cf/meta/llama-3.1-8b-instruct"
```

### DeepSeek

Cost-effective models.

```bash
export DEEPSEEK_API_KEY="your_key"
```

```toml
model = "deepseek:deepseek-chat"
```

## Provider Comparison

| Provider | Format | Caching | Vision | Structured Output |
|----------|--------|---------|--------|-------------------|
| OpenRouter | `openrouter:provider/model` | Yes (model-dependent) | Yes | Model-dependent |
| OpenAI | `openai:model` | No | Yes (GPT-4o+) | Yes |
| Anthropic | `anthropic:model` | Yes (Claude 3.5+) | Yes (Claude 3+) | Yes (tool-based) |
| Google | `google:model` | No | Yes (Gemini 1.5+) | No |
| Amazon | `amazon:model` | No | Yes (Claude models) | No |
| Cloudflare | `cloudflare:model` | No | Limited | No |
| DeepSeek | `deepseek:model` | No | No | No |

## Model Selection Strategy

| Use Case | Recommended | Why |
|----------|-------------|-----|
| Main development | `anthropic:claude-sonnet-4` | Best coding, caching support |
| Fast queries / layers | `openai:gpt-4o-mini` | Fast, cheap |
| Compression decisions | `anthropic:claude-haiku-4-5` | 10x cheaper than Sonnet |
| Research / exploration | `openrouter:google/gemini-2.5-flash-preview` | Large context, fast |
| Cost-effective | `deepseek:deepseek-chat` | Lowest cost |

## Prompt Caching

Providers with caching support can reduce costs by caching repeated context:

- **Anthropic**: Automatic for Claude 3.5+ models. Cache write at 1.25x, read at 0.1x cost.
- **OpenRouter**: Depends on underlying model.

Caching is always-on for supporting providers — the cache marker is moved to the latest message on every turn and uses the provider's long (1h) TTL. No configuration required.

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

**"API key not found"**: Set the provider's environment variable. Use `octomind config --show` to check.

**"Provider does not support structured output"**: Not all providers support `--schema`. Use OpenAI or Anthropic for structured output.

**Google Vertex AI issues**: Ensure `GOOGLE_APPLICATION_CREDENTIALS` points to a valid JSON file and the Vertex AI API is enabled.
