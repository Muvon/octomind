# Give your team a reusable release-review agent

Package a release-review agent as a tap that your whole team can install and use consistently across
projects.

## The problem

One teammate knows which release details matter, but that knowledge lives in their prompts and review comments. Other
people ask for a release review and get different checklists, missing rollout or rollback details. Put the review
method in a named agent that everyone can install, while keeping each team's product name outside the shared manifest.

## What you will set up

- [Tap scaffolding and registration](../integration/04-tap-system.md) with `octomind tap init`.
- [An agent manifest](../integration/04-tap-system.md) addressed by its name.
- [Manifest environment inputs](../integration/04-tap-system.md) using `{{ENV:TEAM_PRODUCT}}`.
- [Interactive and piped sessions](../usage/05-sessions.md) using the same agent.
- [Tap distribution and removal](../integration/04-tap-system.md) through GitHub or a local directory.

## Prerequisites

Use Bash on macOS or Linux. Check Octomind and your existing login:

```bash
octomind --version
octomind login
octomind config --validate
```

Tap initialization uses Git and a downloaded scaffold from the built-in `muvon/tap`. The scaffold's validator can
require Bash and Python with TOML support. Check these before starting:

```bash
git --version
bash --version
python3 -c 'import tomllib; print("TOML parser ready")'
```

Use Python 3.11 or later for that check. You need network access for the initial scaffold download. To share through
GitHub, sign in to your GitHub account in a browser and follow its
[repository creation instructions](https://docs.github.com/en/repositories/creating-and-managing-repositories/creating-a-new-repository).
The local authoring steps do not require a GitHub token.

## Steps

### 1. Create the tap under your account name

Run this from a parent directory where you keep projects. Enter your GitHub username at the prompt. `release-kit` is the
tap's short repository name; `release-reviewer` is the agent you create in steps 2 and 3. These
are resources you are creating, not a pre-existing public tap.

The command creates `./octomind-release-kit`, validates the rendered scaffold, initializes its Git repository, and
registers it locally. It generates a starter agent at `agents/release-reviewer/default.toml`. Look for
that directory and agent name in its output.

```bash
printf 'Your GitHub username: '
read -r TAP_OWNER
export TAP_OWNER
octomind tap init "$TAP_OWNER/release-kit" --agent release-reviewer
```

### 2. Locate where the agent goes

The starter agent is `agents/release-reviewer/default.toml` inside the tap. The directory `release-reviewer` is the agent's namespace, and the file `default.toml` is its base agent, so the tag is `release-reviewer`. The tap may also contain `skills/`, `capabilities/`, `deps/`,
`workflows/`, and `plugins/`; those are separate extension surfaces. This agent needs only its manifest because you
will supply the release note in the conversation.

Inspect the generated starter before replacing it:

```bash
cat octomind-release-kit/agents/release-reviewer/default.toml
```

### 3. Save the release-review method

Replace `octomind-release-kit/agents/release-reviewer/default.toml` with this complete manifest. Save it before running the
agent for the first time so the initial cached copy contains your instructions.

The title and description headers describe the agent for discovery. The namespace directory supplies the runtime tag, which
Octomind injects into the first role's `name`. Keep the explicit name consistent with the path.

`TEAM_PRODUCT` is a non-secret input. Substitution puts its value directly into the prompt; do not use this field for
credentials. Omit a model profile so each installer can choose their own model.

```toml
# Agent: release-reviewer
# Title: Release Readiness Reviewer
# Description: Review release notes for compatibility, rollout, verification, and rollback evidence.

[[roles]]
name = "release-reviewer"
system = """
You review release notes for {{ENV:TEAM_PRODUCT}}.
Use only information supplied in the conversation.
Do not fetch files, run commands, or deploy anything.
For each review, use these four headings:
Compatibility, Rollout, Verification, Rollback.
Under each heading, quote relevant evidence from the supplied note.
If evidence is missing, write Missing and ask one specific question.
Finish with Ready only when all four areas have explicit evidence;
otherwise finish with Needs details.
Treat a claim that something was tested as a claim, not independent proof.
"""
welcome = "Release review for {{ENV:TEAM_PRODUCT}}. Paste a release note."

[roles.mcp]
server_refs = []
allowed_tools = []
```

### 4. Supply the product input and start the agent

Use the example product value for the first run. Each teammate can export a different value without editing the tap.
The same input appears in the welcome message, giving you a visible substitution check before sending a request.

If the variable is missing, manifest resolution can prompt and save a fallback in the current directory's `.env`.
Exporting it first also avoids input prompts during piped runs. Use simple text without TOML delimiter characters.

```bash
export TEAM_PRODUCT='Harbor Reports'
octomind run release-reviewer
```

### 5. Exercise the review method

Type this deliberately incomplete release note. Wait for the review before entering `/info` and `/exit`.

Expect the four requested headings and questions about the missing rollout, verification, and rollback evidence.
`Needs details` is the requested verdict. These are acceptance criteria for the prompt, not guaranteed model output.
`/info` lets you confirm the active role is `release-reviewer`.

```text
Review this release note: Harbor Reports 2.4 changes the CSV export date format from MM/DD/YYYY to YYYY-MM-DD.
/info
/exit
```

### 6. Check that the input can change independently of the manifest

Run a second review with a different product name and a complete note. The model should evaluate the supplied evidence
under the same headings. It should not claim to have performed the staged rollout or rollback drill itself.

```bash
export TEAM_PRODUCT='Harbor Inventory'
printf '%s\n' \
  'Review: Harbor Inventory 2.4 keeps the existing CSV schema.' \
  'Rollout: enable for the internal team first, then expand after a day without export errors.' \
  'Verification: the release owner reports that the CSV fixture checks passed on the release candidate.' \
  'Rollback: disable the feature flag and restore the previous service image; the owner reports a drill passed.' | \
  octomind run release-reviewer --format plain
```

### 7. Publish the manifest and install it on another machine

In GitHub, create a repository named exactly `octomind-release-kit` under the account entered in step 1. Add your
`agents/release-reviewer/default.toml` at that same relative path using GitHub's
[file upload instructions](https://docs.github.com/en/repositories/working-with-files/managing-files/adding-a-file-to-a-repository).
Save the repository change. The one manifest is sufficient for this agent; it does not reference scaffold files.

Use a public repository for this example. On a second machine with Octomind installed and logged in, enter the
publisher's username and install it. The installer maps `user/release-kit` to `user/octomind-release-kit` on GitHub.

```bash
printf 'Publisher GitHub username: '
read -r TAP_OWNER
export TEAM_PRODUCT='Harbor Reports'
octomind tap "$TAP_OWNER/release-kit"
octomind run release-reviewer
```

### 8. Remove the installation when you finish trying it

Exit the session first. In the same shell where `TAP_OWNER` is set, remove the tap and inspect the registration list.
Removing a local tap preserves its source directory. Removing a GitHub tap leaves its clone on disk. Neither operation
clears cached agent manifests, so registration removal alone is not a test that the tag can no longer run.

```bash
octomind untap "$TAP_OWNER/release-kit"
octomind tap
```

## Verify it works

On your authoring machine, keep the local tap registered. If you removed it in step 8, restore it first with
`octomind tap "$TAP_OWNER/release-kit" ./octomind-release-kit`. Then send a small review:

```bash
export TEAM_PRODUCT='Harbor Reports'
printf '%s\n' 'Review: Harbor Reports changes a CSV column name. No rollout or rollback details are available.' | \
  octomind run release-reviewer --format plain
```

Look for the four review headings, the missing details, and `Needs details`. Confirm product substitution separately
through the interactive welcome message. Compare the same input on the second machine to check the shared behavior.

## Skill or variant

An agent covers a whole field of work and does the work itself. A specialty inside the field (a procedure, a format, a
jurisdiction, a platform) is a skill of the agent's namespace. This is the default, and it is how the community tap is
built: 34 base agents, about 330 skills, no variants.

A variant (`<namespace>:<variant>`) is the exception. It is for a private tap, and only when a skill cannot do the job.
Choose by asking, in this order:

1. Does the agent need extra instructions for a procedure, a format, a jurisdiction or a platform? Write a skill.
2. Does it need extra tools on top of the base agent? Write a skill that brings them with `capabilities:` in its
   frontmatter.
3. Does it need its own system prompt and identity, its own set of capabilities (fewer or different tools), its own
   model, or its own safety rules? Write a variant.

See [Tap System](../integration/04-tap-system.md#skill-or-variant) for the layout of both.

## Variations

- **Share locally.** Give a teammate the tap directory. They can register it with
  `octomind tap "$TAP_OWNER/release-kit" ./octomind-release-kit` without publishing it.
- **Use another model.** Add `-m octohub:auto` to the run command, or configure a name-only `[taps]` model mapping as
  described in the tap guide. A manifest with an explicit role model takes precedence over that mapping.
- **Another field of work.** Copy the method into a new `agents/<name>/default.toml`, change the explicit role name
  to match, and supply a checklist appropriate to your team's domain.
- **A specialty.** Add an owned skill at `agents/release-reviewer/skills/<skill>/SKILL.md`; it applies to the base
  agent and every variant of the namespace.
- **A variant.** Only when a skill cannot do the job (see above): add `agents/release-reviewer/strict.toml`, a
  complete manifest of its own. Start it with `octomind run release-reviewer:strict`; `octomind run release-reviewer`
  still starts the base agent.

## Troubleshooting

**Initialization refuses the destination.** Choose an empty destination with `--dir ./release-kit-draft`, or use your
existing tap directory. Initialization refuses nonempty directories and already-registered tap IDs.

**The old prompt still runs after an edit.** The tag has a separate manifest cache, even for local symlinks. Remove only
`${OCTOMIND_DATA_DIR:-$HOME/.local/share/octomind}/agents/release-reviewer.*.toml`, then start a new session. Setting
`cache_ttl_hours = 0` alone still permits a stale cached response while refresh happens in the background.

**The product input is wrong or the run prompts for it.** Export `TEAM_PRODUCT` in the launching shell. Check for a
later assignment in the user-scope or project `.env`, which can override exported values. An explicitly empty
environment variable is accepted as empty input; it does not trigger a prompt.

**GitHub installation cannot find the agent.** Check the `octomind-` repository prefix and the exact
`agents/release-reviewer/default.toml` path. Other taps can shadow the same agent name, and the manifest cache can preserve an
earlier resolution. Inspect `octomind tap` and the tag's cached manifest before changing the prompt again.

## See also

- [Tap System](../integration/04-tap-system.md)
- [Roles](../usage/06-roles.md)
- [Sessions](../usage/05-sessions.md)
- [Session Commands](../reference/02-session-commands.md)
- [Environment Variables](../reference/04-environment-variables.md)
