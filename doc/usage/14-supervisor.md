# Supervisor: Completion Checks, Planning and Loop Notes Now Come From Besydr

Until configuration version 31, Octomind carried a built-in supervisor: a completion check at the end of each turn, a
check on tool calls before they run, condensing of long tool output, a plan manager, notes when the agent loops, and
lessons learned from sessions. That code is no longer part of Octomind. The same work is done by **Besydr**, a separate
program that Octomind runs as a plugin. This page says how Octomind lists and starts the plugin, what Octomind still does
by itself, and where the old settings went.

## What a plugin is

A plugin is a program that Octomind sends events to (session start, each prompt, before and after tool calls, the end of
a turn) and whose answers Octomind applies. The protocol is described in [Plugins](../integration/05-plugins.md) and is
implemented in `src/plugins/`. Besydr is the only plugin a new configuration lists.

## What Besydr does

Besydr's own documentation describes each part, its switch and its limits:

- [Mechanics](https://besydr.com/docs/mechanics): the completion check, tool admission, condensing of large tool output,
  the plan manager, loop and no-progress notes, standing rules, and the extra rows in `/info`.
- [Learning](https://besydr.com/docs/learning): lessons across sessions and the `/learning` command. See also
  [Cross-session learning](13-learning.md).
- [Configuration](https://besydr.com/docs/configuration): every Besydr setting, including its models.

Besydr is free with Octomind and needs no licence there.

## How Octomind lists and starts the plugin

A new configuration has one entry in the root key `plugins`. It is an inline table, so the key stays above the first
`[table]` header of `config.toml`:

```toml
plugins = [
	{ name = "besydr", type = "stdio", command = "besydr", args = ["connect"], install_manifest = "https://dl.besydr.com/latest.json", install_key = "NP/qvbvD3tG3vqBlp+m6aMIzjM4U0p2QdWdpok15v9Y=" },
]
```

- `type = "stdio"` means Octomind starts `besydr connect` and talks to it over standard input and output. That command
  joins the Besydr server shared by all your clients and starts it when it is not running.
- When the `besydr` program is not installed, Octomind downloads it on first use from the signed release that
  `install_manifest` names. The release must be signed with `install_key`, or nothing is installed. Octomind never
  updates the program by itself.
- Provider keys: Besydr reads the OctoHub key that `octomind login` stores, so a logged-in user sets nothing.

Configurations written before version 32 get this entry when Octomind upgrades them, unless they already list a plugin
named `besydr`.

## Turning it off

Add `enabled = false` to the `besydr` entry, or remove the entry. Octomind then runs as a plain agent loop. There is no
`supervisor.enabled` switch any more. A plugin that is not listed, or is listed with `enabled = false`, receives nothing.

To turn off single parts of Besydr (for example condensing or learning) change Besydr's own configuration file, not
Octomind's.

## What Octomind does without it

With or without a plugin, Octomind still runs:

- the agent loop, tools and MCP servers, sessions and cost accounting;
- compaction of long conversations, including `/done`;
- the rules in `.agents/guardrails.toml` (see [Guardrails](18-guardrails.md));
- storage and display of the plan (`/plan`). The agent has no tool to change the plan; a plugin creates and advances it;
- the status line: the agent ends each response with a hidden line that reports its state, which Octomind reads and
  removes before display;
- a hard stop for runaway tool loops: when `max_identical_rounds` tool rounds in a row (default `10`) repeat calls and
  results already seen since the last new result, the turn ends with a notice naming the repeated call. `0` turns the
  stop off. It is a root key of `config.toml`, and `-c max_identical_rounds=0` sets it for one run;
- its own rules for which skill and which capability a request activates, which specialist gets a delegated task, and
  which tool results a compaction summarizes.

For each of those four rules Octomind also asks its plugins for a judgement. Besydr answers with its decision model
when the matching switch is on in its `[decision]` table (`skills`, `capabilities`, `taps`, `compression`; all off by
default), and Octomind then acts on the answer. With no answer the rule's own outcome stands. See
[Plugins: judge](../integration/05-plugins.md#judge).

Without the plugin there is no completion check, no tool admission check, no learning across sessions, no condensing of
tool output and no plan manager.

## When the plugin does not work

When the plugin does not answer an event, Octomind goes on without that answer and queues a notice. A plugin that is
busy is not taken for dead: while it works on an answer it tells Octomind so, and Octomind keeps waiting, for at most
one hour. After three unanswered events in a row the plugin is paused for 30 seconds and then tried again with the next
event. Each failed try doubles the pause, up to 10 minutes; one answered event ends it, with the notice
`plugin 'besydr' answers again`. A connection that was closed is opened again before the next event. A plugin that
cannot be started at all is off for that session, and the next session tries again.

Octomind prints such a notice on standard error, and only when standard error is a terminal and the output format is
not `jsonl`. A plugin that is paused or could not be started is also reported where notices are not shown:
`octomind run` prints the notice as an error on standard error, also with `--format jsonl` or with standard error sent
to a file, and the ACP and WebSocket servers write the same line to their log file. The session continues without the
plugin until it answers again.

## Where the old settings went

Version 32 of the configuration removed the tables `[supervisor]` (with all its sub-tables) and `[decision]` from
`config.toml`. Their settings now live in Besydr's configuration file, described in
[Besydr's configuration page](https://besydr.com/docs/configuration).

When Octomind upgrades a configuration, it saves the removed tables unchanged to
`besydr-carry/supervisor.toml` in the folder that holds `config.toml`. Besydr reads that file once, on its first run,
and takes the matching settings over into its own configuration file. Two more values move at the same upgrade:

- `supervisor.max_identical_rounds` becomes the root key `max_identical_rounds`.
- `supervisor.enabled = false` becomes `enabled = false` on the `besydr` plugin entry.

Command-line overrides such as `-c supervisor.gate.enabled=false` no longer name an Octomind setting. The `[decision]`
model is gone too: `octomind evaluate` takes its model from `--model`, and without the flag calls
`cloudflare:typesafe/jev`, the model Octomind shipped in `[decision]`. If you had set another model there, pass it with
`--model` (see [Calibrated checks](../use-cases/23-calibrated-checks-with-evaluate.md)).

Lessons that Octomind learned earlier are imported by Besydr on its first run.

## See also

- [Cross-session learning](13-learning.md)
- [Guardrails](18-guardrails.md)
- [Token efficiency](16-token-efficiency.md)
- [Configuration reference](../reference/03-config-reference.md)
