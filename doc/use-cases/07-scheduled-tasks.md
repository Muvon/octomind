# Scheduled Tasks

Use the built-in `schedule` tool or `/schedule` command for reminders, periodic checks, and follow-up work in a running
session. This guide covers interactive scheduling, daemon operation, and resuming saved schedules.

## Get Started

Start an interactive session and add a reminder directly:

```bash
octomind run --name project-monitor
```

```text
/schedule add when="in 5m" message="Remind me to check the build" description="Build reminder"
/schedule
```

Interactive CLI roles always receive the `orchestration` server's `schedule` and `monitor` tools. Non-interactive, ACP,
and WebSocket sessions use their normal role grants; the template's `assistant` role includes `orchestration:*`.
`/schedule` dispatches directly to the scheduler, without requiring the AI to call a tool.

For the AI tool, `command="add"` requires `message`. Omitting both `when` and `every` defaults to a one-shot on the next
idle. A timed `every`, such as `"10m"`, also requires `when` to specify the first firing.

## Schedule Follow-Up Work

The following transcripts illustrate prompts and AI tool arguments, not literal shell commands. A scheduled message
enters the session inbox and is processed when the response loop can handle it; it does not interrupt an in-flight model
response. It retains schedule-source metadata and does not replace the supervisor's active human task.

### Basic: Fire on Next Idle (default)

The simplest schedule has no time at all — it fires as soon as the session goes idle (no running taps, detached shell
jobs, or background agent jobs):

```text
> When you're done with the current work, summarize everything we did today

AI calls: schedule(command="add",
          message="Summarize all changes made in this session today",
          description="Daily summary")

# 'when'/'every' omitted -> defaults to when="idle"
# Fires once the session becomes idle, then is removed
```

### Basic: Timed Reminders

```text
> Schedule a reminder in 30 minutes to check the CI build

AI calls: schedule(command="add", when="in 30m",
          message="Check the CI build status and report any failures",
          description="CI build check")

# 30 minutes later, the message fires automatically
# AI processes it: reads CI status, reports results
```

### Time-of-Day Scheduling

```text
> At 3pm, summarize everything we've done today

AI calls: schedule(command="add", when="3:00pm",
          message="Summarize all changes made in this session today",
          description="Daily summary")
```

If the time has already passed today, it fires tomorrow.

### Absolute Datetime

```text
> On March 30th at 10am, remind me to deploy the release

AI calls: schedule(command="add", when="2030-03-30 10:00",
          message="Time to deploy the release. Run the deployment checklist.",
          description="Release deployment")
```

### Repeat on Idle

Use `every="idle"` to fire on **every** idle, not just the first one:

```text
AI calls: schedule(command="add", every="idle",
          message="Remind me to commit; remove this schedule once I have committed.",
          description="Commit nudge")
# Re-fires each time the session returns to idle, until removed
```

Idle scheduling cannot be combined with a clock time: passing a time-based `when` (e.g. `"in 5m"`) together with
`every="idle"`, or a time-based `every` together with `when="idle"`, is rejected with an error. Use the idle keyword on
both fields, or omit the one you do not need.

### Recurring Checks

Use `every` for a native repeat instead of asking the model to create a new one-shot on each firing:

```json
{
  "command": "add",
  "when": "in 10m",
  "every": "10m",
  "message": "Check the CI build status. Report failures. If the build is finished, list schedules and remove the entry described as CI watch.",
  "description": "CI watch"
}
```

The role needs tools that can inspect your CI service. Scheduling supplies the follow-up message, not those tools.
Repeats keep a stable ID and continue until removed or changed to a one-shot. If processing falls behind, the next
firing skips missed intervals instead of issuing a catch-up burst. Include a stop condition in every recurring task.

For live process output, use the `monitor` tool instead of a timer; see [Event-Driven Agent](02-event-driven-agent.md).
Detached shell jobs already deliver completion through the inbox.

## Manage Schedules

Use IDs returned by `list`; the IDs in this transcript are examples to replace with your own.

```text
> What's scheduled?

AI calls: schedule(command="list")
# Shows all pending entries with IDs, trigger times, and countdown

> Cancel the deployment reminder

AI calls: schedule(command="remove", id="abc12345")

> Push the build check back to 20 minutes

AI calls: schedule(command="edit", id="def67890", when="in 20m")

> Stop the daily standup reminder from repeating, but keep its next firing

AI calls: schedule(command="edit", id="ghi13579", every="none")
# every="none" (or every="off") clears the interval, turning a recurring entry into a one-shot
```

### Direct Control: `/schedule` Slash Command

The same operations are available as a session command, so you can list, add, edit, and remove schedules without going
through the AI:

```text
/schedule
/schedule add message="summarize what we just did"
/schedule add when="in 5m" message="check the build"
/schedule add when="9am" message="standup" every="24h" description="daily"
/schedule add every="idle" message="Remind me to commit; remove this schedule once I have committed."
/schedule edit abc12345 when="in 1h"
/schedule edit abc12345 every="none"
/schedule remove abc12345
```

Multi-word values must be quoted (`when="in 1h 30m"`, `message='hello world'`). See [Session
Commands](../reference/02-session-commands.md) for the full reference.

## Keep a Daemon Running

Combine with daemon mode for long-running automated workflows. If you are following the quick start, exit the
interactive session before reusing its name:

```text
/exit
```

```bash
# Keep this process running; send follow-up messages from another terminal.
printf '%s\n' 'Wait for my monitoring instructions.' | \
  octomind run assistant --name project-monitor --daemon --format jsonl
```

Then send it a setup message over the session socket with `octomind send`:

```bash
octomind send --name project-monitor \
  'Schedule a reminder in 30 minutes, repeating every 30 minutes, to review project progress. Stop when I ask.'
```

The AI can add an entry with `when="in 30m"` and `every="30m"`. A `--daemon` session stays alive regardless of the
schedule queue, so it keeps firing entries indefinitely. See [Daemon and Hooks](../integration/03-daemon-and-hooks.md)
for how `octomind send` reaches a running session.

## Persistence and Common Questions

**Will schedules run after I close Octomind?** No. Schedule changes and firings write a best-effort `SCHEDULE_SNAPSHOT`
to the session's compressed `.jsonl.zst` log. Resuming restores the latest readable snapshot, but no work runs while the
process is stopped. Resume by name:

```bash
octomind run --resume project-monitor
```

**Why won't a piped run exit?** Without `--daemon`, a non-interactive run waits while schedules, monitors, tap runs,
background agents, or detached shell jobs remain. A recurring schedule can therefore keep it alive indefinitely.
Interactive sessions and daemon runs stay alive independently of the schedule queue. Remove unwanted repeats using the
ID from `/schedule` or `schedule(command="list")`.

**Why is `every="10m"` rejected?** Add `when="in 10m"` for the first firing. Do not combine a clock time with
`every="idle"`, or `when="idle"` with a timed interval. Use `24h` for a day; `1d`, `1w`, and `10min` are invalid.

**Why does an idle repeat keep prompting?** It can fire again after its own response settles; it is an autonomous loop,
not a timer. Remove it or use `edit ... every="none"` to leave only its next idle firing.

**Can I change a timed entry into an idle entry?** The current `edit` handler accepts clock times and numeric intervals,
but does not change trigger mode or accept `when="idle"`/`every="idle"`. Remove and re-add the entry:

```text
/schedule remove abc12345
/schedule add when="idle" message="Summarize the completed work"
```

**Which timezone is used?** The running process's local timezone. A time-of-day in the past moves to tomorrow; an
absolute datetime in the past is already due. Local times that cannot be resolved uniquely are rejected.

ACP and WebSocket also have schedule/inbox monitors that deliver timed and idle entries. Keep their session
connection/process alive; scheduling is not an external cron service.

## Time Format Reference

| Format | Example | Description |
|--------|---------|-------------|
| Idle | `idle` | Fires the next time the session is idle (no running taps, shell jobs, or agents) |
| Immediate | `now` | Fires on the next scheduler tick |
| Relative | `in 5m` | Minutes from now |
| Relative | `in 2h` | Hours from now |
| Relative | `in 1h30m` | Combined hours and minutes |
| Relative | `in 90s` | Seconds from now |
| Relative | `in 2h 30m 10s` | Full combination (spaces optional) |
| Time today | `9am` | Bare hour, 12-hour form (minutes/seconds default to 0) |
| Time today | `15:30` | 24-hour format (tomorrow if past) |
| Time today | `3:30pm` | 12-hour format (tomorrow if past) |
| Absolute | `2030-03-30 15:30` | Exact date and time |

Relative durations accept only **h** (hours), **m** (minutes), and **s** (seconds), in any combination (`90s`, `10m`,
`1h30m`, `2h 30m 10s`). There is no day or week unit — `1d` is invalid.

The `every` field for repeating entries uses the same `h`/`m`/`s` duration grammar (e.g. `every="10m"`,
`every="1h30m"`); it fires first at `when`, then repeats at that interval. `every="idle"` repeats on each idle instead.

Implementation: [schedule commands and delivery](../../src/mcp/orchestration/schedule/core.rs), [time parsing and
repeats](../../src/mcp/orchestration/schedule/storage.rs), and [slash-command
parsing](../../src/session/chat/session/commands/schedule.rs).

## See also

- [Session Commands](../reference/02-session-commands.md)
- [Daemon and Hooks](../integration/03-daemon-and-hooks.md)
- [Event-Driven Agent](02-event-driven-agent.md)
- [Long-Running Development](08-long-running-development.md)
