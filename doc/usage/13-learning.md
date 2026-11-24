# Cross-Session Learning: Agent Memory Across Sessions

Cross-session learning stores the corrections and rules you give an agent as lessons, and adds the ones that fit a later
request to its context. Octomind no longer does this itself. The Besydr plugin
provides it. This page says how to get it, how to turn it off, and where the settings went.

## Where it comes from

Besydr is a separate program that Octomind runs as a plugin. A new configuration lists it in the root key `plugins`, and
Octomind downloads the program on first use when it is not installed. See [Supervisor](14-supervisor.md) for how Octomind
lists and starts the plugin and what happens when it fails.

Learning is on by default in Besydr. Besydr's own pages cover what is stored, how lessons are extracted, recalled and
retired, and how to inspect them:

- [Learning](https://besydr.com/docs/learning): kinds of lesson, scope, storage, recall, retention and generated rules.
- [Configuration](https://besydr.com/docs/configuration): the `[learning]` and `[learning.evolution]` settings.

## Commands

With Besydr connected, the plugin registers `/learning` in the session, with the sub-commands `list`, `show`, `delete`,
`clear` and `evolution`. They are listed on the Besydr page above. Without the plugin, Octomind has no `/learning`
command, and the hidden `octomind distill` command is gone.

Extraction still follows your session: `/done` asks Octomind to compact the conversation, and Octomind tells the plugin
before it folds anything away.

## Turning it off

To turn off learning only, set `[learning] enabled = false` in Besydr's configuration file. To turn off Besydr as a
whole, set `enabled = false` on its `plugins` entry in `config.toml`, or remove the entry. Without a plugin, Octomind
keeps nothing across sessions on its own.

## Where the settings went

The `[supervisor.learning]` tables are gone from `config.toml` as of configuration version 32. When Octomind upgrades a
configuration, it saves the removed tables unchanged to `besydr-carry/supervisor.toml` in the folder that holds
`config.toml`. Besydr reads that file once, on its first run, and takes the matching settings over into its own
configuration file.

Lessons that Octomind learned earlier are imported by Besydr on its first run. The lessons live in Besydr's data
directory from then on.

## See also

- [Supervisor](14-supervisor.md)
- [Skills](15-skills.md)
- [Guardrails](18-guardrails.md)
- [Session commands](../reference/02-session-commands.md)
- [Configuration reference](../reference/03-config-reference.md)
