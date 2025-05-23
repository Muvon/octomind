# Config Migration Test

This test helps verify that the configuration migration works correctly.

## Expected Behavior After Changes

### 1. New Format Models (Should Work)
```bash
# These should work without any errors
export OPENAI_API_KEY="test_key"
octodev session --model "openai:gpt-4o" -n test_new_format

export OPENROUTER_API_KEY="test_key"  
octodev session --model "openrouter:anthropic/claude-3.5-sonnet" -n test_new_format_2
```

### 2. Old Format Models (Should Show Error)
```bash
# These should show clear error messages
octodev session --model "anthropic/claude-3.5-sonnet" -n test_old_format
# Expected error: "Invalid model format 'anthropic/claude-3.5-sonnet'. Must specify provider..."
```

### 3. Config Validation (Should Warn, Not Fail)
If you have an existing `.octodev/config.toml` with old format models, you should see:
- Warning messages about updating the format
- Application continues to work
- Helpful guidance on how to update

### 4. Default Models (Should Use New Format)
All defaults should now use the `provider:model` format:
- Main config: `openrouter:anthropic/claude-sonnet-4`
- Query processor: `openrouter:openai/gpt-4.1-nano`
- Context generator: `openrouter:google/gemini-2.5-flash-preview`
- Reducer: `openrouter:openai/o4-mini`
- GraphRAG: `openrouter:openai/gpt-4.1-nano`

## Testing Your Current Issue

The error you showed:
```
Current model: anthropic/claude-3.5-haiku
...
Configuration validation warning: Invalid model format: 'anthropic/claude-3.7-sonnet'
```

This suggests:
1. Your session started with an old format model (`anthropic/claude-3.5-haiku`)
2. Your config file has an old format model (`anthropic/claude-3.7-sonnet`)

### Solution:
1. **Update your config**: Edit `.octodev/config.toml` to use new format:
   ```toml
   [openrouter]
   model = "openrouter:anthropic/claude-sonnet-4"  # Instead of old format
   ```

2. **Use new format in sessions**:
   ```bash
   octodev session --model "openrouter:anthropic/claude-3.5-haiku"
   # Instead of: --model "anthropic/claude-3.5-haiku"
   ```

The application should now:
- Show helpful warnings instead of failing
- Guide you to the correct format
- Continue working while you migrate