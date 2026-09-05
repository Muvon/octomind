# Keep fixing a change until it passes review

Build a finite implementation, independent review, and correction loop that keeps working until the change passes.

## The problem

You ask an agent to implement a change, then discover missing cases during review and have to copy feedback into another
prompt. A second fix can introduce a new defect. You want implementation, independent reviews, and corrective passes
to follow an explicit route with a finite stopping point.

## What you will set up

- [Graph workflows](../usage/09-workflows.md) with conditional edges and `max_transitions`.
- [Separate roles](../usage/06-roles.md) for editing and reviewing the same files.
- [External MCP tools](../usage/07-mcp-tools.md) supplied by Octofs.
- [JSONL output](../usage/11-structured-output.md) for inspecting each verdict.

## Prerequisites

Use Bash on macOS or Linux. Install Python 3 and
[Octofs using its own installation guide](https://github.com/Muvon/octofs#installation).
Octofs supplies filesystem and shell tools; Octomind does not ship those as built-in tools.

```bash
bash --version
python3 --version
octofs --version
octofs mcp --help
octomind --version
octomind login
```

The login check should report `already signed in`; otherwise complete its browser flow. The workflow uses
`octohub:auto`. Run this exercise in a new directory: implementation and fix steps will write a Python module there.

## Steps

### 1. Create the exercise and identify the Octofs executable

The server's command will use a required environment variable. This resolves your installed executable without assuming
an installation path. Keep this terminal open for the remaining commands:

```bash
mkdir review-loop-demo
cd review-loop-demo
export TUTORIAL_OCTOFS_BIN="$(command -v octofs)"
test -x "$TUTORIAL_OCTOFS_BIN"
octomind config --show
```

### 2. Register the server and two roles

Save this complete configuration addition as `~/.local/share/octomind/config/tutorial-review-loop.toml` on macOS/Linux.
If you override the data or config path, save it beside your active base configuration instead.
The `mcp` argument is the [documented Octofs stdio entry point](https://github.com/Muvon/octofs#quick-start).

```toml
[[roles]]
name = "tutorial-implement"
system = """
Implement the supplied request in the current directory using the available
tools. Read existing files first. Change only slug.py. Never change tests.
Run python3 -m unittest -v test_slug.py and report its actual result.
Do not claim a successful check unless you ran it successfully.
"""
welcome = ""

[roles.mcp]
server_refs = ["tutorial-fs"]
allowed_tools = ["tutorial-fs:view", "tutorial-fs:text_editor", "tutorial-fs:shell"]

[[roles]]
name = "tutorial-review"
system = """
Review the actual current files against the supplied request. Use view to
read them. You cannot execute tests: distinguish source inspection from
reported test results. Follow the requested output format. For reviews,
return actionable issues, or exactly NO ISSUES. For evaluation, use PASS or FIX.
"""
welcome = ""

[roles.mcp]
server_refs = ["tutorial-fs"]
allowed_tools = ["tutorial-fs:view"]

[[mcp.servers]]
name = "tutorial-fs"
type = "stdio"
command = "{{ENV:TUTORIAL_OCTOFS_BIN}}"
args = ["mcp"]
timeout_seconds = 60
tools = ["view", "text_editor", "shell"]
```

### 3. Save the behavior checks

Save `test_slug.py` in `review-loop-demo/`. The implementation must lowercase ASCII letters, replace each run of
non-ASCII-alphanumeric characters with one hyphen, and remove leading and trailing hyphens.

```python
import unittest
from slug import slugify

class SlugTests(unittest.TestCase):
    def test_words(self):
        self.assertEqual(slugify("Hello World"), "hello-world")

    def test_punctuation(self):
        self.assertEqual(slugify("  Ship...It!! "), "ship-it")

    def test_empty(self):
        self.assertEqual(slugify(""), "")
        self.assertEqual(slugify(" !!! "), "")

    def test_digits_and_non_ascii(self):
        self.assertEqual(slugify("Release 42"), "release-42")
        self.assertEqual(slugify("café"), "caf")

if __name__ == "__main__":
    unittest.main()
```

### 4. Save the bounded graph

Save `review-loop.toml` in `review-loop-demo/`. Every leaf uses a fresh session. Reviewers read the current worktree
on every visit, so a fix is visible without relying on an earlier implementation summary.

```toml
name = "slug-review-loop"
entry = "implement"
max_transitions = 12
max_cost = 3.0

[[steps]]
name = "implement"
role = "tutorial-implement"
model = "octohub:auto"
timeout = 300
prompt = "Implement and test this request:\n{{input}}"

[[steps]]
name = "reviews"
parallel = true
min_success = 2
max_parallel = 2

[[steps.run]]
name = "correctness"
role = "tutorial-review"
model = "octohub:auto"
timeout = 300
prompt = "Read slug.py and test_slug.py. Review correctness against:\n{{input}}"

[[steps.run]]
name = "edge_cases"
role = "tutorial-review"
model = "octohub:auto"
timeout = 300
prompt = "Read slug.py and test_slug.py. Review boundary cases and unnecessary complexity:\n{{input}}"

[[steps]]
name = "verdict"
role = "tutorial-review"
model = "octohub:auto"
timeout = 300
prompt = """
Evaluate both reviews against the request. If both report NO ISSUES and
neither has an actionable concern, return exactly PASS with no other text.
Otherwise return FIX followed by every valid actionable issue. Never include
a standalone PASS line in a rejection.
Request: {{input}}
Reviews: {{reviews}}
"""

[[steps]]
name = "fix"
role = "tutorial-implement"
model = "octohub:auto"
timeout = 300
prompt = "Apply valid review issues and rerun the tests.\nRequest: {{input}}\nVerdict: {{verdict}}"

[[edges]]
from = "implement"
to = "reviews"

[[edges]]
from = "reviews"
to = "verdict"

[[edges]]
from = "verdict"
to = "$end"
when = { matches = "(?m)^PASS\\s*$" }

[[edges]]
from = "verdict"
to = "fix"

[[edges]]
from = "fix"
to = "reviews"
```

### 5. Validate the routes and inspect tool access

Edges from a node are tested in file order. The first match wins; the final unconditional edge is its required fallback.
Without `when.output`, matching examines the node that just completed, here `verdict`.

The template's `(?m)^PASS\s*$` matches a standalone `PASS` line anywhere in the answer, including in a longer rejection.
It is case-sensitive and does not understand the verdict's meaning. That is why the evaluator prompt forbids a `PASS`
line on rejection. If you specify both `contains` and `matches`, either test can select the edge; they are not ANDed.

Check that the plan shows graph mode, entry `implement`, the conditional exit, and the fallback to `fix`:

```bash
octomind config --validate
octomind workflow review-loop.toml --dry-run
octomind run tutorial-review
```

Inspect the active tools. For `tutorial-fs`, the reviewer should see only `view`; then exit the inspection session:

```text
/mcp full
/exit
```

### 6. Run the implementation and review cycle

The counter measures top-level node executions, including repeated visits. A successful initial pass costs three
visits: `implement`, `reviews`, `verdict`. Each fix cycle adds three more. The parallel reviewers count as one graph
node, though each makes its own model calls. Exhaustion exits with an error; it is not an accepted review.

Run from `review-loop-demo/` so every child sees the same files. The cost threshold is checked after completed work,
not as a strict cap on in-flight requests:

```bash
printf '%s\n' \
  'Create slug.py exporting slugify(text). Lowercase ASCII letters; replace runs outside a-z and 0-9' \
  'with one hyphen; trim edge hyphens. Use only the standard library. Preserve test_slug.py.' \
  | octomind workflow review-loop.toml --format jsonl > review.jsonl
```

## Verify it works

Run the independent checks. Expect four passing tests and `OK`; then inspect the last evaluator answer. Its exact
`PASS` check below is stricter than the workflow template's multiline regex.

```bash
python3 -m unittest -v test_slug.py
python3 - <<'PY'
import json
from pathlib import Path
events = [json.loads(line) for line in Path("review.jsonl").read_text().splitlines()]
verdicts = [e["content"] for e in events if e["type"] == "assistant" and e["step"] == "verdict"]
assert verdicts and verdicts[-1].strip() == "PASS", verdicts
assert events[-1]["type"] == "cost"
print(f"Accepted after {len(verdicts)} evaluation(s)")
PY
```

## Variations

- **Require a whole-answer verdict.** Replace the edge's pattern with `\APASS\s*\z`, encoded in a TOML literal string
  as `'\APASS\s*\z'`. Rust regex absolute anchors prevent a `PASS` line embedded in other prose from matching.
- **Allow one fix cycle.** Set `max_transitions` to `6`: three nodes for the initial pass and three for one correction.
  The workflow fails if its sixth node still routes to another fix.
- **Use your repository's checks.** Change the role, request, and reviewer prompts together, including the permitted
  edit scope and exact test command. Keep the reviewers' file access separate from the editor's tools.

## Troubleshooting

**The server cannot start.** Re-export `TUTORIAL_OCTOFS_BIN` in the terminal launching the workflow and check it with
`test -x "$TUTORIAL_OCTOFS_BIN"`. The `{{ENV:TUTORIAL_OCTOFS_BIN}}` placeholder must resolve to a nonempty executable path.

**The graph reaches its limit.** Read the saved verdicts and fix the recurring issue before rerunning. Increasing the
limit permits more work but does not establish progress. Files written before failure remain in the directory.

**The graph accepts a rejection containing PASS.** Use the whole-answer regex variation and inspect the full verdict.
Matching is textual; it cannot infer that surrounding prose contradicts the matched line.

**The agent says tests passed but your check fails.** Treat the local test command as the observable result. Inspect
`slug.py` and the tool output; do not equate a model verdict or a successful workflow exit with test execution.

## See also

- [Workflows](../usage/09-workflows.md)
- [Roles](../usage/06-roles.md)
- [MCP tools](../usage/07-mcp-tools.md)
- [Structured output](../usage/11-structured-output.md)
