# Carry project corrections into the next session

Store project corrections as durable lessons that the Besydr plugin automatically adds to related work in future Octomind
coding agent sessions. A new configuration lists Besydr by default.

## The problem

You correct an agent's approach on Monday, start a new session on Friday, and get the same mistake again. The old
conversation contains the correction, but the new request does not. You need a durable record of the rule and a way
to see whether that record actually reaches the model when related work starts.

## What you will set up

- [Cross-session learning](../usage/13-learning.md), which the Besydr plugin provides: extraction of lessons, storage and
  recall. See [Besydr's learning page](https://besydr.com/docs/learning).
- [`/done` and `/learning`](../reference/02-session-commands.md) for extraction and record inspection. `/learning`
  exists while the plugin is connected.
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

Scoped lessons use the repository root's folder name (the working directory's own name outside a repository) and
the role name. Use this same directory and role
for both sessions. Do not put the rule in `AGENTS.md` during this exercise: that would also supply it at startup and
make the recall experiment ambiguous.

```bash
mkdir "$HOME/octomind-learning-demo"
cd "$HOME/octomind-learning-demo"
export OCTOMIND_CONFIG_PATH="$PWD/.octomind-config/config.toml"
octomind config --validate
```

### 2. Check that Besydr is connected and learning is on

Octomind no longer learns by itself; the Besydr plugin does. A new configuration lists it, so nothing here switches it
on. Check that the generated configuration lists the plugin and that its entry has no `enabled = false`:

```bash
grep besydr .octomind-config/config.toml
```

When the `besydr` program is not installed, Octomind downloads it on first use. Besydr reads the OctoHub key that
`octomind login` stores. Learning is on by default in Besydr's own configuration (`[learning] enabled = true`), and its
generated rules (`[learning.evolution]`) are off by default; see
[Besydr's configuration](https://besydr.com/docs/configuration). Lessons are not compiled into generated skills or
guardrails in this exercise.

Save `.octomind-config/90-learning.toml`. These complete override tables merge with the generated configuration.

```toml
auto_capabilities = false

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

Type `/done` after the response. Octomind compacts the conversation and tells the Besydr plugin before it folds anything
away; Besydr then starts extracting lessons in the background.

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

After you have inspected the stored rule, leave the session. Wait for extraction to finish first: Besydr also extracts
when a session ends normally, but closing the terminal abruptly can end the client before it reports the end.

```text
/exit
```

On macOS and Linux, the default Besydr data directory is `~/.local/share/besydr`. For this exercise, scoped lessons live
under `learning/octomind-learning-demo/memory_demo/`; global lessons live under `learning/_/`.
`BESYDR_DATA_DIR` moves Besydr's data directory. `OCTOMIND_DATA_DIR` and the configuration-path override used here do
not move it.

Run this command to print the actual directories and hot Markdown record paths without assuming generated filenames:

```bash
python3 - <<'PY'
import os
from pathlib import Path

root = Path(os.environ.get("BESYDR_DATA_DIR", str(Path.home() / ".local/share/besydr")))
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

Ask about the same work without repeating the rule itself. The answer should preserve `customer_label` unless a rename
is explicitly requested. Then check `/info`: with Besydr connected it lists extra rows with what Besydr did in this
session.

```text
I want to tidy customer_label and its callers. What constraint from our previous project discussion applies?
/info
```

### 7. Distinguish a stored record from a recalled record

`/learning list` shows storage. Recall is separate: before each of your requests, Besydr chooses lessons and adds them
to the agent's context. The first request of a session ranks lessons by keyword and by meaning; later requests use
meaning alone. Each new set replaces the one before it.

The set is at most 2,000 estimated tokens, of which global lessons may use at most 512. Whole lessons are admitted,
never cut, so a relevant stored lesson can still be left out because of ranking or room.

Use a fresh related request to look at selection again:

```text
/learning list
Review whether a cleanup of customer_label should change its exported name under the project's prior rule.
/info
```

## Verify it works

In the second session, inspect the unfiltered record and trigger one more related request:

```text
/learning list
/learning show 1
Which remembered project rule governs renaming customer_label during cleanup?
```

Use the actual unfiltered index if it is not 1. A stored rule with provenance shows that storage worked. An answer that
keeps `customer_label` is consistent with recall but does not prove it: the model may reach the same answer unaided.
A lesson that Besydr leaves out for lack of room or relevance counts as an unsuccessful recall check, even though
storage succeeded.

## Variations

- For a user-wide preference, state explicitly that it applies across projects. The extractor decides scope
  conservatively; inspect `scope` rather than assuming the rule became global.
- Use an existing development role for real work. The `developer` agent uses `developer` as its lesson
  folder; switching from `memory_demo` does not carry this exercise's scoped lessons into that folder.
- Let a compaction that the agent chooses trigger extraction during longer sessions. Besydr extracts after the fold
  succeeds; there is no setting that forces every conversation into a lesson.

## Troubleshooting

**No lesson appears after /done.** Keep the process open, wait for extraction, and list again. Confirm the `besydr`
plugin entry is not disabled, that `[learning] enabled` is true in Besydr's configuration, and that the conversation
contains an explicit reusable user rule.
The extractor and verifier may correctly reject a candidate; there is no guaranteed “save this sentence” operation.

**The lesson exists but the next session ignores it.** Check the project key (the folder that holds `.git`) and the role
base. Recall is limited by relevance and room; project facts are recalled as assumptions to verify, not as established
facts.

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
