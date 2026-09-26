# Sessions

Start, resume, and inspect Octomind conversations from the terminal. This guide also covers piped runs, background
operation, saved settings, and entry points for ACP and WebSocket clients.

## Starting Sessions

```bash
# Default tag (assistant:concierge)
octomind run

# A tap agent from the registry, addressed as category:variant
octomind run developer:general

# An explicit local [[roles]] entry from the shipped config
octomind run assistant

# Named session
octomind run --name feature-auth

# Main-purpose model override
octomind run -m anthropic:claude-sonnet-4-6
```

With no argument, `octomind run` uses the configured default tag, `assistant:concierge`. Bare names such as `assistant`
refer to roles in the shipped config; `category:variant` names such as `developer:general` are tap agents resolved from
the registry.

## Resuming Sessions

```bash
# Resume by name
octomind run --resume feature-auth

# Resume most recent
octomind run --resume-recent

# Pick a recent session interactively; Escape starts a new one
octomind run --resume
```

`--name feature-auth` resumes that session if it already exists. Without an explicit tag, a resumed session keeps its
saved role; an explicit tag selects a different role. `--resume-recent` searches the current working directory's
sessions. Bare `--resume` requires an interactive terminal; scripts must provide a session name.

List saved sessions, 15 per page:

```text
/list
/list 2
```

Start a fresh session mid-conversation:

```text
/new
/new Auth Refactor
```

`/new Auth Refactor` gives the new session a display title. To retitle the current session without changing the name
used by `--resume` or `send`, use `/rename`; no argument clears the title:

```text
/rename Auth Refactor
/rename
```

## Output Formats

| Mode | Behavior |
|------|----------|
| Interactive (no `--format`, TTY) | Terminal session with colors, markdown, and animations |
| `--format plain` | Reads a prompt from stdin and runs non-interactively; piped stdin selects non-interactive rendering |
| `--format jsonl` | Runs non-interactively and always emits structured JSON Lines, regardless of TTY. Ideal for automation. |

Pipe a nonempty prompt, rather than passing it as a positional argument:

```bash
printf '%s\n' 'Explain the authentication module.' | octomind run developer:general --format plain
printf '%s\n' 'List the project entry points.' | octomind run --format jsonl > session-events.jsonl
```

Piped stdin also selects non-interactive operation without `--format`. JSONL is an event stream, not a single JSON
result. Plain-mode terminal detection uses stdin; it is not an unconditional color-stripping flag.

For schema-constrained final responses, see [Structured Output](11-structured-output.md) for `--schema` examples.

## Daemon Mode

Keep a session alive in the background so other processes can inject messages into it:

```bash
printf '%s\n' 'Wait for further instructions.' | \
  octomind run --name ci-watcher --daemon --format jsonl > ci-watcher.jsonl 2> ci-watcher.err &
```

Send messages to it with `octomind send`:

```bash
echo "Check build status" | octomind send --name ci-watcher
octomind send --name ci-watcher "Summarize your progress."
```

The shell's `&` backgrounds this process; `--daemon` keeps it listening after a turn. Wait for startup to finish before
sending. A successful `send` acknowledges message delivery, not completion of the model's work. Non-TTY stdin must
contain a prompt even in daemon mode; `/dev/null` produces an empty-input error.

With terminal stdin, `--daemon --format jsonl` can start without an initial prompt. `--daemon` without `--format` on a
terminal enters the interactive input path. For unattended use, prefer the explicit piped example above.

See [Daemon and Hooks](../integration/03-daemon-and-hooks.md) for webhook integration.

## Session Commands

Recognized slash commands dispatch to session handlers. Some, such as `/done`, `/run`, and `/prompt`, can invoke models.
Unknown commands are treated as ordinary user input. See the [Session Commands
Reference](../reference/02-session-commands.md) for detailed arguments.

| Surface | Commands and concrete examples |
|---------|--------------------------------|
| Lifecycle | `/help`, `/exit`, `/quit`, `/clear`, `/list 2`, `/new Auth Refactor`, `/rename Auth Refactor` |
| Monitoring | `/status`, `/info`, `/report`, `/loglevel debug` |
| Model and behavior | `/model octohub:auto`, `/role assistant`, `/effort high`, `/prompt` |
| Context and compression | `/done`, `/context tool`, `/context large` |
| Media and clipboard | `/image screenshot.png`, `/video demo.mp4`, `/copy all` |
| Tools and planning | `/mcp`, `/run`, `/plan`, `/skill`, `/schedule` |
| Account, learning, and viewing | `/usage`, `/login`, `/learning list`, `/share`, `/analyze` |

`/?` is registered for completion but is not dispatched as help; use `/help`.

`/status` is the activity dashboard: the default view is concise and active-only across agents, MCP background jobs, and
command monitors. Use `/status agents`, `/status monitors`, or `/status jobs` for the full category view. Status is
scoped to the current process and session.

For example, inspect available tools, the supervisor-managed plan, and background activity:

```text
/mcp
/plan
/status agents
/status monitors
/status jobs
```

`/run`, `/prompt`, `/skill`, and `/schedule` with no arguments list their available entries. `/plan` inspects the plan;
the supervisor owns plan updates. `/share` uploads the log for viewing. `/analyze` starts a localhost bridge for the
browser viewer without uploading the log through the share endpoint.

`/workflow` lists tap workflows; `/workflow <name> <input>` runs one — it shells out to `octomind workflow <name> --format jsonl` with the input on stdin and returns the final step's output. See [Workflows](09-workflows.md) for the file format and CLI examples.

## Cost Monitoring

Track token usage and spending:

```text
/info
/report
```

`/info` shows:

- Token counts (input, output, cached, reasoning)
- Cumulative session cost; `/report` supplies the detailed usage breakdown
- Estimated cache savings
- Per-tool, per-response, per-request (input), and per-compression token averages (each shown only when nonzero)
- Cache marker stats (system / tool / content markers, non-cached tokens)
- Compression statistics (when any compression has happened)
- Request/turn timings, learning usage, and available agent/supervisor statistics

Supervisor counters are process-global and reset on restart; concurrent ACP/WebSocket sessions can mix those counters.
They are separate from persisted session totals.

Edit these root keys before any table header to set USD thresholds (both default to `0.0`, disabled):

```toml
max_session_spending_threshold = 5.0
max_request_spending_threshold = 1.0
```

The session threshold asks a terminal user whether to continue and resets its checkpoint on acceptance; piped input and
ACP/WebSocket decline automatically. The request threshold stops further work for the current request. These checks use
recorded spend, so a provider call can cross a threshold before the next check.

## Human Time and Energy

`/report` also estimates what the session cost *you*: the timesheet hours it occupied and the mental energy it drained.
`/report day`, `/report week` and `/report month` sum it over your interactive CLI sessions since local midnight, Monday,
or the 1st of the month, per day and per project; add `here` to count only the current project.

```text
/report
/report day
/report week here
/report month
```

Per request, `human` is the time the request occupied you, `DHE` its energy in deep-hour equivalents (1 DHE is one hour
of pre-AI deep coding; the daily budget is 4 DHE), `lines` how many lines its run changed, and `read` how much of their
reading time you spent before your next message — `unread` for the last answer. The lines under the table split your
time into code review, dialog/behavior checks and waiting, and list any warning flags.

### Why two numbers

When an agent writes the code, your work moves from writing to specifying and reviewing, and time stops being a good
proxy for effort: an hour of reviewing agent output drains more than an hour of hand coding, and an hour of waiting on an
agent drains much less. Timesheets need the hours (**time**); your workload limit needs the drain (**energy**). A day can
log 8 hours while its energy reaches the daily budget by noon.

Self-reports are unreliable here — developers in a randomized trial were 19% slower with AI while believing they were
20% faster (METR 2025) — so both numbers are computed from the session log instead of asked for.

### How time is estimated

Each genuine user message starts a turn. The agent's run lasts until its last assistant or tool message before your next
input; slash commands and injected system messages are not turns.

1. **Turn estimate.** Reading the previous run's visible text at 238 words/min (Brysbaert 2019), reviewing its changed
   lines at 6.7 lines/min, about 400 lines/h (Cohen 2006), and typing your input at 52 words/min (Dhakal et al. 2018),
   plus `think_overhead_min`.
2. **Active time** before an input is the gap since the previous run finished, capped at `deliberation_factor` × the
   estimate: a longer gap means you were away or elsewhere, a short one (pasted input) stays short. The first input of
   a session is measured from when the session opened. After the last run, reading its text is added — already when
   you run `/report` right after the answer; its changed lines stay `unread` until your next message.
3. **Overlap.** Attention is single-threaded. Where active intervals of parallel sessions overlap, those minutes are
   split equally between the sessions.
4. **Wait.** Time with no active interval anywhere while a run is in progress counts as wait, but only for
   `attention_window_min` after your last input to that session. A long autonomous run costs you its launch and its
   review, not its duration. Time with neither is idle and not counted.

A session's time is its active plus wait minutes, and the day's total never counts a minute twice. Periods are computed
day by day, since the energy budget and the flags are daily. The project is the directory name in the session name
(`YYMMDD-<project>-HHMM-<id>`); a session started with a custom `--name` is its own project.

### Code review or behavior check

What you read is measured, not assumed. The time before your next message first covers reading the agent's text,
deciding and typing (with the `deliberation_factor` slack); only the time left over counts as reading the changed code,
up to the 400 lines/h it takes. A short reply after a big change is a **behavior check** — you trust the result, run it,
or read the summary — and shows a low `read`; a long one is a **code review** and shows up to 100%. You know which one
you did; the report shows both parts side by side, and nothing is flagged for choosing a behavior check.

### How energy is estimated

Code-reading minutes weigh `review_weight`; dialog, spec writing and behavior checks weigh 1; attended waiting weighs
`wait_weight`. Each switch between parallel sessions without a 10-minute break costs `switch_cost_dhe`.

Review weighs more than writing because validating output is vigilance work: detection drops within 15–30 minutes, and
the work is demanding and stressful rather than passive (Mackworth 1948; Warm, Parasuraman & Matthews 2008). Monitoring
an automated system is often harder than doing the task yourself (Bainbridge 1983). Generative AI moves effort from
producing to verifying (Lee et al. 2025), verification load partly explains the stress and fatigue that build up across
tasks with AI coding assistants (When Help Hurts, CHI 2026), and oversight of AI tools beyond one's capacity produces a
distinct mental fatigue ("AI brain fry", Bedard et al. 2026).

The 4 DHE budget follows the roughly 4 hours a day that elite performers sustain in deliberate practice (Ericsson,
Krampe & Tesch-Römer 1993). With the default weights, 1.5 h of spec writing plus 1.25 h of review reaches it.

### Flags

| Flag | Shown when | Basis |
|------|------------|-------|
| `rubber-stamp` (row) | A run changed 50+ lines and your next message came in under 30% of the time it takes just to read the agent's text and reply | Engagement with agent output drops as a task goes on (Catalan et al. 2026) |
| Deep block | Active intervals chained without a 10-minute break for more than 90 minutes | 60–90 minute review sessions (Cohen 2006); 10-minute breaks reset stress build-up (Microsoft WorkLab 2021) |
| Over budget | Energy exceeds 4 DHE | Ericsson, Krampe & Tesch-Römer 1993 |

A rubber-stamp is not a behavior check: it means a large change was approved before even its summary could have been
read, which is when fatigue ships bugs.

### Calibrate

The reading, review and typing speeds and the flag thresholds are fixed research values. The six `[timing]` keys (see
[Configuration Reference](../reference/03-config-reference.md#timing)) are starting guesses meant to be fitted per person:

1. For 5–10 working days, log time per task by hand with a timer switched at every task change, and record an
   end-of-day fatigue score (NASA-TLX, or a 1–10 rating).
2. Fit `think_overhead_min`, `deliberation_factor` and `attention_window_min` until the session times from `/report day`
   match the log; aim for a median per-task error within 15%.
3. Keep dialog and spec weight at 1 and fit `review_weight`, `wait_weight` and `switch_cost_dhe` against the fatigue
   scores.

### Limitations

- Work outside sessions — reading docs, reviewing in the IDE, meetings — is invisible.
- Changed lines are the rows of the agent's own edits as a line-id editor (octofs `text_editor`, `batch_edit`) reports
  them. Diffs the agent only reads, such as `git diff` in a shell, are not counted, and files written through `shell` or
  created whole are invisible.
- Reading and typing speeds are population averages; complex code reads slower than 400 lines/h.
- `/report day` counts only sessions a human drove from the interactive CLI. Tap runs, workflow steps, one-shot
  `octomind run` prompts and ACP/WebSocket sessions are left out because their prompts often come from another agent;
  that also leaves out a person typing in an ACP editor. Sessions last used before this marker existed are left out too.
- The energy weights are hypotheses, not measurements, and one number merges executive load (review, spec) with other
  kinds of fatigue.
- The estimate is meant for self-reporting and team norms. Used for per-minute surveillance, it would change behavior
  and stop measuring what it claims to.

### References

- Bainbridge, L. (1983). Ironies of automation. *Automatica*, 19(6), 775–779.
- Bedard, J., Kropp, M., Hsu, M., Karaman, O. T., Hawes, J., & Kellerman, G. R. (2026, March). When using AI leads to
  "brain fry". *Harvard Business Review*. <https://hbr.org/2026/03/when-using-ai-leads-to-brain-fry>
- Brysbaert, M. (2019). How many words do we read per minute? A review and meta-analysis of reading rate. *Journal of
  Memory and Language*, 109, 104047.
- Catalan, C. R., Dizon, L. M., Monderin, P. N., & Kuang, E. (2026). "I'm not reading all of that": Understanding
  software engineers' level of cognitive engagement with agentic coding assistants. CHI 2026 Workshop on Tools for
  Thought. <https://arxiv.org/abs/2603.14225>
- Cohen, J. (2006). *Best Kept Secrets of Peer Code Review*. SmartBear Software (Cisco case study).
- Dhakal, V., Feit, A. M., Kristensson, P. O., & Oulasvirta, A. (2018). Observations on typing from 136 million
  keystrokes. *CHI 2018*.
- Ericsson, K. A., Krampe, R. T., & Tesch-Römer, C. (1993). The role of deliberate practice in the acquisition of expert
  performance. *Psychological Review*, 100(3), 363–406.
- Lee, H.-P., et al. (2025). The impact of generative AI on critical thinking: Self-reported reductions in cognitive
  effort and confidence effects from a survey of knowledge workers. *CHI 2025*.
- Mackworth, N. H. (1948). The breakdown of vigilance during prolonged visual search. *Quarterly Journal of Experimental
  Psychology*, 1(1), 6–21.
- METR (2025). Measuring the impact of early-2025 AI on experienced open-source developer productivity.
  <https://arxiv.org/abs/2507.09089>
- Microsoft WorkLab (2021). Research proves your brain needs breaks.
- Warm, J. S., Parasuraman, R., & Matthews, G. (2008). Vigilance requires hard mental work and is stressful. *Human
  Factors*, 50(3), 433–441.
- When help hurts: Verification load and fatigue with AI coding assistants (2026). *CHI 2026*.
  <https://doi.org/10.1145/3772318.3791176>

## Adjust Model and Behavior

A few commands change runtime settings without touching your global config:

- `/model <provider:model>` switches the active model and **saves it into the session file**, so resuming restores it.
  It does not change your global config.
- `/effort <level>` sets the reasoning effort for the session (`low`, `medium`, `high`, `xhigh`, `max`) and also saves
  it to the session file. It mirrors the `reasoning_effort` config field and is ignored by non-thinking models. See
  [Configuration](03-configuration.md).
- `/loglevel <none|info|debug>` changes logging verbosity for the running session only. It is **never** saved to the
  session file or global config.

```text
/model octohub:auto
/effort high
/loglevel debug
```

## Multimodal (Vision)

Attach images for AI analysis:

```text
/image screenshot.png
Explain the error shown in this screenshot.
```

Use an existing image path. With an image copied to the system clipboard, attach it without a path:

```text
/image
Describe this image.
```

Supported image formats: PNG, JPEG, GIF, WebP. Images larger than 5 MiB are rejected, and images are automatically
resized to fit within 1568x1568.

Attach videos:

```text
/video demo.mp4
Summarize the actions in this video.
```

Use an existing video path. `/video` requires a path; no argument attaches nothing after the model capability check.
Supported video formats: mp4, mov, avi, webm, mkv, m4v, 3gp. Videos larger than 100 MiB are rejected. Interactive
`Ctrl+V` can also attach a copied video file or clipboard image.

Attachments are queued onto your **next** message rather than sent immediately, and vision/video support depends on the
active model. Use `/model` to check or switch to a vision-capable model.

## Context Management

As sessions grow, manage context to control costs:

| Command | Effect |
|---------|--------|
| `/done` | Force context compression; start lesson extraction when learning is enabled |
| `/context` | View current context (same as `/context all`) |
| `/context all` | Show all messages |
| `/context assistant` | Show only assistant messages |
| `/context user` | Show only user messages |
| `/context tool` | Show only tool messages |
| `/context system` | Show only system messages |
| `/context large` | Show messages whose content exceeds 1000 UTF-8 bytes |

An unrecognized filter silently falls back to showing all messages.

Automatic compression also runs as sessions grow. See [Compression](08-compression.md).

```text
/context large
/done
/learning list
```

`/done` bypasses automatic compression thresholds and keeps the session open. Lesson extraction uses the pre-compression
transcript and runs asynchronously; the learning list may not update immediately.

## Project Instructions

See [Configuration](03-configuration.md#project-instructions-and-template-variables) for the `AGENTS.md` loader and a
project-instruction example.

## Connect an ACP or WebSocket Client

Configure an ACP client to launch this command over stdio:

```bash
octomind acp developer:general --name editor-session
```

ACP creates sessions when the client requests them. See [ACP Protocol](../integration/02-acp-protocol.md) for the client
lifecycle. To start a WebSocket server for a client:

```bash
octomind server developer:general --host 127.0.0.1 --port 8080
```

Browser clients must have their exact origin allowed, for example:

```bash
octomind server --allow-origin http://localhost:3000
```

See [WebSocket Server](../integration/01-websocket-server.md) for session and message JSON payloads. Command
availability and lifecycle effects depend on the transport; this guide's interactive transcripts target the CLI.

## Session Storage

Sessions are stored in `~/.local/share/octomind/sessions/` (on Windows, `%LOCALAPPDATA%\octomind\sessions\`). Each
session is an append-only, zstd-compressed JSONL log file named `<session_name>.jsonl.zst`. Every line is an independent
zstd frame recording conversation messages, tool calls, cost and token snapshots, and compression markers — so the file
grows as the session continues rather than being rewritten.

`OCTOMIND_DATA_DIR` changes the parent data directory. Runtime processes, monitors, and MCP connections must be
initialized again on restart; saving a conversation does not preserve running processes.

Auto-generated session names follow the pattern `YYMMDD-<project-basename>-HHMM-<uuid>`, where `<uuid>` is the first 4
characters of a UUID. A `--name` you pass replaces this generated name.

To inspect a saved session, resume it and open the local viewer:

```bash
octomind run --resume feature-auth
```

```text
/analyze
```

If you have `zstd` installed, decompress the log before passing it to text tools (Unix default path):

```bash
zstd -dc ~/.local/share/octomind/sessions/feature-auth.jsonl.zst
```

## Troubleshooting

**Why did the response stop?** Check `/info` for recorded spend and `/status` for active work. To cancel a current
operation, press `Ctrl+C`; to exit interactive input, use `/exit` or `Ctrl+D`.

**Why does `send` say no running session?** A saved session file is insufficient. Start or resume the named session
first, wait for initialization, and send from the same machine/user runtime environment.

**Why is an attachment rejected?** Check the path, size, and active model's image/video support. Attach first, then send
your text; attaching alone does not request an answer.

## See also

- [Roles](06-roles.md)
- [Session Commands Reference](../reference/02-session-commands.md)
- [Compression](08-compression.md)
- [Daemon and Hooks](../integration/03-daemon-and-hooks.md)
