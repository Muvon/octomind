# Block destructive commands and check the agent's work

Set up sandbox boundaries, command guardrails, and verification so an autonomous agent cannot damage files or skip checks.

## The problem

You let an agent work in a repository, then discover it deleted a directory, wrote into your home directory, or
finished without checking the result. Instructions alone leave you reviewing every tool call. You need a filesystem
boundary, explicit denials for known commands, and a test you can inspect after the turn.

## What you will set up

- The [`--sandbox` filesystem boundary](../reference/01-cli-reference.md) for Octomind and its child processes.
- [Guardrails](../usage/18-guardrails.md) for pre-call denial, history conditions, result hooks, and turn validators.
- A [local tap agent](../integration/04-tap-system.md) with the existing shell and file-reading capabilities.
- A project [local tool](../usage/17-local-tools.md) that probes an outside write without overwriting anything.

## Prerequisites

Use macOS or Linux and a shell with standard Unix utilities. Linux needs working Landlock support; the startup
message and the write probe below establish whether restrictions actually apply on your machine.

```bash
uname -s
uname -r
octomind --version
octomind login
python3 --version
git --version
```

An existing login reports that you are already signed in. Python 3.11 or newer is required for the TOML check.
Git is needed for the baseline tap's first download. Install Octofs using its
[installation instructions](https://github.com/muvon/octofs#installation), then check the executable:

```bash
command -v octofs
octofs --version
```

## Steps

### 1. Create a disposable project and its configuration

Use a new directory directly under your home directory, outside the sandbox's temporary-directory exceptions.
Keep this shell open: `OCTOMIND_CONFIG_PATH` selects a separate configuration directory while your existing login
stays in the normal data directory. The first validation generates the complete default configuration.

```bash
mkdir "$HOME/octomind-safety-demo"
cd "$HOME/octomind-safety-demo"
mkdir -p .agents/tools .agents/hooks .agents/validators scripts disposable safety-tap/agents/safety
export OCTOMIND_CONFIG_PATH="$PWD/.octomind-config/config.toml"
octomind config --validate
printf '42\n' > answer.txt
printf 'keep this\n' > disposable/keep.txt
```

### 2. Give a local agent the verified capabilities

You create `tutorial/safety` here; it is a local tap, not a published package. Save this entire manifest as
`safety-tap/agents/safety/check.toml`. The resolver uses `safety:check` as the runtime role name.

```toml
capabilities = ["octomind/shell", "octomind/filesystem-read"]

[[roles]]
name = "safety"
system = "Follow the user's exact probe sequence. Report tool errors accurately and do not bypass denied calls."
welcome = "Safety checks ready."
```

Save `.octomind-config/90-safety.toml` with the following content. These complete override tables merge with
the generated defaults. The named providers belong to the baseline tap and use the installed Octofs executable.

```toml
auto_capabilities = false

[capabilities]
shell = "octofs"
filesystem-read = "octofs"

[supervisor.learning]
enabled = false
```

Register your local directory and validate the merged settings:

```bash
octomind tap tutorial/safety ./safety-tap
octomind config --validate
```

### 3. Install the test and the outside-write probe

The test checks an actual file. The probe attempts to create one new sibling file using exclusive creation, so it
cannot overwrite an existing file. Both scripts are yours; `probe_write` is a project tool, not a shipped MCP tool.

```bash
cat > scripts/check.py <<'EOF'
from pathlib import Path
import sys

ok = Path("answer.txt").read_text().strip() == "42"
print("TESTS PASS" if ok else "TESTS FAIL: answer.txt must contain 42")
sys.exit(0 if ok else 1)
EOF
cat > .agents/tools/probe_write <<'EOF'
#!/usr/bin/env python3
# @description Attempt one exclusive write beside the project to check the OS sandbox.

from pathlib import Path

target = Path.cwd().parent() / "octomind-safety-probe.txt"
try:
    with target.open("x") as stream:
        stream.write("sandbox probe\n")
except PermissionError:
    print("WRITE BLOCKED")
except FileExistsError:
    print("INCONCLUSIVE: the probe file already exists")
else:
    print("WRITE ALLOWED: the expected boundary is absent")
    target.unlink()
EOF
chmod +x .agents/tools/probe_write
python3 scripts/check.py
```

### 4. Save the hook and validator scripts

The hook records tool names, capability ownership, and success in `hook-events.jsonl`. An error also produces
nonempty stdout and exits 1, which queues feedback. The validator runs your test and saves its output independently
of the model's answer. These scripts receive JSON on stdin and run with the project as their working directory.

```bash
cat > .agents/hooks/record-result <<'EOF'
#!/usr/bin/env python3
import json
import sys

event = json.load(sys.stdin)
with open("hook-events.jsonl", "a") as stream:
    stream.write(json.dumps({k: event[k] for k in ("tool", "capability", "success")}) + "\n")
if not event["success"]:
    print("The last tool failed. Resolve the failure or state what remains blocked.")
    sys.exit(1)
EOF
cat > .agents/validators/tests <<'EOF'
#!/usr/bin/env python3
from pathlib import Path
import json
import subprocess
import sys

json.load(sys.stdin)
result = subprocess.run([sys.executable, "scripts/check.py"], capture_output=True, text=True)
output = result.stdout + result.stderr
Path("validator-result.txt").write_text(output)
if result.returncode:
    print(output or "Test process failed without output.")
    sys.exit(1)
EOF
chmod +x .agents/hooks/record-result .agents/validators/tests
```

### 5. Declare the rules

Save `.agents/guardrails.toml`. The first guard denies a specific destructive command shape. The second uses a
harmless marker to demonstrate a conditional rule: it blocks until an allowed `pwd` shell call enters history.
`has` names an MCP server; `match` and `when` name capabilities.

History records allowed attempts before execution, including attempts that later fail. It cannot establish that
tests passed. These regular expressions cover the shown command forms; they are not a shell-language security policy.

```toml
[[guard]]
match = 'shell(command=^rm\s+-rf?\b)'
message = "Recursive deletion is denied in this project."

[[guard]]
match = 'shell(command=^printf POLICY_OK$)'
has = "octofs"
when = ['-shell(command=^pwd$)']
message = "Run pwd before the policy marker."

[[hook]]
on = "any"
script = ".agents/hooks/record-result"

[[validator]]
name = "project-tests"
roles = ["safety:check"]
script = ".agents/validators/tests"
```

### 6. Check parsing, then start the sandbox

The Python command checks TOML syntax. Guardrails have their own loader; `octomind config --validate` does not
validate this project file. Watch startup for guardrail parse errors as well as the sandbox status.

Linux may report fully enforced, partially enforced, or not enforced. macOS reports Seatbelt when initialization
succeeds. An unenforced sandbox is not a successful setup.

```bash
python3 -c 'import tomllib; tomllib.load(open(".agents/guardrails.toml", "rb")); print("TOML OK")'
octomind run safety:check --sandbox
```

### 7. Exercise each boundary

Type these messages separately, waiting for each response. Inspect `/mcp full` for `shell`, `view`, and `probe_write`.
The deletion probe targets only the disposable fixture. A refusal without a tool call does not prove the guard fired:
look for the synthetic error containing `[guardrail] Recursive deletion is denied in this project.`

The marker should first receive `Run pwd before the policy marker.`, then succeed after `pwd`. If the agent executes
`pwd` early, restart and repeat the marker probe first. The final local-tool result should be `WRITE BLOCKED`.
The intentional Python failure exercises the error hook; it should leave a `success: false` row in the event log.

```text
/mcp full
Call shell with command exactly "printf POLICY_OK". Do not call pwd or work around a denial.
Call shell with command exactly "pwd", then call shell with command exactly "printf POLICY_OK".
Call shell with command exactly "rm -rf ./disposable". Do not substitute another deletion method.
Call shell with command "python3 -c 'raise SystemExit(1)'". Report the intentional failure; do not retry.
Call probe_write once and report its result. Do not attempt another outside write.
/exit
```

## Verify it works

Check the files from your ordinary shell. Look for `keep this`, `TESTS PASS`, and a hook row whose `tool` is `shell`
and whose `capability` is `shell`. The session's `WRITE BLOCKED` result supplies the OS-boundary observation.
Hooks do not run for denied calls, so the absence of a deletion row alone is not proof of a denial.

```bash
cat disposable/keep.txt
cat validator-result.txt
cat hook-events.jsonl
python3 scripts/check.py
```

The OS boundary has exceptions. Both backends allow writes under `~/.local/share`; macOS also permits device and
temporary paths. Linux keeps reads unrestricted; macOS additionally restricts several credential directories.
Network access remains available. Hooks and validators queue feedback after work; they neither undo writes nor
guarantee a retry before exit. Keep an independent test command as your final acceptance check.

## Variations

- Add `when = ["+shell"]` to the validator to run it only after recorded shell activity since its previous run.
- Change the hook to `on = "error"` when you want feedback only for failed tool results and no success log.
- For another project, replace `scripts/check.py` with your test runner inside the executable validator wrapper.

## Troubleshooting

**The guard never fires.** Check the hook's capability value. Ownership comes from installed capability provider
files, not from the tool name alone. Confirm the baseline shell provider owns `octofs:shell`, and restart after
changing rules or provider selection. A tool without capability ownership cannot match a guard target.

**The sandbox probe allows the write.** Read the startup status and check that the project is directly under your
home directory, not inside an allowed temporary or data directory. Do not treat unsupported or unenforced Landlock
as protection. The probe removes only the file it successfully created.

**The validator leaves no result file.** Check the executable bit and start from the project root. Raise logging with
`/loglevel debug` to inspect spawn failures. Script paths name executables, not shell strings with arguments.

**A failing test does not change the answer.** Validator feedback requires nonzero exit plus nonempty stdout and
arrives through the inbox. Send a new request to address the failure and rerun the independent test. Validators have
a 300-second timeout; a timeout does not itself inject repair feedback.

## See also

- [Guardrails](../usage/18-guardrails.md)
- [CLI reference](../reference/01-cli-reference.md)
- [Tap system](../integration/04-tap-system.md)
- [Local tools](../usage/17-local-tools.md)
