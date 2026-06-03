# Use Case: Scheduled Tasks and Timed Automation

Use the built-in `schedule` tool to have the AI perform actions at specific times during a session -- reminders, periodic checks, timed workflows.

## The Problem

During long development sessions, you need the AI to do things at specific times: check build status in 10 minutes, remind you about a meeting, run a test suite after lunch. External cron jobs are overkill. You want scheduling inside the session.

## Solution

The `schedule` MCP tool lets the AI (or you) schedule messages that fire at specific times -- or the next time the session is idle -- and get processed automatically.

Only `message` is required when adding an entry. Both `when` and `every` are optional: if you omit both, the entry defaults to `when="idle"` (a one-shot that fires the next time the session is idle).

### Basic: Fire on Next Idle (default)

The simplest schedule has no time at all -- it fires as soon as the session goes idle (no running taps, no background jobs):

```
> When you're done with the current work, summarize everything we did today

AI calls: schedule(command="add",
          message="Summarize all changes made in this session today",
          description="Daily summary")

# 'when'/'every' omitted -> defaults to when="idle"
# Fires once the session becomes idle, then is removed
```

### Basic: Timed Reminders

```
> Schedule a reminder in 30 minutes to check the CI build

AI calls: schedule(command="add", when="in 30m",
          message="Check the CI build status and report any failures",
          description="CI build check")

# 30 minutes later, the message fires automatically
# AI processes it: reads CI status, reports results
```

### Time-of-Day Scheduling

```
> At 3pm, summarize everything we've done today

AI calls: schedule(command="add", when="3:00pm",
          message="Summarize all changes made in this session today",
          description="Daily summary")
```

If the time has already passed today, it fires tomorrow.

### Absolute Datetime

```
> On March 30th at 10am, remind me to deploy the release

AI calls: schedule(command="add", when="2026-03-30 10:00",
          message="Time to deploy the release. Run the deployment checklist.",
          description="Release deployment")
```

### Idle Scheduling

Instead of a clock time, you can fire a message the next time the session becomes **idle** -- meaning there are no running tap-runs and no running background agent jobs. This is useful for "do this once the current work settles" tasks.

```
> Once you're idle, run the linter and report any issues

AI calls: schedule(command="add", when="idle",
          message="Run the linter and report any issues found",
          description="Lint on idle")
```

Use `every="idle"` to fire on **every** idle, not just the first one:

```
AI calls: schedule(command="add", every="idle",
          message="Remind me to commit my changes",
          description="Commit nudge")
# Re-fires each time the session returns to idle, until removed
```

Idle scheduling cannot be combined with a clock time: passing a time-based `when` (e.g. `"in 5m"`) together with `every="idle"`, or a time-based `every` together with `when="idle"`, is rejected with an error. Use the idle keyword on both fields, or omit the one you do not need.

### Build-Check-Fix Loop

A practical pattern: schedule a build check, then fix issues when they appear.

```
> Start the long-running test suite in the background, then check results in 5 minutes

AI:
  1. shell(command="cargo test --release 2>&1 > /tmp/test-output.txt &", background=true)
  2. schedule(command="add", when="in 5m",
             message="Read /tmp/test-output.txt and report test results. If any tests failed, analyze the failures and suggest fixes.",
             description="Test results check")

# Session continues with other work...
# 5 minutes later: AI reads results, reports, suggests fixes
```

### Monitoring Pattern

Chain scheduled checks to monitor something over time:

```
> Monitor the server logs for errors every 10 minutes for the next hour

AI:
  schedule(command="add", when="in 10m",
    message="Check server logs for errors. If found, report them. Then schedule another check in 10 minutes. Stop after 6 total checks.",
    description="Log check 1/6")

# Each check fires, AI processes it, and can schedule the next one
# (Each schedule is one-shot; the AI re-schedules in response to each fired message)
```

### Managing Schedules

```
> What's scheduled?

AI calls: schedule(command="list")
# Shows all pending entries with IDs, trigger times, and countdown

> Cancel the deployment reminder

AI calls: schedule(command="remove", id="abc12345")

> Push the build check back to 20 minutes

AI calls: schedule(command="edit", id="def67890", when="in 20m")

> Stop the daily standup reminder from repeating, but keep it for tomorrow

AI calls: schedule(command="edit", id="ghi13579", every="none")
# every="none" (or every="off") clears the interval, turning a recurring entry into a one-shot
```

### Direct Control: `/schedule` Slash Command

The same operations are available as a session command, so you can list, add, edit, and remove schedules without going through the AI:

```
/schedule                                                 # list pending
/schedule add message="summarize what we just did"        # default: when="idle"
/schedule add when="in 5m" message="check the build"
/schedule add when="9am" message="standup" every="24h" description="daily"
/schedule add every="idle" message="remind me to commit"  # fires every idle
/schedule edit abc12345 when="in 1h"
/schedule edit abc12345 every="none"                      # clear a repeat interval
/schedule remove abc12345
```

Multi-word values must be quoted (`when="in 1h 30m"`, `message='hello world'`). See [Session Commands → /schedule](../reference/02-session-commands.md#schedule-subcommand-args) for the full reference.

## Daemon + Scheduled Tasks

Combine with daemon mode for long-running automated workflows:

```bash
# Start daemon session
octomind run --name project-monitor --daemon --format jsonl
```

Then send it a setup message over the session socket with `octomind send`:
```bash
echo "Set up monitoring: check git status every 30 minutes, summarize changes, and alert if there are uncommitted changes older than 2 hours" | octomind send --name project-monitor
```

The AI schedules recurring checks within the session, each check can schedule the next. A `--daemon` session stays alive regardless of the schedule queue, so it keeps firing entries indefinitely. See [Daemon and Hooks](../integration/03-daemon-and-hooks.md) for how `octomind send` reaches a running session.

## Supported Time Formats

| Format | Example | Description |
|--------|---------|-------------|
| Idle | `idle` | Fires the next time the session is idle (no running taps/jobs) |
| Immediate | `now` | Fires on the next scheduler tick |
| Relative | `in 5m` | Minutes from now |
| Relative | `in 2h` | Hours from now |
| Relative | `in 1h30m` | Combined hours and minutes |
| Relative | `in 90s` | Seconds from now |
| Relative | `in 2h 30m 10s` | Full combination (spaces optional) |
| Time today | `9am` | Bare hour, 12-hour form (minutes/seconds default to 0) |
| Time today | `15:30` | 24-hour format (tomorrow if past) |
| Time today | `3:30pm` | 12-hour format (tomorrow if past) |
| Absolute | `2026-03-30 15:30` | Exact date and time |

Relative durations accept only **h** (hours), **m** (minutes), and **s** (seconds), in any combination (`90s`, `10m`, `1h30m`, `2h 30m 10s`). There is no day or week unit -- `1d` is invalid.

The `every` field for repeating entries uses the same `h`/`m`/`s` duration grammar (e.g. `every="10m"`, `every="1h30m"`); it fires first at `when`, then repeats at that interval. `every="idle"` repeats on each idle instead.

## Limitations

- **Session-scoped**: Schedules belong to the session that created them. Each change is written as a `SCHEDULE_SNAPSHOT` entry in the session's compressed `.jsonl.zst` log, and the snapshot is re-seeded automatically when the session is resumed.
- **Process lifetime**: In a non-daemon, non-interactive run (`--format` without `--daemon`), the process exits once every scheduled entry has fired and no background jobs remain. Interactive sessions and `--daemon` runs stay alive independently of the schedule queue, so they keep waiting for future and idle entries.
- **Idle firing covers more than `run`**: idle entries also fire under the WebSocket server (`octomind server`) and the ACP agent loop, not just interactive and daemon `run` sessions.

## Key Points

- `schedule` is a built-in MCP tool -- the AI uses it naturally in conversation
- The same operations are exposed as the `/schedule` slash command for direct control
- Only `message` is required for `add`; omit both `when` and `every` to default to `when="idle"`
- Messages fire automatically and the AI processes them like any user message
- One-shot entries fire once; entries with `every` re-schedule themselves automatically until removed (clear the interval with `edit ... every="none"`)
- Combine with daemon mode for persistent monitoring
- Use `schedule(command="list")` or `/schedule` to see all pending entries
- A non-daemon non-interactive run exits once all entries have fired and no jobs remain; interactive and `--daemon` sessions stay alive regardless
