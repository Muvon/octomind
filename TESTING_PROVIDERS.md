# Testing Multi-Provider Support

## Quick Test Commands

### 1. Test OpenAI Provider
```bash
# Set up your OpenAI API key
export OPENAI_API_KEY="your_openai_api_key_here"

# Test with OpenAI GPT-4o
octodev session --model "openai:gpt-4o" -n test_openai

# Test with OpenAI O1 Preview
octodev session --model "openai:o1-preview" -n test_o1
```

### 2. Test OpenRouter (existing functionality)
```bash
# Set up your OpenRouter API key (if not already set)
export OPENROUTER_API_KEY="your_openrouter_api_key_here"

# Test with explicit OpenRouter provider
octodev session --model "openrouter:anthropic/claude-3.5-sonnet" -n test_openrouter

# Test with legacy format (still works, defaults to OpenRouter)
octodev session --model "anthropic/claude-sonnet-4" -n test_legacy
```

### 3. Test Model Format Parsing
The system now supports both legacy and new formats:

**Legacy Format (defaults to OpenRouter):**
- `anthropic/claude-3.5-sonnet`
- `openai/gpt-4o`
- `google/gemini-pro`

**New Provider Format:**
- `openrouter:anthropic/claude-3.5-sonnet`
- `openai:gpt-4o`
- `openai:o1-preview`

### 4. Test in Different Modes
```bash
# Use OpenAI for chat mode (lighter, faster)
octodev session --mode=chat --model="openai:gpt-4o-mini" -n chat_test

# Use OpenRouter for agent mode (full features)
octodev session --mode=agent --model="openrouter:anthropic/claude-sonnet-4" -n agent_test
```

### 5. Test Configuration
Create or edit `.octodev/config.toml`:

```toml
# Default model (can use either format)
[openrouter]
model = "openai:gpt-4o"

# Mode-specific models
[agent.openrouter]
model = "openrouter:anthropic/claude-sonnet-4"

[chat.openrouter]
model = "openai:gpt-4o-mini"
```

## Expected Behavior

1. **Provider Detection**: The system should automatically detect the provider from the model string
2. **API Key Validation**: You'll get clear error messages if API keys are missing
3. **Model Validation**: The system validates that the provider supports the specified model
4. **Tool Support**: Both providers support MCP tools and function calling
5. **Token Tracking**: Token usage is tracked (cost tracking available for OpenRouter)

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
- Verify the model format: `provider:model` or legacy format