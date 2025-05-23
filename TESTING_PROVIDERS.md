# Testing Multi-Provider Support

## Quick Test Commands

### 1. Test OpenAI Provider
```bash
export OPENAI_API_KEY="your_openai_api_key_here"
octodev session --model "openai:gpt-4o" -n test_openai
octodev session --model "openai:o1-preview" -n test_o1
```

### 2. Test Anthropic Provider
```bash
export ANTHROPIC_API_KEY="your_anthropic_api_key_here"
octodev session --model "anthropic:claude-3-5-sonnet" -n test_anthropic
octodev session --model "anthropic:claude-3-opus" -n test_claude_opus
```

### 3. Test Google Vertex AI Provider
```bash
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
export GOOGLE_PROJECT_ID="your-gcp-project-id"
export GOOGLE_REGION="us-central1"
octodev session --model "google:gemini-1.5-pro" -n test_gemini
octodev session --model "google:gemini-1.5-flash" -n test_gemini_flash
```

### 4. Test OpenRouter (existing functionality)
```bash
export OPENROUTER_API_KEY="your_openrouter_api_key_here"
octodev session --model "openrouter:anthropic/claude-3.5-sonnet" -n test_openrouter
octodev session --model "openrouter:anthropic/claude-sonnet-4" -n test_legacy
```

### 5. Model Format Requirements
The system now REQUIRES the `provider:model` format:

**Supported Formats:**
- `openrouter:anthropic/claude-3.5-sonnet`
- `openrouter:openai/gpt-4o` 
- `openai:gpt-4o`
- `openai:o1-preview`
- `anthropic:claude-3-5-sonnet`
- `anthropic:claude-3-opus`
- `google:gemini-1.5-pro`
- `google:gemini-1.5-flash`

**No Longer Supported (will show error):**
- `anthropic/claude-3.5-sonnet` ❌
- `openai/gpt-4o` ❌
- `gpt-4o` ❌

### 6. Test in Different Modes
```bash
# Use OpenAI for chat mode (lighter, faster)
octodev session --mode=chat --model="openai:gpt-4o-mini" -n chat_test

# Use Anthropic for agent mode (full features)
octodev session --mode=agent --model="anthropic:claude-3-5-sonnet" -n agent_test

# Use Google for development tasks
octodev session --mode=agent --model="google:gemini-1.5-pro" -n dev_test
```

### 7. Test Configuration
Create or edit `.octodev/config.toml`:

```toml
# Default model (must use provider:model format)
[openrouter]
model = "anthropic:claude-3-5-sonnet"

# Mode-specific models
[agent.openrouter]
model = "openrouter:anthropic/claude-sonnet-4"

[chat.openrouter]
model = "openai:gpt-4o-mini"
```

## Expected Behavior

1. **Provider Detection**: The system automatically detects the provider from the model string
2. **API Key Validation**: You'll get clear error messages if API keys are missing
3. **Model Validation**: The system validates that the provider supports the specified model
4. **Tool Support**: Both providers support MCP tools and function calling
5. **Cost Tracking**: 
   - **OpenRouter**: Uses API-provided cost data
   - **OpenAI**: Calculates cost using built-in pricing constants
   - **Anthropic**: Calculates cost using built-in pricing constants
   - **Google**: Calculates cost using built-in pricing constants (Note: requires OAuth2 setup)
6. **Format Enforcement**: Old format without provider prefix will show clear error messages

## Troubleshooting

### OpenAI Provider Issues
- Ensure `OPENAI_API_KEY` environment variable is set
- Verify the model name is correct (e.g., `gpt-4o`, `gpt-3.5-turbo`, `o1-preview`)
- Check API key has sufficient credits

### OpenRouter Provider Issues  
- Ensure `OPENROUTER_API_KEY` environment variable is set
- All existing OpenRouter functionality should work unchanged

### General Issues
- Run with debug mode to see detailed logs: add `log_level = "debug"` to config
- Check that the provider name is spelled correctly in the model string
- **Verify the model format**: Must be `provider:model` (e.g., `openai:gpt-4o`)
- **Old format no longer supported**: `anthropic/claude-3.5-sonnet` will show error "Invalid model format. Must specify provider like 'openai:gpt-4o'"