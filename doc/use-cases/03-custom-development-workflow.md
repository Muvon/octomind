# Use Case: Custom Development Workflow

Build a multi-stage AI pipeline that refines, researches, and validates tasks before the main AI executes them.

## The Problem

Asking an AI to "fix the login bug" often produces mediocre results because it starts coding immediately without understanding context. You want a structured pipeline: first understand the task, then gather context, then execute.

## Solution

Create a workflow with multiple layers, each handling a different stage.

### Architecture

```
User: "fix the login bug"
    |
    v
[task_refiner] Clarify the request, guess relevant files
    |
    v
[context_researcher] Read code, search patterns, gather context
    |
    v
Main AI Session: now has refined task + gathered context
    |
    v
Executes the fix with full understanding
```

### Step 1: Define Layers

Layers execute via ACP protocol. Each layer needs a matching role that defines the model, system prompt, and tools:

```toml
# In config.toml

[[layers]]
name = "task_refiner"
description = "Clarifies vague requests into actionable tasks"
command = "octomind acp task_refiner"
input_mode = "last"
output_mode = "none"
output_role = "assistant"

[[layers]]
name = "context_researcher"
description = "Gathers codebase context for development tasks"
command = "octomind acp context_researcher"
input_mode = "last"
output_mode = "append"
output_role = "assistant"

# Roles for the layers (model + prompt config lives here)
[[roles]]
name = "task_refiner"
model = "openrouter:openai/gpt-4.1-mini"
max_tokens = 2048
system = """
Take the user's request and make it clearer:
1. If vague, add specificity based on the project context
2. Guess which files might be relevant
3. Keep it concise -- you're refining, not solving

If the request is already clear, return it unchanged.

Respond ONLY with the refined task. No questions.
"""
temperature = 0.3
top_p = 0.7
top_k = 20

[roles.mcp]
server_refs = []

[[roles]]
name = "context_researcher"
model = "openrouter:google/gemini-2.5-flash-preview"
max_tokens = 8192
system = """
You are a research assistant. Gather the most important information for the task.

Strategy:
1. Search for relevant files and functions
2. Read key interfaces and signatures (not full implementations)
3. Note patterns and conventions used in the codebase

Present findings as:
- **Starting Points**: Key files and functions
- **Patterns**: Relevant code conventions
- **Context**: Dependencies and related components

Stay focused. Get starting points, not full implementations.
"""
temperature = 0.3
top_p = 0.7
top_k = 20

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["view"]
```

### Step 2: Define the Workflow

```toml
[[workflows]]
name = "dev_workflow"
description = "Refine task, then research context before execution"

[[workflows.steps]]
name = "refine"
type = "once"
layer = "task_refiner"

[[workflows.steps]]
name = "research"
type = "once"
layer = "context_researcher"
```

### Step 3: Bind to a Role

```toml
[[roles]]
name = "developer"
workflow = "dev_workflow"
temperature = 0.3
system = """
You are an expert developer.
Working directory: {{CWD}}
Git status: {{GIT_STATUS}}
"""

[roles.mcp]
server_refs = ["core", "filesystem", "agent"]
allowed_tools = ["core:*", "filesystem:*", "agent:*"]
```

### Step 4: Use It

```bash
octomind run developer
```

Now when you type "fix the login bug":
1. `task_refiner` (gpt-4.1-mini, cheap) clarifies: "Fix the authentication failure in src/auth/login.rs. Likely files: src/auth/login.rs, src/auth/mod.rs, tests/auth_test.rs"
2. `context_researcher` (gemini-flash, fast) reads those files, finds the relevant functions and patterns
3. Main session (claude-sonnet, powerful) receives the refined task + gathered context and produces a high-quality fix

### Advanced: Validation Loop

Add validation before the main AI acts:

```toml
[[workflows]]
name = "validated_dev"
description = "Refine, research, validate in a loop"

[[workflows.steps]]
name = "refine"
type = "once"
layer = "task_refiner"

[[workflows.steps]]
name = "validate_loop"
type = "loop"
max_iterations = 2
exit_pattern = "READY"

  [[workflows.steps.substeps]]
  name = "research"
  type = "once"
  layer = "context_researcher"

  [[workflows.steps.substeps]]
  name = "validate"
  type = "once"
  layer = "task_validator"
```

With a validator layer:
```toml
[[layers]]
name = "task_validator"
description = "Validates whether enough context has been gathered"
command = "octomind acp task_validator"
input_mode = "last"
output_mode = "none"
output_role = "assistant"

# Role for the validator
[[roles]]
name = "task_validator"
model = "openrouter:openai/gpt-4.1-mini"
max_tokens = 1024
system = """
Review the research output. Is there enough context to proceed?

If yes: respond with READY
If no: respond with what additional information is needed
"""
temperature = 0.2

[roles.mcp]
server_refs = []
```

## Cost Optimization

Each layer can use a different model optimized for its task:

| Layer | Model | Cost | Why |
|-------|-------|------|-----|
| Refiner | gpt-4.1-mini | $0.001 | Simple text processing |
| Researcher | gemini-flash | $0.002 | Fast, large context for code reading |
| Validator | gpt-4.1-mini | $0.001 | Simple yes/no decision |
| Main session | claude-sonnet | $0.01+ | Complex reasoning and code generation |

Total pipeline overhead: ~$0.004 per task for much better results.

## Key Points

- `output_mode = "none"` for intermediate layers that don't show output
- `output_mode = "append"` for layers that add context to the session
- Each layer can have different models, tools, and prompts
- Workflows run automatically when bound to a role via `workflow = "..."`
- Use `/workflow` to trigger manually
- Use cheap models for simple tasks, powerful models where it matters
