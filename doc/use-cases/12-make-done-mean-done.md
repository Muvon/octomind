# Check the result before accepting “done”

Set up completion review and executable checks so done means the requested change actually works.

## The problem

You ask an agent to fix a bug, receive a completion message, and then run the tests yourself to discover the bug is
still there. Longer tasks make this harder: the agent loses track of requirements or overlooks failures in a large
log. You need completion review tied to your request and a test runner that checks the actual files.

## What you will set up

- The [supervisor completion gate and plan manager](../usage/14-supervisor.md).
- [Supervisor condensation](../usage/14-supervisor.md) for oversized plain-text tool results.
- A [post-turn guardrail validator](../usage/18-guardrails.md) that runs the test suite.
- A [local role](../usage/06-roles.md) with an explicitly configured [MCP server](../usage/07-mcp-tools.md).

## Prerequisites

Use macOS or Linux, a Unix shell, and Python 3.11 or newer. Check the installed binary, login, and Python:

```bash
octomind --version
octomind login
python3 --version
```

Install Octofs following its [own installation instructions](https://github.com/muvon/octofs#installation).
It supplies the `shell` and `view` tools used here. Check the executable and keep its discovered path in the
environment; the MCP configuration uses `{{ENV:TUTORIAL_OCTOFS}}` to read this user-supplied value.

```bash
command -v octofs
octofs --version
export TUTORIAL_OCTOFS="$(command -v octofs)"
```

## Steps

### 1. Create a failing project

The application should join a first and last name with one space. Its initial implementation omits that space.
The test suite has both a full-name case and a single-name case. You will authorize the agent to change the
application while preserving these assertions.

```bash
mkdir "$HOME/octomind-done-demo"
cd "$HOME/octomind-done-demo"
mkdir -p .agents/validators
cat > names.py <<'EOF'
def full_name(first, last):
    return first + last
EOF
cat > test_names.py <<'EOF'
import unittest
from names import full_name

class NameTests(unittest.TestCase):
    def test_full_name(self):
        self.assertEqual(full_name("Ada", "Lovelace"), "Ada Lovelace")

    def test_single_name(self):
        self.assertEqual(full_name("Ada", ""), "Ada")

if __name__ == "__main__":
    unittest.main()
EOF
python3 -m unittest -v
```

### 2. Enable the three supervisor mechanics

The previous command must fail on `test_full_name`. Generate a separate configuration, retaining your normal login:

```bash
export OCTOMIND_CONFIG_PATH="$PWD/.octomind-config/config.toml"
octomind config --validate
```

Save `.octomind-config/90-completion.toml`. The complete tables below merge with the generated defaults.
This role uses only the installed Octofs server explicitly; no development tap or extra server installation is needed.

```toml
auto_capabilities = false

[supervisor]
enabled = true

[supervisor.model]
name = "octohub:auto"
reasoning_effort = "medium"
max_tokens = 8192
temperature = 0.0
top_p = 1.0
top_k = 0
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300

[supervisor.learning]
enabled = false

[supervisor.gate]
enabled = true

[supervisor.plan]
enabled = true

[supervisor.condense]
enabled = true
adaptive = false
tokens_threshold = 512

[[roles]]
name = "completion"
system = "Complete the requested change and report observed verification. Preserve test assertions."
welcome = "Completion exercise ready."

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

### 3. Add the independent post-turn test runner

The validator consumes its JSON payload, runs the actual test suite, and saves the exit code and output.
On failure it returns nonempty stdout and exits nonzero so Octomind can queue a `<validation>` message.
An unconditional validator avoids confusing an attempted shell call with a successful test run.

```bash
cat > .agents/validators/tests <<'EOF'
#!/usr/bin/env python3
from pathlib import Path
import json
import subprocess
import sys

json.load(sys.stdin)
run = subprocess.run([sys.executable, "-m", "unittest", "-v"], capture_output=True, text=True)
output = run.stdout + run.stderr
Path("validation.json").write_text(json.dumps({"exit_code": run.returncode, "output": output}, indent=2))
if run.returncode:
    print("The project test suite failed. Fix names.py without weakening test_names.py.")
    print(output or "The test process returned no output.")
    sys.exit(1)
EOF
chmod +x .agents/validators/tests
```

Save `.agents/guardrails.toml`. This validator is independent of `[skills].auto_validation` and runs after every
assistant turn for the exact `completion` role.

```toml
[[validator]]
name = "name-tests"
roles = ["completion"]
script = ".agents/validators/tests"
```

### 4. Prove the validator catches the initial bug

Call the executable directly with an empty JSON object; this particular script does not need any payload fields.
It should print the failure feedback and return status 1. The saved JSON should contain a nonzero `exit_code`.

```bash
printf '{}\n' | .agents/validators/tests
cat validation.json
python3 -c 'import tomllib; tomllib.load(open(".agents/guardrails.toml", "rb")); print("TOML OK")'
octomind config --validate
```

### 5. Give the agent an observable completion contract

Start the configured role, then inspect the tool surface. The supervisor's gate evaluates eligible completion claims
against your request and recorded evidence. It can request bounded rework or leave the turn unverified.
It does not promise that every model-written completion sentence implies a passing suite.

```bash
octomind run completion
```

Send the three prose lines below as one request. The shell tool can edit this file; no editing tool is required.
Use `/plan` after the response to inspect supervisor-owned state. A small repair can legitimately have no plan.
`/plan` displays the plan; it does not create one.

```text
/mcp full
Fix names.py so full_name joins nonempty names with one space. Preserve every assertion in test_names.py.
You may edit names.py and run python3 -m unittest -v. Run that command after the final edit.
Report its exit status and the two tested cases. Do not claim success from source inspection alone.
/plan
/info
```

### 6. Inspect the saved check before accepting completion

Wait for the turn to finish. The guardrail validator runs after the final assistant message and queues failure
feedback for a subsequent request; it does not synchronously convert the gate's verdict into a test-suite verdict.
If the saved result fails, ask the agent to address it explicitly.

The gate's mutation checks can also accept artifact read-back as evidence of content. Your explicit test requirement
and the validator establish the stronger runtime requirement for this exercise.

```text
Read validation.json with view. If its exit_code is nonzero, fix names.py and rerun python3 -m unittest -v.
Report the observed result and leave any remaining failure explicit.
/exit
```

### 7. Try a larger diagnostic result

Create a log with repeated status lines and one failure marker. This fixture exceeds the demonstration's
512-token condensation threshold while remaining below the default hard tool-result cap.

```bash
python3 - <<'PY'
from pathlib import Path
lines = [f"status {i}: unrelated background item completed" for i in range(600)]
lines.insert(300, "FAILURE MARKER: worker-17 missed its deadline")
Path("diagnostic.log").write_text("\n".join(lines) + "\n")
PY
octomind run completion
```

Ask for the full output so the main-session tool path has a candidate to condense. With a local reader available,
condensation can retain verbatim lines and attach a spill-file path for omitted material. It may keep everything;
a candidate is not a guarantee that output will be shortened.

```text
/loglevel debug
Use shell to run cat diagnostic.log. Identify the failure marker and report its worker name.
If the result is condensed and you need omitted context, use view on the supplied spill-file path.
/info
/loglevel info
/exit
```

## Verify it works

Run the suite yourself and inspect the validator's independent result. Expect both tests to pass and the saved
`exit_code` to equal `0`. A correct answer to the diagnostic request identifies `worker-17`.

```bash
python3 -m unittest -v
python3 -c 'import json; r=json.load(open("validation.json")); print(r["exit_code"]); print(r["output"])'
```

## Variations

- For dependent work, ask for an implementation, a compatibility update, and verification with separate acceptance
  conditions. The supervisor decides whether a plan is useful; inspect it with `/plan` between requests.
- Restore `tokens_threshold = 5000` for ordinary work, or enable `adaptive = true` in the existing condensation table.
- In CI, pipe the repair prompt into `octomind run completion --format plain`, then run `python3 -m unittest -v`
  as the next CI step. Use the test process's exit code as the acceptance signal.

## Troubleshooting

**The agent says done but validation fails.** Read `validation.json` and send its failure back as the next request.
Gate retries are bounded, and post-turn feedback does not retract the displayed answer. Keep the independent test.

**No plan appears.** Planning is exceptional and supervisor-owned. A two-test repair can be plan-free. Check
`/info` and the enabled supervisor tables rather than inventing a plan-creation slash command.

**Nothing is condensed.** Check the threshold, actual result size, and availability of `view`. Only eligible
plain-text results are narrowed. A failed condenser call or a decision to retain the whole result leaves it intact.

**The validator never runs.** Verify the role name, project working directory, and executable bit. Use
`/loglevel debug` for spawn errors. Guardrail parse failures discard authored rules; a successful main config
validation does not validate `.agents/guardrails.toml`.

## See also

- [Supervisor](../usage/14-supervisor.md)
- [Guardrails](../usage/18-guardrails.md)
- [Roles](../usage/06-roles.md)
- [MCP tools](../usage/07-mcp-tools.md)
