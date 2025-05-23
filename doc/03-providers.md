# AI Provider Setup Guide

## Overview

OctoDev supports multiple AI providers through a unified interface. All providers use the `provider:model` format for consistency and support various features like tool calling, caching, and cost tracking.

## Supported Providers

### OpenRouter (Recommended)
**Access to multiple AI models through a single API**

- **Format**: `openrouter:provider/model`
- **Features**: Full tool support, caching (Claude models), cost tracking
- **Models**: Anthropic, OpenAI, Google, and many others

#### Setup
```bash
export OPENROUTER_API_KEY="your_openrouter_key"
```

```toml
[openrouter]
model = "openrouter:anthropic/claude-sonnet-4"
api_key = "your_key"  # Optional, can use env var
```

#### Popular Models
```bash
# Anthropic models via OpenRouter
octodev session --model "openrouter:anthropic/claude-3.5-sonnet"
octodev session --model "openrouter:anthropic/claude-sonnet-4"

# OpenAI models via OpenRouter  
octodev session --model "openrouter:openai/gpt-4o"
octodev session --model "openrouter:openai/o1-preview"

# Google models via OpenRouter
octodev session --model "openrouter:google/gemini-1.5-pro"
```

### OpenAI (Direct)
**Direct access to OpenAI models**

- **Format**: `openai:model-name`
- **Features**: Full tool support, built-in cost calculation
- **Models**: GPT-4o, GPT-4o-mini, O1, GPT-3.5

#### Setup
```bash
export OPENAI_API_KEY="your_openai_key"
```

#### Usage
```bash
octodev session --model "openai:gpt-4o"
octodev session --model "openai:gpt-4o-mini"
octodev session --model "openai:o1-preview"
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
- **Features**: Full tool support, caching (3.5 models), cost calculation
- **Models**: Claude 3.5 Sonnet, Claude 3.5 Haiku, Claude 3 Opus

#### Setup
```bash
export ANTHROPIC_API_KEY="your_anthropic_key"
```

#### Usage
```bash
octodev session --model "anthropic:claude-3-5-sonnet"
octodev session --model "anthropic:claude-3-5-haiku"
octodev session --model "anthropic:claude-3-opus"
```

#### Pricing (per 1M tokens)
| Model | Input | Output |
|-------|-------|--------|
| claude-3-5-sonnet | $3.00 | $15.00 |
| claude-3-5-haiku | $0.25 | $1.25 |
| claude-3-opus | $15.00 | $75.00 |

### Google Vertex AI
**Google's AI models via Vertex AI**

- **Format**: `google:model-name`
- **Features**: Tool support, cost calculation
- **Models**: Gemini 1.5 Pro, Gemini 1.5 Flash, Gemini 1.0 Pro
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
octodev session --model "google:gemini-1.5-pro"
octodev session --model "google:gemini-1.5-flash"
```

#### Pricing (per 1M tokens)
| Model | Input | Output |
|-------|-------|--------|
| gemini-1.5-pro | $3.50 | $10.50 |
| gemini-1.5-flash | $0.075 | $0.30 |
| gemini-1.0-pro | $0.50 | $1.50 |

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
model = "openrouter:anthropic/claude-3.5-sonnet"

# For fast code search
model = "openai:gpt-4o-mini"
```

#### Layer-Specific Models
```toml
[openrouter]
# Main model for development work
model = "openrouter:anthropic/claude-sonnet-4"

# Lightweight models for processing layers
query_processor_model = "openai:gpt-4o-mini"
context_generator_model = "google:gemini-1.5-flash"
reducer_model = "openai:gpt-4o-mini"
```

## Cost Optimization

### Model Cost Comparison

**Most Expensive → Least Expensive**
1. `openai:o1-preview` - $15.00/$60.00
2. `anthropic:claude-3-opus` - $15.00/$75.00
3. `google:gemini-1.5-pro` - $3.50/$10.50
4. `anthropic:claude-3-5-sonnet` - $3.00/$15.00
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

# Layer-specific cost optimization
query_processor_model = "google:gemini-1.5-flash"
context_generator_model = "openai:gpt-4o-mini"
developer_model = "openrouter:anthropic/claude-sonnet-4"
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

## Provider-Specific Features

### OpenRouter Features
- **Multi-provider access**: Single API for multiple models
- **Automatic caching**: For supported models
- **Cost tracking**: Detailed usage reporting
- **Model routing**: Automatic fallbacks

### OpenAI Features
- **Latest models**: Early access to new models
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
❌ anthropic/claude-3.5-sonnet
✅ openrouter:anthropic/claude-3.5-sonnet
✅ anthropic:claude-3-5-sonnet
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
octodev session --model "openrouter:anthropic/claude-3.5-sonnet"
octodev session --model "openai:gpt-4o-mini"
octodev session --model "anthropic:claude-3-5-haiku"
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
model = "anthropic/claude-3.5-sonnet"
```

**New (required):**
```toml
model = "openrouter:anthropic/claude-3.5-sonnet"
# or
model = "anthropic:claude-3-5-sonnet"
```

### Update Configuration

```bash
# Validate current config
octodev config --validate

# Update to new format
octodev config --openrouter-model "openrouter:anthropic/claude-sonnet-4"
```

## Best Practices

1. **Use OpenRouter** for access to multiple providers
2. **Set environment variables** for API keys
3. **Choose models by use case** - expensive for complex, cheap for simple
4. **Enable caching** for repeated work
5. **Monitor costs** with `/info` command
6. **Validate configuration** regularly
7. **Use layer-specific models** for optimization