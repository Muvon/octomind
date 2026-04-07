# Workflows

Workflows are multi-step AI processing pipelines that enhance user requests before the main AI processes them. They implement a planner-executor separation.

> **See also:** [Pipelines](14-pipelines.md) -- deterministic script-driven steps that run *before* workflows. Use pipelines for context gathering and preparation, workflows for AI-driven reasoning.

## Concept

```
User Input --> [Pipeline (scripts)] --> [Workflow (AI)] --> Enhanced Input --> AI Session
                                             |
                                             ├── Step 1: Refine query
                                             ├── Step 2: Research context
                                             └── Step 3: Validate plan
```

The workflow acts as a **planner**: it enriches and clarifies the user's request. The session AI acts as the **executor**: it carries out the refined task.

## Configuration

Define workflows in `[[workflows]]` and reference layers from `[[layers]]`:

```toml
[[workflows]]
name = "developer_workflow"
description = "Two-stage workflow: refine task, then research context"

[[workflows.steps]]
name = "refine"
type = "once"
layer = "task_refiner"

[[workflows.steps]]
name = "research"
type = "once"
layer = "task_researcher"
```

Activate a workflow for a role:

```toml
[[roles]]
name = "developer"
workflow = "developer_workflow"
```

## Step Types

### Once

Execute a layer once.

```toml
[[workflows.steps]]
name = "refine"
type = "once"
layer = "task_refiner"
```

### Loop

Repeat until an exit pattern is matched in the output.

```toml
[[workflows.steps]]
name = "validation_loop"
type = "loop"
max_iterations = 3
exit_pattern = "APPROVED"

  [[workflows.steps.substeps]]
  name = "propose"
  type = "once"
  layer = "task_refiner"

  [[workflows.steps.substeps]]
  name = "validate"
  type = "once"
  layer = "task_researcher"
```

### Foreach

Parse output into items and process each.

```toml
[[workflows.steps]]
name = "process_items"
type = "foreach"
parse_pattern = "\\d+\\. (.+)"
layer = "item_processor"
```

### Conditional

Branch based on pattern matching.

```toml
[[workflows.steps]]
name = "route"
type = "conditional"
condition_pattern = "COMPLEX"
layer = "complex_handler"
```

### Parallel

Execute multiple layers concurrently.

```toml
[[workflows.steps]]
name = "gather"
type = "parallel"
parallel_layers = ["researcher_a", "researcher_b"]
```

## Pattern-Based Control

Workflows use regex patterns for flow control:

- **`exit_pattern`** (loop): Stop iterating when output matches
- **`parse_pattern`** (foreach): Extract items from output using capture groups
- **`condition_pattern`** (conditional): Branch when output matches

## Brain-Inspired MAP Architecture

The MAP (Modular Agentic Planner) system models cognitive processes:

| Module | Brain Region | Role |
|--------|-------------|------|
| TaskDecomposer | aPFC | Break tasks into subtasks |
| Actor | dlPFC | Execute actions |
| Monitor | ACC | Track progress and errors |
| Predictor | OFC | Estimate outcomes |
| Evaluator | OFC | Assess quality |
| Orchestrator | aPFC | Coordinate modules |

MAP templates are available in `config-templates/map-planner.toml` and `config-templates/map-executor.toml`.

## Usage

### In Sessions

```
/workflow                        # List available workflows
/workflow developer_workflow     # Execute specific workflow
```

### Via Role Binding

When a role has `workflow = "developer_workflow"`, the workflow runs automatically on each user message before the AI processes it.

## Layer Requirements

Each workflow step references a layer. Layers must:
- Have a `description` field (required)
- Be defined in `[[layers]]` config

```toml
[[layers]]
name = "task_refiner"
description = "Refines and clarifies user requests"
model = "openrouter:openai/gpt-4.1-mini"
max_tokens = 2048
system_prompt = "You are a query processor..."
temperature = 0.3
input_mode = "last"
output_mode = "none"
output_role = "assistant"

[layers.mcp]
server_refs = []
allowed_tools = []
```

See [Commands and Layers](10-commands-and-layers.md) for layer configuration details.

## Best Practices

1. **Use appropriate output modes**: `"none"` for intermediate steps, `"append"` for final output
2. **Keep patterns robust**: Use anchored regex where possible
3. **Set `max_iterations`** on loops to prevent infinite cycles
4. **Use cheap models** for refinement layers (e.g., `gpt-4.1-mini`)
5. **Monitor costs**: Workflows add overhead -- ensure the improved output justifies it

## Debugging

Enable debug logging to see workflow execution:

```
/loglevel debug
```

Check workflow status:
```
/workflow
```

Common issues:
- **Layer not found**: Ensure the `layer` name in step config matches a `[[layers]]` entry
- **Pattern never matches**: Test regex patterns against expected output
- **Loop runs forever**: Always set `max_iterations`
