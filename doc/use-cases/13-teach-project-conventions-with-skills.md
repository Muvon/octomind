# Apply project conventions without repeating them each session

Package project conventions as standing instructions, reusable skills, and executable checks that apply in every relevant session.

## The problem

You keep correcting the same changes: helper names use the wrong style, domain code imports transport libraries,
and the agent calls a plausible implementation finished. Repeating the rules in every prompt takes time and still
leaves gaps. You need standing project instructions, a skill that activates for relevant work, and an executable check.

## What you will set up

- [Project `AGENTS.md` instructions](../usage/03-configuration.md) loaded at session startup.
- A [skill with domain and content activation rules](../usage/15-skills.md).
- A [local tap](../integration/04-tap-system.md) containing the skill and its executable validator.
- [`/skill` inspection and toggling](../reference/02-session-commands.md).
- An explicitly configured [MCP server](../usage/07-mcp-tools.md) for reading files and running commands.

## Prerequisites

Use macOS or Linux, a Unix shell, and Python 3.11 or newer:

```bash
octomind --version
octomind login
python3 --version
```

Install Octofs using its [installation instructions](https://github.com/muvon/octofs#installation).
Check the executable and export its discovered path for the `{{ENV:TUTORIAL_OCTOFS}}` configuration value:

```bash
command -v octofs
octofs --version
export TUTORIAL_OCTOFS="$(command -v octofs)"
```

No remote skill pack is required. You author the local tap and its skill below.

## Steps

### 1. Create the project and its standing instructions

The fixture has a customer-label function that incorrectly preserves surrounding whitespace. The project's boundary
is simple: `domain.py` contains pure functions, and names use snake_case.

```bash
mkdir "$HOME/octomind-conventions-demo"
cd "$HOME/octomind-conventions-demo"
mkdir -p project-tap/skills/customer-domain
cat > domain.py <<'EOF'
def customer_label(customer_name):
    return customer_name
EOF
cat > AGENTS.md <<'EOF'
Keep domain.py independent of transport, database, and filesystem code.
Preserve the public customer_label(customer_name) function.
Change only domain.py for this exercise. Do not weaken check_conventions.py.
Run python3 check_conventions.py after your final edit and report its result.
EOF
export OCTOMIND_CONFIG_PATH="$PWD/.octomind-config/config.toml"
octomind config --validate
```

### 2. Write an executable definition of the conventions

This check inspects every function name and parameter, rejects imports in the domain module, and checks the customer
label behavior. It checks the resulting file, so a well-written final answer cannot satisfy it by itself.

The initial run must fail because `customer_label("  Ada  ")` returns surrounding whitespace.

```bash
cat > check_conventions.py <<'EOF'
import ast
from pathlib import Path
import re
import runpy
import sys

def check():
    tree = ast.parse(Path("domain.py").read_text())
    for node in ast.walk(tree):
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            raise ValueError("domain.py must remain import-free")
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            if not re.fullmatch(r"[a-z][a-z0-9_]*", node.name):
                raise ValueError("function names must use snake_case")
        if isinstance(node, ast.arg):
            if not re.fullmatch(r"[a-z][a-z0-9_]*", node.arg):
                raise ValueError("parameter names must use snake_case")
    namespace = runpy.run_path("domain.py")
    label = namespace["customer_label"]
    if label("  Ada  ") != "Ada":
        raise ValueError("customer_label must strip surrounding whitespace")
    if label("Ada Lovelace") != "Ada Lovelace":
        raise ValueError("customer_label must preserve internal spaces")

try:
    check()
except Exception as error:
    print(f"CONVENTIONS FAIL: {error}")
    sys.exit(1)
print("CONVENTIONS PASS")
EOF
python3 check_conventions.py
```

### 3. Author the skill and its activation rule

Save `project-tap/skills/customer-domain/SKILL.md` with the exact contents below.
The directory and frontmatter name must agree. Both `name` and `description` are required.

`domains: conventions` matches the plain role you will create. The two checks on one rule line are AND:
`domain.py` must exist and the new user request must contain the word `customer`. Separate rule lines would be OR.

```markdown
---
name: customer-domain
description: Implement customer domain functions with the project's naming and dependency conventions.
domains: conventions
rules:
  - file(domain.py) content(customer)
---
Keep domain.py import-free. Put transport and persistence work outside this module.
Use snake_case for every function name and parameter.
Preserve customer_label(customer_name); strip surrounding whitespace and keep internal spaces.
Read the current implementation before editing it.
Run python3 check_conventions.py after the final edit.
Report the observed result. Do not claim a check passed from reading its source.
```

Check that you saved a nonempty file at the discovery path:

```bash
test -s project-tap/skills/customer-domain/SKILL.md
```

### 4. Add the skill validator inside the tap

Save the script with the command below. Skill validators receive `assistant` as their first argument and the final
assistant text on stdin. This validator consumes that text, then checks the actual project.

Its nonzero exit plus nonempty stderr produces repair feedback. On success, it returns 0 and resets the skill's
failure counter. The saved `skill-validation.json` lets you observe which check actually ran.

```bash
cat > project-tap/skills/customer-domain/validate <<'EOF'
#!/usr/bin/env python3
from pathlib import Path
import json
import subprocess
import sys

sys.stdin.read()
run = subprocess.run([sys.executable, "check_conventions.py"], capture_output=True, text=True)
output = run.stdout + run.stderr
Path("skill-validation.json").write_text(
    json.dumps({"exit_code": run.returncode, "output": output}, indent=2)
)
if run.returncode:
    print(output or "The convention check failed without output.", file=sys.stderr)
    sys.exit(1)
EOF
chmod +x project-tap/skills/customer-domain/validate
printf 'Proposed change.\n' | project-tap/skills/customer-domain/validate assistant
```

### 5. Enable automatic activation and validation

Save `.octomind-config/90-conventions.toml`. This complete `[skills]` table enables both mechanisms with a
60-second script timeout and a three-failure cap per skill. The activation timeout field is reserved; the shown
file/content rule evaluates in-process.

```toml
auto_capabilities = false

[skills]
auto_activation = true
auto_validation = true
activation_timeout = 3
validation_timeout = 60
max_retries = 3

[[roles]]
name = "conventions"
system = "Follow project instructions and active skills. Make the requested change and verify the result."
welcome = "Convention exercise ready."

[roles.mcp]
server_refs = ["octofs"]
allowed_tools = ["octofs:shell", "octofs:view"]

[[mcp.servers]]
name = "octofs"
type = "stdio"
command = "{{ENV:TUTORIAL_OCTOFS}}"
args = ["mcp"]
timeout_seconds = 300
tools = []
```

Register the skill directory as a local tap. `tutorial/conventions` is the local registration you create here.
The validator runner currently searches taps; placing this same validator only under `.agents/skills` would not run it.

```bash
octomind tap tutorial/conventions ./project-tap
octomind config --validate
octomind run conventions
```

### 6. Check automatic activation and make the repair

Send the request as one message. It contains more than eight non-whitespace characters and the keyword `customer`,
so it can pass the intent gate and match the skill rule. The active skill's body joins the model context.

After the response, `/skill` should show `customer-domain` as active. Look for the convention-check tool output,
then inspect the saved validator result under “Verify it works.”

```text
/mcp full
Fix the customer label function so it strips surrounding whitespace and preserves internal spaces.
/skill
/exit
```

### 7. Check manual activation in a fresh session

Start the role again. Project `AGENTS.md` instructions load independently of skill activation.

```bash
octomind run conventions
```

Before sending a task, toggle the exact skill on, then list it. Manual activation bypasses domain and rule matching.
Typing the same exact toggle again turns it off. Listing with `/skill` does not toggle anything.

```text
/skill customer-domain
/skill
Explain the active naming and dependency rules, then run python3 check_conventions.py without editing files.
/exit
```

## Verify it works

Run the independent check and inspect the record written by the skill validator. Expect `CONVENTIONS PASS` and
a saved `exit_code` of `0`. The active entry in `/skill` proves loading; the JSON file proves the script ran.

```bash
python3 check_conventions.py
python3 -c 'import json; r=json.load(open("skill-validation.json")); print(r["exit_code"]); print(r["output"])'
```

## Variations

- For instruction-only skills, use `.agents/skills/customer-domain/SKILL.md` and omit the validator. Project skills
  support automatic activation; automatic validator lookup still requires a tap.
- Preload the registered skill with `OCTOMIND_SKILLS=customer-domain octomind run conventions`. This skips activation
  rules and uses exact comma-separated skill names.
- Replace the content check with `content(review)` to activate on review requests, or add a second rule line for
  another trigger. Keep `domains` aligned with the role's base name before `:`.

## Troubleshooting

**The skill is listed but never activates.** Check `domains`, the rule, and the directory/frontmatter name match.
Restart after adding the skill. Short acknowledgments such as “ok” do not pass automatic activation's intent gate.

**The validator never runs.** Keep `validate` inside the registered tap, enable `auto_validation`, and make the script
executable. `/loglevel debug` shows spawn and timeout errors; those errors do not themselves generate repair feedback.

**The agent stops receiving corrections.** After three recorded failures, this example's skill validator stops
retrying. Fix the underlying issue and start a new session. A nonzero exit with no captured output produces no feedback.

**A different skill body loads.** Taps win over project and global skill directories by frontmatter name. Use a unique
skill name and keep it identical to its directory. `allowed-tools` metadata would not grant MCP permissions.

## See also

- [Configuration and project instructions](../usage/03-configuration.md)
- [Skills](../usage/15-skills.md)
- [Tap system](../integration/04-tap-system.md)
- [Session commands](../reference/02-session-commands.md)
- [MCP tools](../usage/07-mcp-tools.md)
