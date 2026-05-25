# Pipelines

Pipelines are deterministic script-driven processing steps that run **before** the main AI session. They execute external scripts (any language) with stdin/stdout piping between steps, providing reliable pre-processing without AI involvement.

## Concept

```
User Input --> [Pipeline (scripts)] --> Enhanced Input --> AI Session
                    |
                    +-- Step 1: detect files (bash)
                    +-- Step 2: gather context (python)
                    +-- Step 3: enrich metadata (ruby)
```

Pipelines handle **deterministic** work: context gathering, file detection, validation, data transformation.

For multi-step **agentic** orchestration (reasoning, code generation, planning across multiple sessions), use the external [`octomind workflow`](09-workflows.md) CLI instead — it sits above sessions, not inside them.

## When to Use Pipelines

| Use Case | Pipeline |
|----------|----------|
| Run `rg` to find relevant files | Yes |
| Parse git history for context | Yes |
| Validate input format | Yes |
| Run linters/formatters | Yes |
| Classify task complexity | Yes |

## Configuration

Define pipelines in `[[pipelines]]` and bind them to roles:

```toml
[[pipelines]]
name = "dev_pipeline"
description = "Gather context before AI processing"

[[pipelines.steps]]
name = "detect_files"
type = "once"
command = "./scripts/detect-files.sh"
timeout = 30

[[pipelines.steps]]
name = "enrich_context"
type = "once"
command = "./scripts/enrich-context.py"
timeout = 60
```

Activate a pipeline for a role:

```toml
[[roles]]
name = "developer"
pipeline = "dev_pipeline"          # deterministic pre-processing
```

## How It Works

1. User sends a message
2. The message is piped as **stdin** to the first pipeline step
3. Each step's **stdout** becomes the next step's **stdin** (Unix-style piping)
4. The final step's stdout replaces the user message as the main model's input
5. Runs on the **first message only**

```
User: "fix the login bug"
    |  stdin
    v
[detect-files.sh]  -->  stdout: "fix the login bug\n\nRelevant: src/auth/login.rs"
    |  stdin
    v
[git-context.sh]   -->  stdout: "fix the login bug\n\nRelevant: src/auth/login.rs\n\nRecent: abc123 fix auth timeout"
    |
    v
[Main model receives this enriched text as its input]
```

### Exit Codes

- **Exit 0** = success. Stdout is passed to the next step.
- **Non-zero exit** = fatal error. Pipeline stops, error is reported. The session is not terminated but the message is not processed further.

### Environment Variables

Each script receives these environment variables:

| Variable | Description |
|----------|-------------|
| `PIPELINE_NAME` | Name of the current pipeline |
| `PIPELINE_STEP` | Name of the current step |
| `PIPELINE_STEP_INDEX` | 1-based index of the current step |
| `PIPELINE_TOTAL_STEPS` | Total number of top-level steps |
| `OCTOMIND_ROLE` | Current role name |
| `OCTOMIND_WORKING_DIR` | Working directory |

### Step Output Display

Each step's result is displayed with timing information:

```
[Pipeline: context_pipeline]
  Step 1/2: detect_files ........................ 0.12s ✓
  Step 2/2: git_context ......................... 0.45s ✓
```

Failed steps show the error inline:

```
  Step 1/2: detect_files ........................ 0.02s ✗
    Error: command not found: ./scripts/detect-files.sh
```

Scripts are executed with their working directory set to the project's working directory. Command paths are resolved relative to this directory.

## Step Types

### Once

Execute a script once.

```toml
[[pipelines.steps]]
name = "gather"
type = "once"
command = "./scripts/gather-context.sh"
timeout = 30
```

### Loop

Repeat substeps until stdout matches an exit pattern.

```toml
[[pipelines.steps]]
name = "resolve"
type = "loop"
max_iterations = 5
exit_pattern = "RESOLVED"

  [[pipelines.steps.substeps]]
  name = "resolve_one"
  type = "once"
  command = "./scripts/resolve-dep.sh"
```

### Foreach

Parse items from input using a regex pattern, run substeps for each item.

```toml
[[pipelines.steps]]
name = "lint_all"
type = "foreach"
parse_pattern = "FILE: (.*)"

  [[pipelines.steps.substeps]]
  name = "lint_one"
  type = "once"
  command = "./scripts/lint-file.sh"
```

### Conditional

Run a script, check stdout against a pattern, branch accordingly.

```toml
[[pipelines.steps]]
name = "classify"
type = "conditional"
command = "./scripts/classify-task.sh"
condition_pattern = "COMPLEX"
on_match = ["./scripts/deep-analysis.sh"]
on_no_match = ["./scripts/quick-scan.sh"]
```

The condition script must exit 0 (non-zero = fatal). Branching is based on stdout pattern matching, not exit codes.

Branch commands execute sequentially with piping — each command's stdout becomes the next command's stdin. The final command's stdout is passed to the next pipeline step.

## Writing Pipeline Scripts

Scripts can be written in any language. Just add a shebang:

```bash
#!/bin/bash
# scripts/detect-files.sh
# Reads user message from stdin, outputs relevant files

INPUT=$(cat)
# Find files related to the user's request
echo "$INPUT"
echo ""
echo "Relevant files:"
rg -l "$(echo "$INPUT" | head -1)" --type rust 2>/dev/null | head -20
```

```python
#!/usr/bin/env python3
# scripts/enrich-context.py
# Reads previous step's output from stdin, adds git context

import sys
import subprocess

input_text = sys.stdin.read()
print(input_text)
print("\nRecent commits:")
result = subprocess.run(["git", "log", "--oneline", "-5"], capture_output=True, text=True)
print(result.stdout)
```

Make scripts executable: `chmod +x scripts/*.sh scripts/*.py`

## Debugging

Enable debug logging to see pipeline execution:

```
/loglevel debug
```

Common issues:
- **Command not found**: Ensure the script path is correct relative to the working directory and the script is executable (`chmod +x`)
- **Script hangs**: Set appropriate `timeout` values. Default is 30 seconds.
- **Empty output**: Check that your script writes to stdout, not stderr
- **Pipeline stops unexpectedly**: Check the script's exit code. Any non-zero exit is fatal.
