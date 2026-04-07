# Use Case: Deterministic Pipelines for Context Preparation

Use pipelines to gather and prepare context deterministically before AI processing, improving response quality and reducing token usage.

## The Problem

AI workflows (layers) are powerful but expensive and non-deterministic. Using an LLM to run `rg`, parse git logs, or detect file types wastes tokens on work that a simple script handles perfectly. You want deterministic, reliable pre-processing before the AI even starts.

## Solution

Create a pipeline of scripts that gather context, then pass the enriched input to the workflow and main AI.

### Architecture

```
User: "fix the login bug"
    |
    v
[Pipeline: detect-files.sh]     <-- bash, instant, free
    Finds: src/auth/login.rs, src/auth/mod.rs
    |
    v
[Pipeline: git-context.sh]      <-- bash, instant, free
    Adds: recent commits touching those files
    |
    v
[Workflow: task_refiner]         <-- LLM, but now has context
    Refines with real file info, not guessing
    |
    v
Main AI Session                  <-- starts with full context
```

### Step 1: Write the Scripts

**`scripts/detect-files.sh`** -- find relevant files from the user's message:

```bash
#!/bin/bash
INPUT=$(cat)
echo "$INPUT"
echo ""
echo "=== Detected Files ==="

# Extract potential keywords from the first line
KEYWORDS=$(echo "$INPUT" | head -1 | tr ' ' '\n' | grep -v '^the$\|^a$\|^in$\|^to$\|^fix$' | head -5)

for word in $KEYWORDS; do
    FOUND=$(rg -l -i "$word" --type rust 2>/dev/null | head -5)
    if [ -n "$FOUND" ]; then
        echo "$FOUND"
    fi
done | sort -u
```

**`scripts/git-context.sh`** -- add recent git history for detected files:

```bash
#!/bin/bash
INPUT=$(cat)
echo "$INPUT"
echo ""
echo "=== Recent Changes ==="

# Extract file paths from input
FILES=$(echo "$INPUT" | grep -E '\.rs$|\.py$|\.ts$|\.go$' | head -10)

for file in $FILES; do
    if [ -f "$file" ]; then
        echo "--- $file ---"
        git log --oneline -3 -- "$file" 2>/dev/null
    fi
done
```

Make them executable:

```bash
chmod +x scripts/detect-files.sh scripts/git-context.sh
```

### Step 2: Define the Pipeline

```toml
[[pipelines]]
name = "context_pipeline"
description = "Gather file and git context before AI processing"

[[pipelines.steps]]
name = "detect_files"
type = "once"
command = "./scripts/detect-files.sh"
timeout = 10

[[pipelines.steps]]
name = "git_context"
type = "once"
command = "./scripts/git-context.sh"
timeout = 10
```

### Step 3: Combine with a Workflow

```toml
[[workflows]]
name = "enhanced_dev"
description = "Refine task using pipeline-gathered context"

[[workflows.steps]]
name = "refine"
type = "once"
layer = "task_refiner"
```

### Step 4: Bind to Role

```toml
[[roles]]
name = "developer"
pipeline = "context_pipeline"
workflow = "enhanced_dev"
temperature = 0.3
# ...
```

### Step 5: Use It

```bash
octomind run developer
```

Type "fix the login bug" and the pipeline:
1. Detects relevant files (instant, free)
2. Gathers git history for those files (instant, free)
3. Passes enriched context to the workflow's task refiner (LLM, but now well-informed)
4. Main AI receives a refined task with real code context

## Advanced: Conditional Pipeline

Route different task types through different scripts:

```toml
[[pipelines]]
name = "smart_pipeline"
description = "Route tasks based on type"

[[pipelines.steps]]
name = "classify"
type = "conditional"
command = "./scripts/classify-task.sh"
condition_pattern = "TYPE:BUG"
on_match = ["./scripts/gather-bug-context.sh"]
on_no_match = ["./scripts/gather-feature-context.sh"]

[[pipelines.steps]]
name = "enrich"
type = "once"
command = "./scripts/add-project-context.sh"
timeout = 15
```

Where `classify-task.sh` outputs `TYPE:BUG` or `TYPE:FEATURE` based on keywords.

## Cost Comparison

| Approach | Token Cost | Time | Reliability |
|----------|-----------|------|-------------|
| AI-only (no pipeline) | ~2000 tokens for context gathering | ~5s | Variable |
| Pipeline + AI | 0 tokens for context, ~500 for refinement | ~1s + ~2s | Deterministic context, AI reasoning |

Pipelines save tokens by doing deterministic work that doesn't need AI.

## Key Points

- Pipeline scripts run **before** workflows and the main AI
- Scripts can be written in **any language** (bash, python, ruby, etc.)
- Stdin/stdout piping between steps -- Unix philosophy
- Non-zero exit code = fatal stop (script errors are real problems)
- Use pipelines for: file detection, git context, validation, data transformation
- Use workflows for: reasoning, refinement, planning
- Combine both for the best results: deterministic preparation + AI reasoning
