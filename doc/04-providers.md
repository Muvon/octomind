# AI Provider Setup Guide

## Overview

Octomind supports multiple AI providers through a unified interface powered by the **octolib** library. All providers use the **required** `provider:model` format for consistency and support various features like tool calling, caching, and cost tracking.

**Architecture**: Octomind uses octolib as the underlying provider implementation, with an adapter layer that maintains backward compatibility while leveraging the comprehensive provider ecosystem.

## Provider Format

**All models use the `provider:model` format:**
- ✅ Correct: `"openrouter:anthropic/claude-sonnet-4"` or `"anthropic:claude-sonnet-4"`
- ❌ Incorrect: `"anthropic/claude-sonnet-4"` (missing provider prefix)

## Supported Providers

Octomind supports seven AI providers:
- **OpenRouter**: Multi-provider access through single API
- **OpenAI**: Direct API access to GPT models
- **Anthropic**: Direct API access to Claude models
- **Google Vertex AI**: Gemini models via Google Cloud
- **Amazon Bedrock**: Claude and other models via AWS
- **Cloudflare Workers AI**: Edge AI inference
- **DeepSeek**: Cost-effective models

## Supported Providers

### OpenRouter (Recommended)
**Access to multiple AI models through a single API**

- **Format**: `openrouter:provider/model`
- **Features**: Full tool support, caching (Claude models), cost tracking, **vision support**
- **Models**: Anthropic, OpenAI, Google, and many others
- **Vision Models**: All vision-capable models from underlying providers (Claude 3+, GPT-4o, Gemini, Llama 3.2 vision, Pixtral)

#### Setup
```bash
export OPENROUTER_API_KEY="your_openrouter_key"
```

```toml
[openrouter]
model = "openrouter:anthropic/claude-sonnet-4"
api_key = "your_key"  # Optional, can use env var
```

#### Caching & Cost Tracking
- All providers supporting Claude (Anthropic, OpenRouter) enable caching and cost tracking
- Vision models supported via OpenRouter: Claude 3+, GPT-4o, Gemini, Llama 3.2 vision, Pixtral
- Use `octomind run --model <provider:model>` to override the model at runtime

```bash
# Anthropic models via OpenRouter
octomind run --model "openrouter:anthropic/claude-sonnet-4"

# OpenAI models via OpenRouter
octomind run --model "openrouter:openai/gpt-4o"
octomind run --model "openrouter:openai/o1-preview"

# Google models via OpenRouter
octomind run --model "openrouter:google/gemini-1.5-pro"
```

### OpenAI (Direct)
**Direct access to OpenAI models**

- **Format**: `openai:model-name`
- **Features**: Full tool support, built-in cost calculation, **vision support**
- **Models**: GPT-4o, GPT-4o-mini, O1, GPT-3.5
- **Vision Models**: GPT-4o, GPT-4o-mini, GPT-4-turbo, GPT-4-vision

#### Setup
```bash
export OPENAI_API_KEY="your_openai_key"
```

#### Usage
```bash
octomind run --model "openai:gpt-4o"
octomind run --model "openai:gpt-4o-mini"
octomind run --model "openai:o1-preview"
```

#### Responses API Behavior
OpenAI direct uses the Responses API. Octomind tracks the provider response_id automatically and, after the first request, sends only incremental user/system messages or tool outputs instead of the full history.

#### Vision Support
```bash
# Use vision-capable model
octomind run --model "openai:gpt-4o"

# Analyze images
> /image diagram.png
> Explain this architecture
```

#### Pricing (per 1M tokens)
| Model | Input | Output |
|-------|-------|--------|
| gpt-4o | $2.50 | $10.00 |
| gpt-4o-mini | $0.15 | $0.60 |
| o1-preview | $15.00 | $60.00 |

### Anthropic (Direct)
**Direct access to Claude models**

- **Format**: `anthropic:model-name`
- **Features**: Full tool support, caching (3.5 models), cost calculation, **vision support**
- **Models**: Claude 3.5 Sonnet, Claude 3.5 Haiku, Claude 3 Opus
- **Vision Models**: All Claude 3+ models support image analysis

#### Setup
```bash
export ANTHROPIC_API_KEY="your_anthropic_key"
```

#### Usage
```bash
octomind run --model "anthropic:claude-sonnet-4"
octomind run --model "anthropic:claude-3-5-haiku"
octomind run --model "anthropic:claude-3-opus"
```

#### Vision Support
```bash
# Start session with vision-capable model
octomind run --model "anthropic:claude-sonnet-4"

# Attach image and analyze
> /image screenshot.png
> What's in this image?
```

#### Pricing (per 1M tokens)
| Model | Input | Output |
|-------|-------|--------|
| claude-sonnet-4 | $3.00 | $15.00 |
| claude-3-5-haiku | $0.25 | $1.25 |
| claude-3-opus | $15.00 | $75.00 |

### Google Vertex AI
**Google's AI models via Vertex AI**

- **Format**: `google:model-name`
- **Features**: Tool support, cost calculation, **vision support**
- **Models**: Gemini 1.5 Pro, Gemini 1.5 Flash, Gemini 1.0 Pro
- **Vision Models**: Gemini 1.5+, 2.0+, 2.5+ models support image analysis
- **Note**: Requires additional OAuth2 setup

#### Setup
```bash
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
export GOOGLE_PROJECT_ID="your-gcp-project-id"
export GOOGLE_REGION="us-central1"  # Optional
```

#### Google Cloud Setup

1. **Create a Service Account** in Google Cloud Console
2. **Download the JSON key file**
3. **Enable the Vertex AI API** in your project
4. **Set environment variables** as shown above

#### Usage
```bash
octomind run --model "google:gemini-1.5-pro"
octomind run --model "google:gemini-1.5-flash"
```

#### Pricing (per 1M tokens)
| Model | Input | Output |
|-------|-------|--------|
| gemini-1.5-pro | $3.50 | $10.50 |
| gemini-1.5-flash | $0.075 | $0.30 |
| gemini-1.0-pro | $0.50 | $1.50 |

### Amazon Bedrock
**AWS Bedrock AI models**

- **Format**: `amazon:model-name`
- **Features**: Tool support, cost calculation, **vision support**
- **Models**: Claude, Titan, Jurassic, and other AWS Bedrock models
- **Vision Models**: Claude 3+ models on Bedrock support image analysis
- **Note**: Requires AWS credentials

#### Setup
```bash
export AWS_ACCESS_KEY_ID="your_access_key"
export AWS_SECRET_ACCESS_KEY="your_secret_key"
export AWS_REGION="us-east-1"
```

#### Configuration
```toml
[providers.amazon]
region = "us-east-1"
access_key_id = "your_access_key"  # Optional, can use env var
secret_access_key = "your_secret_key"  # Optional, can use env var
```

#### Usage
```bash
octomind run --model "amazon:anthropic.claude-3-sonnet"
octomind run --model "amazon:amazon.titan-text-express"
```

### Cloudflare Workers AI
**Cloudflare's AI models**

- **Format**: `cloudflare:model-name`
- **Features**: Tool support, edge computing, **vision support**
- **Models**: Various models available through Cloudflare Workers AI
- **Vision Models**: Llama 3.2 vision models support image analysis
- **Note**: Requires Cloudflare account and API token

#### Setup
```bash
export CLOUDFLARE_ACCOUNT_ID="your_account_id"
export CLOUDFLARE_API_TOKEN="your_api_token"
```

#### Configuration
```toml
[providers.cloudflare]
account_id = "your_account_id"
api_token = "your_api_token"  # Optional, can use env var
```

#### Usage
```bash
octomind run --model "cloudflare:@cf/meta/llama-2-7b-chat-int8"
octomind run --model "cloudflare:@cf/mistral/mistral-7b-instruct-v0.1"
```

### DeepSeek
**Cost-effective AI models with competitive performance**

- **Format**: `deepseek:model-name`
- **Features**: Tool support, cost-effective pricing, competitive performance
- **Models**: DeepSeek Chat and other models
- **Note**: Requires DeepSeek API key

#### Setup
```bash
export DEEPSEEK_API_KEY="your_deepseek_key"
```

#### Configuration
```toml
[providers.deepseek]
api_key = "your_key"  # Optional, can use env var
```

#### Usage
```bash
octomind run --model "deepseek:deepseek-chat"
octomind run --model "deepseek:deepseek-coder"
```

## Model Selection Strategy

### For Different Use Cases

#### Development Work (Agent Mode)
```toml
[agent.openrouter]
model = "openrouter:anthropic/claude-sonnet-4"  # Best reasoning
```

#### Quick Chat (Chat Mode)
```toml
[chat.openrouter]
model = "openai:gpt-4o-mini"  # Fast and cost-effective
```

#### Code Analysis
```toml
# For complex code analysis
model = "openrouter:anthropic/claude-sonnet-4"

# For fast code search
model = "openai:gpt-4o-mini"
```

#### Layer-Specific Models
```toml
[openrouter]
# Main model for development work
model = "openrouter:anthropic/claude-sonnet-4"

# Individual layer models are configured in [[layers]] sections
# See doc/05-sessions.md for layer configuration details
```

## Cost Optimization

### Model Cost Comparison

**Most Expensive → Least Expensive**
1. `openai:o1-preview` - $15.00/$60.00
2. `anthropic:claude-3-opus` - $15.00/$75.00
3. `google:gemini-1.5-pro` - $3.50/$10.50
4. `anthropic:claude-sonnet-4` - $3.00/$15.00
5. `openai:gpt-4o` - $2.50/$10.00
6. `google:gemini-1.0-pro` - $0.50/$1.50
7. `anthropic:claude-3-5-haiku` - $0.25/$1.25
8. `openai:gpt-4o-mini` - $0.15/$0.60
9. `google:gemini-1.5-flash` - $0.075/$0.30

### Cost-Effective Configuration

```toml
# Use expensive models only for complex reasoning
[agent.openrouter]
model = "openrouter:anthropic/claude-sonnet-4"

# Use cheap models for simple tasks
[chat.openrouter]
model = "google:gemini-1.5-flash"

# Layer-specific cost optimization is done in [[layers]] sections
# Each layer has its own model configuration
# See doc/05-sessions.md for complete layer configuration examples
```

## Caching Support

### Supported Models
- **Anthropic Claude 3.5** models (via OpenRouter or direct)
- **OpenRouter** with Claude models

### Enabling Caching
```bash
# During session, mark cache points
/cache

# Automatic caching threshold
```

```toml
[openrouter]
cache_tokens_pct_threshold = 40  # Auto-cache at 40% context
```

### Benefits
- Reduced cost for repeated context
- Faster response times
- Better token utilization

## Token Counting and Cost Tracking

### Unified Token Calculation System

Octomind uses a unified token counting system that provides accurate estimates matching what's actually sent to API providers. This single source of truth ensures consistency across all systems: display, compression, and cost tracking.

**For detailed information about compression and cost optimization, see [Advanced Features - Smart Adaptive Compression System](./06-advanced.md#smart-adaptive-compression-system) and [Configuration - Smart Adaptive Compression](./03-configuration.md#smart-adaptive-compression).**
#### Token Counting Functions

**`estimate_tokens(text: &str) -> usize`**
- Counts tokens in plain text using tiktoken
- Used for individual message content estimation
- Fast and accurate for any text input

**`estimate_message_tokens(message: Message) -> usize`**
- Counts tokens in a complete message (role + content)
- Includes message overhead (role markers, separators)
- Accounts for message structure in token calculation

**`estimate_session_tokens(messages: &[Message]) -> usize`**
- Counts tokens across multiple messages
- Includes inter-message overhead
- Used for conversation history estimation

**`estimate_full_context_tokens(messages: &[Message], config: &Config) -> usize`**
- Comprehensive token count including:
  - All conversation messages
  - System prompt
  - Tool definitions (MCP tools)
  - Safety margin for response generation
- This is the "single source of truth" used by compression and display
```
Full Context = Messages + System Prompt + Tool Definitions + Safety Margin

Example breakdown:
- Conversation messages: 15,000 tokens
- System prompt: 500 tokens
- Tool definitions (shell, text_editor, etc.): 2,000 tokens
- Safety margin (10%): 1,750 tokens
- Total: 19,250 tokens
```

### Cost Tracking

#### Real-Time Cost Monitoring

Use the `/info` command to see detailed cost breakdown:

```
Session Cost Report:
  Total requests: 5
  Total tokens: 45,000 (input: 30,000 | output: 15,000)
  Total cost: $0.135

  Per-request breakdown:
    Request 1: 8,000 tokens → $0.024
    Request 2: 9,500 tokens → $0.028
    Request 3: 12,000 tokens → $0.036
    Request 4: 7,500 tokens → $0.023
    Request 5: 8,000 tokens → $0.024

  Compression Statistics:
    Total compressions: 2
    Average reduction: 65%
    Tokens saved: 18,000
    Cost saved: $0.054

  Cache Statistics:
    Cache writes: 1 (cost: $0.015)
    Cache reads: 3 (savings: $0.045)
    Net cache benefit: $0.030
```

#### Cost Calculation

Octomind calculates costs based on:
1. **Input tokens**: Tokens sent to the model (higher cost)
2. **Output tokens**: Tokens generated by the model (lower cost)
3. **Cache writes**: 1.25x base cost (Anthropic standard)
4. **Cache reads**: 0.1x base cost (90% savings)

Example cost calculation:
```
Model: anthropic:claude-sonnet-4
Input tokens: 10,000 @ $3.00/1M = $0.03
Output tokens: 2,000 @ $15.00/1M = $0.03
Total: $0.06 per request
```

### Compression Cost-Benefit Analysis

The compression system performs cache-aware cost analysis before compressing:

#### When Compression Saves Money

```
Scenario: 100,000 tokens, target 4x compression (25% size)

Without compression:
- Next 5 turns @ 100k tokens each = 500k tokens
- Cost: 500k × $0.003 = $1.50

With compression:
- Cache invalidation cost: 100k × 0.0025 = $0.25
- Compressed context: 25k tokens
- Next 5 turns @ 25k tokens each = 125k tokens
- Cost: 125k × $0.003 = $0.375
- Total: $0.25 + $0.375 = $0.625
- Savings: $1.50 - $0.625 = $0.875 ✓ COMPRESS
```

#### When Compression Costs Money

```
Scenario: 55,000 tokens, target 2x compression (50% size)

Without compression:
- Next 2 turns @ 55k tokens each = 110k tokens
- Cost: 110k × $0.003 = $0.33

With compression:
- Cache invalidation cost: 55k × 0.0025 = $0.1375
- Compressed context: 27.5k tokens
- Next 2 turns @ 27.5k tokens each = 55k tokens
- Cost: 55k × $0.003 = $0.165
- Total: $0.1375 + $0.165 = $0.3025
- Savings: $0.33 - $0.3025 = $0.0275 ✓ COMPRESS (marginal)

But if only 1 turn remains:
- Cost: $0.1375 + $0.0825 = $0.22
- Savings: $0.33 - $0.22 = $0.11 ✓ COMPRESS

But if 0 turns remain:
- Cost: $0.1375 (just invalidation)
- Savings: $0.33 - $0.1375 = $0.1925 ✓ COMPRESS
```

### Monitoring Token Usage

#### Session Commands

```bash
# Show detailed token and cost breakdown
/info

# Add cache checkpoint (marks point for caching)
/cache

# Show current context with token estimates
/context

# Manually truncate context if needed
/truncate
```

#### Environment Variables for Cost Control

```bash
# Warn when MCP tools generate large outputs
```bash
# Warn when MCP tools generate large outputs
export OCTOMIND_OPENROUTER__MCP_RESPONSE_WARNING_THRESHOLD=20000

# Auto-cache when context reaches this percentage
export OCTOMIND_OPENROUTER__CACHE_TOKENS_PCT_THRESHOLD=40
```

1. **Monitor `/info` regularly**: Track costs and identify expensive operations
2. **Use compression**: Enable adaptive compression to reduce context size
3. **Enable caching**: Use `/cache` to mark important context for caching
4. **Choose models wisely**: Use cheaper models for simple tasks, expensive ones for complex reasoning
5. **Set spending limits**: Configure `max_session_tokens_threshold` to prevent runaway costs
6. **Use decision models**: Set cheaper model for compression decisions (e.g., Claude Haiku)
7. **Review compression stats**: Check if compression is actually saving money in your workflow

## Provider-Specific Features

### OpenRouter Features
- **Multi-provider access**: Single API for multiple models
- **Automatic caching**: For supported models
- **Cost tracking**: Detailed usage reporting
- **Model routing**: Automatic fallbacks

### OpenAI Features
- **Model variety**: Access to GPT-4o, o1, and other OpenAI models
- **Function calling**: Advanced tool integration
- **Structured outputs**: JSON mode support

### Anthropic Features
- **Long context**: Up to 200K tokens
- **Tool use**: Native function calling
- **Caching**: Prompt caching for 3.5 models

### Google Features
- **Multimodal**: Vision and text capabilities
- **Code generation**: Optimized for programming
- **Fast inference**: Especially Flash models

## Troubleshooting

### Common Issues

#### API Key Issues
```bash
# Check if key is set
echo $OPENROUTER_API_KEY

# Test API access
curl -H "Authorization: Bearer $OPENROUTER_API_KEY" https://openrouter.ai/api/v1/models
```

#### Model Format Errors
```
❌ anthropic/claude-sonnet-4
✅ openrouter:anthropic/claude-sonnet-4
✅ anthropic:claude-sonnet-4
```

#### Google Vertex AI Issues
```bash
# Check service account
gcloud auth list

# Test authentication
gcloud auth application-default login
```

### Provider Status

Check provider status:
```bash
# Test different providers
octomind run --model "openrouter:anthropic/claude-sonnet-4"
octomind run --model "openai:gpt-4o-mini"
octomind run --model "anthropic:claude-3-5-haiku"
```

### Debug Mode

Enable debug logging:
```toml
[openrouter]
log_level = "debug"
```

## Migration Guide

### From Old Format

**Old (deprecated):**
```toml
model = "anthropic/claude-sonnet-4"
```

**New (required):**
```toml
model = "openrouter:anthropic/claude-sonnet-4"
# or
model = "anthropic:claude-sonnet-4"
```

### Update Configuration

```bash
# Validate current config
octomind config --validate

# Update to preferred format
octomind config --openrouter-model "openrouter:anthropic/claude-sonnet-4"
```

## Best Practices

1. **Use OpenRouter** for access to multiple providers
2. **Set environment variables** for API keys
3. **Choose models by use case** - expensive for complex, cheap for simple
4. **Enable caching** for repeated work
5. **Monitor costs** with `/info` command
6. **Validate configuration** regularly
7. **Use layer-specific models** for optimization
