# Workflows in Octomind

Workflows are Octomind's brain-inspired planning system that enables complex, multi-step AI processing with validation, feedback loops, and conditional branching.

## 🧠 Core Concept: Planner vs Executor

**CRITICAL**: Workflows act as **PLANNERS**, not executors!

```
┌─────────────────────────────────────────────────────────────┐
│                    BRAIN-INSPIRED ARCHITECTURE               │
│                                                              │
│  User Input                                                  │
│       ↓                                                      │
│  ┌────────────────────────────────────────────────────┐    │
│  │    WORKFLOW (Prefrontal Cortex - Planning)        │    │
│  │                                                     │    │
│  │  • Decomposes tasks                                │    │
│  │  • Validates approaches                            │    │
│  │  • Predicts impacts                                │    │
│  │  • Evaluates quality                               │    │
│  │  • Generates enhanced plan                         │    │
│  └──────────────────┬──────────────────────────────────┘    │
│                     │ Enhanced Plan                          │
│                     ▼                                        │
│  ┌────────────────────────────────────────────────────┐    │
│  │    MAIN MODEL (Motor Cortex - Execution)           │    │
│  │                                                     │    │
│  │  • Receives refined plan                           │    │
│  │  • Uses MCP tools                                  │    │
│  │  • Implements changes                              │    │
│  └────────────────────────────────────────────────────┘    │
│       ↓                                                      │
│  Result                                                      │
└─────────────────────────────────────────────────────────────┘
```

### Why This Architecture?

**Separation of Concerns**:
- **Planning** (workflow): Think, validate, refine, strategize
- **Execution** (main model): Implement, use tools, make changes

**Benefits**:
- ✅ Better plans through validation and iteration
- ✅ Reduced execution errors
- ✅ Multiple planning strategies for same executor
- ✅ Clear debugging (plan failures vs execution failures)
- ✅ Brain-inspired cognitive architecture

## Workflow Types

Workflows support five control flow primitives:

### 1. Once - Single Execution

Execute a layer once and pass output to next step.

```toml
[[workflows.simple.steps]]
name = "analyze"
type = "once"
layer = "analyzer"
```

**Use Cases**:
- Single analysis steps
- Data transformation
- Initial task decomposition

### 2. Loop - Repeat Until Condition

Repeat substeps until exit pattern matches or max iterations reached.

```toml
[[workflows.feedback.steps]]
name = "refine_loop"
type = "loop"
max_iterations = 5
exit_pattern = "COMPLETE"

  [[workflows.feedback.steps.substeps]]
  name = "propose"
  type = "once"
  layer = "proposer"

  [[workflows.feedback.steps.substeps]]
  name = "validate"
  type = "once"
  layer = "validator"
```

**Use Cases**:
- Actor-Monitor feedback loops
- Iterative refinement
- Validation with retry

### 3. Foreach - Iterate Over Items

Parse items from input and execute substeps for each item.

```toml
[[workflows.batch.steps]]
name = "process_subgoals"
type = "foreach"
parse_pattern = "SUBGOAL \\d+: (.*)"

  [[workflows.batch.steps.substeps]]
  name = "implement"
  type = "once"
  layer = "implementer"
```

**Use Cases**:
- Process decomposed subgoals
- Batch operations
- Multi-item workflows

### 4. Conditional - Branch on Pattern

Execute different layers based on pattern matching.

```toml
[[workflows.validation.steps]]
name = "validate_and_branch"
type = "conditional"
layer = "validator"
condition_pattern = "VALID"
on_match = ["predictor", "evaluator"]
on_no_match = ["proposer"]  # Retry
```

**Use Cases**:
- Validation with feedback
- Decision trees
- Error handling

### 5. Parallel - Concurrent Execution

Execute multiple layers in parallel and optionally aggregate results.

```toml
[[workflows.exploration.steps]]
name = "explore_approaches"
type = "parallel"
parallel_layers = ["approach_a", "approach_b", "approach_c"]
aggregator = "evaluator"
```

**Use Cases**:
- Tree search
- Multiple approach exploration
- Parallel analysis

## Pattern-Based Control

Workflows use regex patterns for data extraction and control flow:

### Parse Pattern (Foreach)

Extract items from layer output:

```toml
parse_pattern = "SUBGOAL \\d+: (.*?)(?=\\nSUBGOAL|$)"
```

Matches:
```
SUBGOAL 1: Add dependency
SUBGOAL 2: Create module
SUBGOAL 3: Implement function
```

Extracts: `["Add dependency", "Create module", "Implement function"]`

### Exit Pattern (Loop)

Determine when to exit loop:

```toml
exit_pattern = "COMPLETE|DONE|FINISHED"
```

Loop exits when layer output matches pattern.

### Condition Pattern (Conditional)

Branch based on pattern match:

```toml
condition_pattern = "VALID|APPROVED|OK"
```

Executes `on_match` layers if pattern matches, `on_no_match` otherwise.

## Brain-Inspired MAP Architecture

The Modular Agentic Planner (MAP) architecture from cognitive neuroscience research demonstrates the power of workflows.

### MAP Modules

| Module | Brain Region | Function | Workflow Type |
|--------|--------------|----------|---------------|
| **TaskDecomposer** | aPFC | Breaks tasks into subgoals | `once` |
| **Actor** | dlPFC | Proposes solutions | `once` in `loop` |
| **Monitor** | ACC | Validates proposals | `conditional` |
| **Predictor** | OFC | Predicts impact | `once` |
| **Evaluator** | OFC | Judges quality | `once` |
| **Orchestrator** | aPFC | Checks completion | `once` |

### MAP Workflow Structure

```toml
[workflows.map_development]
description = "Brain-inspired MAP workflow"

# Step 1: Decompose task
[[workflows.map_development.steps]]
name = "decompose"
type = "once"
layer = "task_decomposer"

# Step 2: Execute each subgoal
[[workflows.map_development.steps]]
name = "execute_subgoals"
type = "foreach"
parse_pattern = "SUBGOAL \\d+: (.*)"

  # Subgoal loop with feedback
  [[workflows.map_development.steps.substeps]]
  name = "subgoal_loop"
  type = "loop"
  max_iterations = 5
  exit_pattern = "COMPLETE"

    # Propose solutions
    [[workflows.map_development.steps.substeps.substeps]]
    name = "propose"
    type = "once"
    layer = "actor"

    # Validate with feedback
    [[workflows.map_development.steps.substeps.substeps]]
    name = "validate"
    type = "conditional"
    layer = "monitor"
    condition_pattern = "VALID"
    on_match = ["predictor", "evaluator"]
    on_no_match = ["actor"]  # Retry with feedback

    # Check completion
    [[workflows.map_development.steps.substeps.substeps]]
    name = "check"
    type = "once"
    layer = "orchestrator"

# Step 3: Final verification
[[workflows.map_development.steps]]
name = "final_verification"
type = "once"
layer = "final_orchestrator"
```

### How MAP Works

```
User: "Add JWT authentication to API"
     ↓
TaskDecomposer:
  Output: SUBGOAL 1: Add JWT dependency
          SUBGOAL 2: Create auth middleware
          SUBGOAL 3: Implement token validation
          SUBGOAL 4: Protect existing routes
     ↓
For Each Subgoal (Foreach):
     ↓
  Subgoal Loop (max 5 iterations):
       ↓
    Actor: Proposes 2-3 approaches
       ↓
    Monitor: Validates proposals
       ├─ VALID → Predictor → Evaluator
       └─ INVALID → Back to Actor (with feedback)
       ↓
    Orchestrator: Checks if subgoal complete
       ├─ COMPLETE → Exit loop
       └─ INCOMPLETE → Continue loop
     ↓
Final Orchestrator: Verifies all subgoals complete
     ↓
Workflow Output: Comprehensive, validated implementation plan
     ↓
Main Model: Executes the plan using MCP tools
     ↓
Result: JWT authentication implemented
```

## Configuration

### Role Configuration

Assign workflow to role:

```toml
[[roles]]
name = "developer"
model = "openrouter:anthropic/claude-sonnet-4"
workflow = "map_development"  # Use MAP workflow
system = "You are an elite developer using brain-inspired planning."

[roles.developer.mcp]
server_refs = ["core", "filesystem"]
allowed_tools = ["core:*", "filesystem:*"]
```

### Layer Configuration

Layers used by workflows. **Note: The `description` field is REQUIRED for all layers** (used for agents, commands, and documentation):

```toml
[layers.task_decomposer]
description = "Decomposes tasks into subgoals"
model = "openrouter:anthropic/claude-sonnet-4"
temperature = 0.3
max_tokens = 8192
input_mode = "all"
output_mode = "none"
output_role = "assistant"
system_prompt = """You are a task decomposition specialist.

OUTPUT FORMAT:
SUBGOAL 1: <description>
Success Criteria: <verification>

SUBGOAL 2: <description>
Success Criteria: <verification>"""

[layers.task_decomposer.mcp]
server_refs = ["filesystem"]
allowed_tools = ["view"]
```

## Usage


### Starting a Session with Workflow

```bash
# Use role with workflow
octomind run developer

# Or copy the template to your config directory and set OCTOMIND_CONFIG_PATH
cp config-templates/map-planner.toml ~/.config/octomind/
```

### Using the MAP Planner Template

Octomind includes a ready-to-use MAP (Mindful Autonomous Planner) configuration template at `config-templates/map-planner.toml`. To use it:

```bash
# Copy the template to your project
cp config-templates/map-planner.toml ./my-map-workflow.toml

# Edit the configuration as needed
# Copy the template to your config directory
cp config-templates/map-planner.toml ~/.config/octomind/

# Then start a session — it will pick up the config automatically
octomind run developer
```

The MAP planner template provides brain-inspired task decomposition and goal management out of the box.

### Workflow Execution

When you send a message, the workflow:

1. **Receives** your input
2. **Plans** using configured steps
3. **Validates** approaches
4. **Generates** enhanced plan
5. **Returns** to main model for execution

You'll see workflow progress:

```
═══ Workflow ═══
Brain-inspired MAP workflow

▶ Step 1/3: decompose_task
→ decompose_task

▶ Step 2/3: execute_subgoals
⇉ execute_subgoals (3 items)
  → Item 1/3
  ⟳ subgoal_loop (max: 5)
    → propose_solutions
    ⎇ validate_proposals
      ✓ Condition matched
    → check_completion
    ✓ Loop complete (iteration 2)

✓ Workflow completed in 23.45s
```

### Using the MAP Template

A ready-to-use MAP workflow template is available at `config-templates/map-planner.toml`. To use it:

```bash
# Copy the template to your config directory
cp config-templates/map-planner.toml your-config-dir/

# Then start a session with the config directory
octomind run developer
```

The template includes:
- Pre-configured MAP workflow with TaskDecomposer, Actor, Monitor, Predictor, Evaluator, and Orchestrator layers
- Role configuration for developer workflow
- All necessary layer definitions with proper output formats

## Best Practices

### 1. Output Format Consistency

Ensure layers output in expected format:

```toml
system_prompt = """
OUTPUT FORMAT (CRITICAL):
SUBGOAL 1: <description>
SUBGOAL 2: <description>
"""
```

### 2. Pattern Robustness

Use flexible patterns:

```toml
# Good - handles variations
parse_pattern = "SUBGOAL \\d+: (.*?)(?=\\nSUBGOAL|\\nSuccess|$)"

# Bad - too strict
parse_pattern = "SUBGOAL \\d+: (.*)\\n"
```

### 3. Iteration Limits

Set reasonable max_iterations:

```toml
# Simple tasks
max_iterations = 3

# Complex tasks
max_iterations = 5

# Very complex
max_iterations = 10
```

### 4. Cost Optimization

Use cheaper models for validation:

```toml
[layers.monitor]
model = "openrouter:anthropic/claude-haiku"  # Cheaper

[layers.actor]
model = "openrouter:anthropic/claude-sonnet-4"  # Quality
```

### 5. Clear Exit Conditions

Define clear exit patterns:

```toml
# Good - specific
exit_pattern = "COMPLETE"

# Bad - ambiguous
exit_pattern = "done|ok|good"
```

## Debugging

### Enable Debug Logging

```bash
# In session
/loglevel debug

# Or environment
export OCTOMIND_LOG_LEVEL="debug"
```

### Check Workflow Execution

```bash
# View workflow progress
# Colored output shows:
# → Once steps
# ⟳ Loop steps
# ⇉ Foreach steps
# ⎇ Conditional steps
# ✓ Completions
```

### Common Issues

**Loop never exits**:
- Check exit_pattern matches layer output
- Verify Orchestrator output format
- Enable debug logging

**No items parsed (Foreach)**:
- Check parse_pattern regex
- Verify layer output format
- Test pattern with sample output

**Conditional always branches same way**:
- Check condition_pattern matches output
- Verify Monitor output format
- Test pattern matching

## Examples

### Simple Sequential

```toml
[workflows.simple]
description = "Simple sequential processing"

[[workflows.simple.steps]]
name = "refine"
type = "once"
layer = "task_refiner"

[[workflows.simple.steps]]
name = "research"
type = "once"
layer = "task_researcher"
```

### Validation Loop

```toml
[workflows.validation]
description = "Validation with retry"

[[workflows.validation.steps]]
name = "validation_loop"
type = "loop"
max_iterations = 3
exit_pattern = "VALID"

  [[workflows.validation.steps.substeps]]
  name = "propose"
  type = "once"
  layer = "proposer"

  [[workflows.validation.steps.substeps]]
  name = "validate"
  type = "once"
  layer = "validator"
```

### Batch Processing

```toml
[workflows.batch]
description = "Process multiple items"

[[workflows.batch.steps]]
name = "parse_items"
type = "foreach"
parse_pattern = "ITEM \\d+: (.*)"

  [[workflows.batch.steps.substeps]]
  name = "process"
  type = "once"
  layer = "processor"
```

## Research Background

Workflows are inspired by cognitive neuroscience research on planning in the human brain:

- **Paper**: "A brain-inspired agentic architecture to improve planning with LLMs"
- **Authors**: Webb, Mondal, Momennejad (2025)
- **Journal**: Nature Communications
- **Key Finding**: Modular planning (MAP) outperforms standard LLM approaches

**Results**:
- 74% success vs 11% baseline (Tower of Hanoi)
- 0% invalid actions vs 31% baseline
- Better transfer across tasks

## See Also

- [Configuration Guide](./03-configuration.md) - General configuration
- [Command Layers](./07-command-layers.md) - Layer system basics
- [MAP Testing Guide](../config-examples/MAP_TESTING_GUIDE.md) - Detailed MAP testing
- [Workflow Examples](../config-examples/) - Example configurations


## References

- Webb, T., Mondal, S.S. & Momennejad, I. (2025). "A brain-inspired agentic architecture to improve planning with LLMs." *Nature Communications*, 16, 8633.
- Paper URL: https://www.nature.com/articles/s41467-025-63804-5
