# Carry project corrections into the next session

Store project corrections as durable memories that automatically guide related work in future sessions.

## The problem

You correct an agent's approach on Monday, start a new session on Friday, and get the same mistake again. The old
conversation contains the correction, but the new request does not. You need a durable record of the rule and a way
to see whether that record actually reaches the model when related work starts.

## What you will set up

- [Supervisor learning](../usage/13-learning.md) for grounded extraction and cross-session recall.
- The shared [supervisor model profile](../usage/14-supervisor.md) used by learning operations.
- [`/done` and `/learning`](../reference/02-session-commands.md) for extraction and record inspection.
- A separate [configuration directory and local role](../usage/03-configuration.md) for a repeatable exercise.

## Prerequisites

Use a Unix shell on macOS or Linux and Python 3.11 or newer. You need Octomind's installed binary and your existing
login. This exercise uses `octohub:auto` and does not require an external memory MCP server.

```bash
octomind --version
octomind login
python3 --version
```

An existing login reports that you are already signed in. Keep the terminal and the session open while extraction
finishes; `/done` schedules background work rather than waiting for every memory operation.

## Steps

### 1. Create a project with a stable identity

Scoped learning uses the working directory's basename and the role base before `:`. Use this same directory and role
for both sessions. Do not put the rule in `AGENTS.md` during this exercise: that would also supply it at startup and
make the recall experiment ambiguous.

```bash
mkdir "$HOME/octomind-learning-demo"
cd "$HOME/octomind-learning-demo"
export OCTOMIND_CONFIG_PATH="$PWD/.octomind-config/config.toml"
octomind config --validate
```

### 2. Enable learning with the shared supervisor profile

Save `.octomind-config/90-learning.toml`. These complete override tables merge with the generated configuration.
Learning has no separate `[supervisor.learning.model]` table: extraction, verification, and recall preparation use
`[supervisor.model]`. Missing profile fields can inherit from the main model, but this example spells them out.

Evolution stays disabled. Stored learning text remains recall material; this setup does not compile it into generated
skills or guardrails.

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
enabled = true

[supervisor.learning.evolution]
enabled = false

[[roles]]
name = "memory_demo"
system = "Answer the user's project questions. Distinguish stated rules, observed evidence, and assumptions."
welcome = "Project learning exercise ready."

[roles.mcp]
server_refs = ["core"]
allowed_tools = ["core:*"]
```

Validate and start the first session. The role is local and explicitly defined above; no development tap is required.

```bash
octomind config --validate
octomind run memory_demo
```

### 3. State the correction as a durable user rule

Send the first line as one message and wait for the reply. Then send the second line.
The rule names this repository so the extractor has evidence for project scope. Short lessons require a verbatim
real-user quote and a separate support verification; an agent's own advice does not become a user rule.

```text
In this repository, preserve public API names unless I explicitly request a rename. This is a standing project rule.
Apply that rule to a proposed cleanup of customer_label. Explain the constraint without editing files.
```

### 4. Finish the task and inspect extraction

Type `/done` after the response. It attempts task-boundary compression and schedules learning from a snapshot of
the original conversation. The learning trigger does not depend on compression succeeding.

Use `/learning list` again after the background work finishes. There may be no new record: extraction rejects
unsupported or non-reusable material. Do not interpret the command returning as proof that a lesson was stored.

```text
/done
/learning list
```

Find the new rule in the unfiltered list. The example below inspects row 1; substitute the current unfiltered row
number if your rule appears elsewhere. `show` reloads the unfiltered list, so filtered row numbers are not safe.
Look for the rule body, file path, scope, source session, and evidence handles.

```text
/learning list
/learning show 1
```

### 5. Locate the records on disk

After you have inspected the stored rule, leave the session. Waiting before exit matters: the in-process extraction
task needs the process to remain alive.

```text
/exit
```

On macOS and Linux, the default data root is `~/.local/share/octomind`. For this exercise, scoped records live under
`learning/octomind-learning-demo/memory_demo/`; global records live under `learning/_/`.
`OCTOMIND_DATA_DIR` overrides the data root, including learning and login/config state. The configuration-path
override used here changes configuration selection, not the learning root.

Run this command to print the actual directories and hot Markdown record paths without assuming generated filenames:

```bash
python3 - <<'PY'
import os
from pathlib import Path

root = Path(os.environ.get("OCTOMIND_DATA_DIR", str(Path.home() / ".local/share/octomind")))
scoped = root / "learning" / Path.cwd().name / "memory_demo"
global_rules = root / "learning" / "_"
for directory in (scoped, global_rules):
    print(directory)
    for record in sorted(directory.glob("*.md")):
        print(" ", record.name)
PY
```

### 6. Start a new session and observe recall

Start from the same directory with the same exported configuration path. Do not use a resume flag: the experiment
needs a new conversation. Stored scoped records can now be retrieved for the same project and role.

```bash
octomind run memory_demo
```

Enable debug logging before the first real request. Ask about the same work without repeating the rule itself.
Look for the actual final Active Memory Pack in the debug output and the public-API rule inside it.
The answer should preserve `customer_label` unless a rename is explicitly requested.

```text
/loglevel debug
I want to tidy customer_label and its callers. What constraint from our previous project discussion applies?
/loglevel info
```

### 7. Distinguish a stored record from a recalled record

`/learning list` shows storage. Debug output around a genuine request shows the pack selected for that request.
The first request considers global rules and hybrid scoped recall; later genuine user requests replace the pack,
and tool follow-ups reuse it.

The pack is bounded to 2,000 estimated tokens, with at most 512 for global rules. It is materialized around provider
requests and removed afterwards rather than appended permanently to conversation history. A relevant stored rule can
still be omitted because of ranking or available context.

Use a fresh related request to inspect selection again:

```text
/learning list
/loglevel debug
Review whether a cleanup of customer_label should change its exported name under the project's prior rule.
/loglevel info
/info
```

## Verify it works

In the second session, inspect the unfiltered record and trigger one more related request with debug logging:

```text
/learning list
/learning show 1
/loglevel debug
Which remembered project rule governs renaming customer_label during cleanup?
/loglevel info
```

Use the actual unfiltered index if it is not 1. Success has two observations: a stored rule with provenance, and that
rule in the final provider-bound memory pack. A plausible answer alone does not prove recall. If the pack omits the
rule, record that as an unsuccessful recall check even though storage succeeded.

## Variations

- For a user-wide preference, state explicitly that it applies across projects. The extractor decides scope
  conservatively; inspect `scope` rather than assuming the rule became global.
- Use an existing development role for real work. A tag such as `developer:general` uses `developer` as its learning
  directory component; switching from `memory_demo` does not carry this exercise's scoped records into that domain.
- Let eligible automatic compaction trigger extraction during longer sessions. Its minimum user-message threshold
  is internal; there is no tutorial setting that forces every conversation into a record.

## Troubleshooting

**No lesson appears after /done.** Keep the process open, wait for extraction, and list again. Confirm
`[supervisor.learning].enabled` is true and that the conversation contains an explicit reusable user rule.
The extractor and verifier may correctly reject a candidate; there is no guaranteed “save this sentence” operation.

**The lesson exists but the next session ignores it.** Check the working-directory basename and role base.
Enable debug before a fresh related request and inspect the actual pack. Recall is limited by relevance and context;
orientation records are working assumptions to verify, not established facts.

**The shown row is not the filtered match.** Run `/learning list` without a pattern and inspect the desired row's
current index. Background updates can change ordering. `show` and `delete` use the current unfiltered hot list.

**Older records are missing from the hot list.** Retention can move them under the scope's `.archive/` directory.
Cold records are retained on disk and considered through bounded lexical retrieval. The explicit
`/learning clear` command deletes the current role/project's hot and cold records; it leaves global records alone.

## See also

- [Cross-session learning](../usage/13-learning.md)
- [Supervisor](../usage/14-supervisor.md)
- [Session commands](../reference/02-session-commands.md)
- [Configuration](../usage/03-configuration.md)
