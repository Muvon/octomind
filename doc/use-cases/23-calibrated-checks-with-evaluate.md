# Calibrated checks with the evaluation model

Ask a calibrated yes/no about the agent's own work from the shell, a guardrail validator, or a pipe, and act on the
probability instead of parsing prose.

## The problem

"Is this done?", "is this change in scope?", "is this request too vague to start?" are judgment calls. Asking a chat
model returns text you have to parse and a confidence it made up. The supervisor already routes such calls to an
evaluation model at its [gate seams](../usage/14-supervisor.md#evaluation-gates); `octomind evaluate` gives you the
same model from your own scripts: one JSON state in, one probability per question out, in well under a second.

## What you will set up

- `octomind evaluate` run by hand on a request file, to learn the shape and see how the numbers move.
- A [`[[validator]]`](../usage/18-guardrails.md#validator--end-of-turn-scripts) that scores the agent's completion
  report against the working-tree diff and nudges when the report cites no observed verification or the diff carries
  changes the report does not mention.
- A [`[[pipe]]`](../usage/18-guardrails.md#pipe-guardrail-pre-model-input-transform) that flags an underspecified
  request before the model sees it.

## Prerequisites

A build whose `octomind --help` lists `evaluate`, plus `jq`, `git`, and Python 3 for the demo project. Set the
evaluation provider you hold a key for: `TYPESAFE_API_KEY` for `typesafe:jev-latest`, `CLOUDFLARE_API_KEY` and
`CLOUDFLARE_ACCOUNT_ID` for `cloudflare:typesafe/jev` (the generated default), or `OCTOHUB_API_KEY` for
`octohub:auto`. Either put it in config or pass it with `-m` on every call:

```toml
[supervisor.evaluate]
model = "typesafe:jev-latest"
```

```bash
octomind --version
command -v jq
octomind evaluate -m typesafe:jev-latest config-templates/evaluate.json
```

The last command must print three answers keyed `done`, `next`, and `risk`. Every step below uses the configured model;
add `-m` if you skipped the config change.

## Steps

### 1. Probe by hand

The request is `{"state": <any JSON>, "questions": {...}}`. Start with a done-check on a small state and watch the
probability follow the evidence:

```bash
cat > done.json <<'JSON'
{
  "state": {
    "task": "Fix the failing test parser::empty_after_trim in src/parser.rs without changing the public API",
    "diff": "-    if input.is_empty() {\n+    if input.trim().is_empty() {",
    "test_output": "test parser::empty_after_trim ... ok\n12 passed; 0 failed"
  },
  "questions": {
    "done": {
      "type": "noul",
      "instructions": "Is the task complete?",
      "criteria": {
        "true": "The named test passes and no public signature changed.",
        "false": "The test still fails, or a public signature changed."
      }
    }
  }
}
JSON
octomind evaluate done.json
octomind evaluate done.json | jq .done.noul
```

Observed with `jev-1.13.0`, changing only the state:

| State | `done.noul` |
|-------|-------------|
| Diff as above, test output `ok`, `12 passed; 0 failed` | 0.96 |
| Same diff, test output `FAILED`, `11 passed; 1 failed` | 0.18 |
| Diff adds a `trim: bool` parameter to `pub fn parse`, tests green | 0.06 |

The third row is the point: green tests are not enough, the criteria said the public API must not change, and the
model read the diff. On a terminal the output is pretty-printed; piped into `jq` it is one compact line.

### 2. Create a demo project

The same small repair used in [Make "Done" Mean Done](12-make-done-mean-done.md), in a git repository so the
validator has a diff to read:

```bash
mkdir "$HOME/octomind-evaluate-demo" && cd "$HOME/octomind-evaluate-demo"
git init -q
mkdir -p .agents/validators .agents/pipes
cat > names.py <<'PY'
def full_name(first, last):
    return first + last
PY
cat > test_names.py <<'PY'
import unittest
from names import full_name

class NameTests(unittest.TestCase):
    def test_full_name(self):
        self.assertEqual(full_name("Ada", "Lovelace"), "Ada Lovelace")

    def test_single_name(self):
        self.assertEqual(full_name("Ada", ""), "Ada")

if __name__ == "__main__":
    unittest.main()
PY
git add -A && git commit -qm "demo"
```

### 3. Score the completion report against the diff

The validator receives the assistant's final message on stdin, reads the working-tree diff, and asks two questions.
A report with no observed verification, or a diff with changes the report does not describe, exits nonzero with a
one-line nudge that Octomind pushes to the session inbox.

```bash
cat > .agents/validators/done-evidence <<'SH'
#!/usr/bin/env bash
# Nudge when a completion report cites no observed verification, or when the
# working-tree diff carries changes the report does not describe.
set -euo pipefail
payload=$(cat)
diff=$(git diff; git diff --cached)
[ -z "$diff" ] && exit 0
answers=$(jq -n --argjson p "$payload" --arg diff "$diff" '{
  state: { report: $p.assistant_text, diff: $diff },
  questions: {
    evidence:  { type: "noul", instructions: "Does the report cite an observed verification result (a command that was run and what it printed), not only source inspection?" },
    unrelated: { type: "noul", instructions: "Does the diff contain changes the report does not describe?" }
  }
}' | octomind evaluate)
status=0
if jq -e '.evidence.noul < 0.5' <<<"$answers" >/dev/null; then
  echo "Your report claims completion without an observed verification result. Run the project's checks and report the command and its output."
  status=1
fi
if jq -e '.unrelated.noul >= 0.7' <<<"$answers" >/dev/null; then
  echo "The working tree contains changes your report does not describe. Describe them or revert them."
  status=1
fi
exit $status
SH
chmod +x .agents/validators/done-evidence
```

Register it so it fires only on turns that claim completion:

```toml
# .agents/guardrails.toml
[[validator]]
name   = "done-evidence"
match  = "(?i)\\b(done|finished|completed|implemented)\\b"
script = ".agents/validators/done-evidence"
```

Prove it by hand. Apply the fix, then feed three reports through the same JSON payload Octomind sends:

```bash
cat > names.py <<'PY'
def full_name(first, last):
    return " ".join(p for p in (first, last) if p)
PY
report() { jq -n --arg t "$1" '{validator:"done-evidence",role:"developer",assistant_text:$t,triggered_by:[]}' | .agents/validators/done-evidence; echo "exit=$?"; }
report "Done. I changed full_name in names.py to join the non-empty parts with a space."
report "Done. full_name in names.py now joins the non-empty parts with a space. Ran python3 -m unittest -v: test_full_name ok, test_single_name ok, 2 tests OK."
echo "Rewritten intro paragraph." >> README.md
report "Done. full_name in names.py now joins the non-empty parts with a space. Ran python3 -m unittest -v: 2 tests OK."
rm README.md
```

Observed: the first report is nudged for missing verification (`exit=1`), the second passes (`exit=0`), the third
passes the evidence check but is nudged for the undescribed `README.md` change (`exit=1`).

### 4. Flag vague requests before the model sees them

A pipe receives the raw user message and its stdout replaces it. This one prepends a clarify-first instruction when the
request scores as underspecified and passes everything else through unchanged. It always exits 0, so it never blocks.

```bash
cat > .agents/pipes/clarify <<'SH'
#!/usr/bin/env bash
# Prepend a clarify-first instruction when the request scores as underspecified.
set -euo pipefail
message=$(cat)
vague=$(jq -n --arg m "$message" '{
  state: { request: $m },
  questions: { vague: { type: "noul", instructions: "Is the request too underspecified to implement without asking a clarifying question?" } }
}' | octomind evaluate | jq '.vague.noul')
if [ "$(jq -n "$vague >= 0.8")" = true ]; then
  printf 'This request scored %s underspecified. Ask one clarifying question before changing anything.\n\n' "$vague"
fi
printf '%s' "$message"
SH
chmod +x .agents/pipes/clarify
```

```toml
# append to .agents/guardrails.toml
[[pipe]]
name    = "clarify"
command = ".agents/pipes/clarify"
```

```bash
printf 'make it faster' | .agents/pipes/clarify; echo
printf 'fix the login bug' | .agents/pipes/clarify; echo
printf 'Fix names.py so full_name joins non-empty names with one space; keep every assertion in test_names.py' | .agents/pipes/clarify; echo
```

Observed: `make it faster` scores 0.92 and `fix the login bug` 0.95, both get the prefix; the third scores 0.41 and
passes through untouched.

### 5. Run a session

Reset the fix so the agent has work to do, then start a session in the demo directory:

```bash
git checkout -- names.py
octomind run
```

```text
fix the bug
```

The pipe prepends the clarify-first line, so expect a question back rather than an edit. Answer it:

```text
Fix names.py so full_name joins non-empty names with one space. Keep every assertion in test_names.py. Run python3 -m unittest -v after the edit and report the output.
```

When the reply claims completion, the validator scores it. A report that quotes the test run passes silently; one that
does not gets the nudge as a `<validation validator="done-evidence">` message on the next turn.

## Verify it works

`octomind evaluate done.json | jq .done.noul` prints a number near 0.96 and drops below 0.2 when you change the test
output to a failure. The three hand-fed reports in step 3 exit 1, 0, 1 in that order. The three messages in step 4
print with, with, and without the prefix.

## Variations

- **CI scope check.** In a pull-request job, put the PR body and `git diff origin/main...HEAD` in the state and ask
  `unrelated` and `done`; fail the job below your threshold. The compact single-line output makes `jq -e` the whole
  gate.
- **Destructive-command screening.** You do not need a script: `[supervisor.evaluate] authorizer = true` asks the
  same model whether each pending call is prohibited, destructive, or external. Probed by hand, `git push --force
  origin master` scored 0.92 destructive and 0.98 prohibited under a "never push" instruction, `rm -rf target/` 0.17
  (regenerable), `cargo test` 0.07.
- **Thresholds.** 0.5 and 0.7 above are starting points. Print the raw answers from your validator to stderr for a
  few turns, then move the cut to where your false positives stop.
- **Another provider for one call.** `octomind evaluate -m octohub:auto req.json` keeps config untouched.

## Troubleshooting

**The command exits nonzero with a provider error.** The key for the configured provider is missing or wrong. Check
the environment variable named in the [CLI reference](../reference/01-cli-reference.md#octomind-evaluate-file) or
pass `-m` for a provider you do hold a key for.

**`request has no questions`.** The `questions` map is empty or misspelled; nothing is sent.

**The validator never nudges.** It exits 0 on a clean tree: run `git diff` and confirm there is a diff. Check the
executable bit and that the assistant text matched the `match` regex. `/loglevel debug` shows spawn errors.

**The pipe blocked a message.** Only a nonzero exit blocks. The script above always exits 0; a nonzero exit means
`jq` or `octomind evaluate` failed, and the error text is on stderr in the rejection message.

## See also

- [CLI reference: `octomind evaluate`](../reference/01-cli-reference.md#octomind-evaluate-file)
- [Supervisor: evaluation gates](../usage/14-supervisor.md#evaluation-gates)
- [Guardrails](../usage/18-guardrails.md)
- [Make "Done" Mean Done](12-make-done-mean-done.md)
