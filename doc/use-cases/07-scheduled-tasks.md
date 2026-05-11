# Use Case: Scheduled Tasks and Timed Automation

Use the built-in `schedule` tool to have the AI perform actions at specific times during a session -- reminders, periodic checks, timed workflows.

## The Problem

During long development sessions, you need the AI to do things at specific times: check build status in 10 minutes, remind you about a meeting, run a test suite after lunch. External cron jobs are overkill. You want scheduling inside the session.

## Solution

The `schedule` MCP tool lets the AI (or you) schedule messages that fire at specific times and get processed automatically.

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
```

### Direct Control: `/schedule` Slash Command

The same operations are available as a session command, so you can list, add, edit, and remove schedules without going through the AI:

```
/schedule                                                 # list pending
/schedule add when="in 5m" message="check the build"
/schedule add when="9am" message="standup" every="1d" description="daily"
/schedule edit abc12345 when="in 1h"
/schedule remove abc12345
```

Multi-word values must be quoted (`when="in 1h 30m"`, `message='hello world'`). See [Session Commands → /schedule](../reference/02-session-commands.md#schedule-subcommand-args) for the full reference.

## Daemon + Scheduled Tasks

Combine with daemon mode for long-running automated workflows:

```bash
# Start daemon session
octomind run --name project-monitor --daemon --format jsonl
```

Then inject a setup message:
```bash
echo "Set up monitoring: check git status every 30 minutes, summarize changes, and alert if there are uncommitted changes older than 2 hours" | octomind send --name project-monitor
```

The AI schedules recurring checks within the session, each check can schedule the next.

## Supported Time Formats

| Format | Example | Description |
|--------|---------|-------------|
| Immediate | `now` | Fires on the next scheduler tick |
| Relative | `in 5m` | Minutes from now |
| Relative | `in 2h` | Hours from now |
| Relative | `in 1h30m` | Combined hours and minutes |
| Relative | `in 90s` | Seconds from now |
| Relative | `in 2h 30m 10s` | Full combination (spaces optional) |
| Time today | `15:30` | 24-hour format (tomorrow if past) |
| Time today | `3:30pm` | 12-hour format (tomorrow if past) |
| Absolute | `2026-03-30 15:30` | Exact date and time |

## Limitations

- **In-memory**: Schedules are lost when the session exits. Use daemon mode (`--daemon`) for long-running scheduled work.
- **Local timezone**: All times are interpreted in the system's local timezone.
- **Session-scoped**: Schedules belong to the session that created them.

## Key Points

- `schedule` is a built-in MCP tool -- the AI uses it naturally in conversation
- The same operations are exposed as the `/schedule` slash command for direct control
- Messages fire automatically and the AI processes them like any user message
- One-shot entries fire once; entries with `every` re-schedule themselves automatically until removed
- Combine with daemon mode for persistent monitoring
- Use `schedule(command="list")` or `/schedule` to see all pending entries
- The session stays alive until all scheduled messages have fired
