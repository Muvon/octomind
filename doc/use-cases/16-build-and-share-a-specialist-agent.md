# Give your team a reusable release-review specialist

Package a release-review specialist your whole team can install and use consistently across projects.

## The problem

One teammate knows which release details matter, but that knowledge lives in their prompts and review comments. Other
people ask for a release review and get different checklists, missing rollout or rollback details. Put the review
method in a named agent that everyone can install, while keeping each team's product name outside the shared manifest.

## What you will set up

- [Tap scaffolding and registration](../integration/04-tap-system.md) with `octomind tap init`.
- [An agent manifest](../integration/04-tap-system.md) addressed by a `category:variant` tag.
- [Manifest environment inputs](../integration/04-tap-system.md) using `{{ENV:TEAM_PRODUCT}}`.
- [Interactive and piped sessions](../usage/05-sessions.md) using the same specialist.
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
tap's short repository name; `release-kit:reviewer` is the agent tag. These are resources you are creating, not a
pre-existing public tap.

The command creates `./octomind-release-kit`, validates the rendered scaffold, initializes its Git repository, and
registers it locally. Look for that directory and `release-kit:reviewer` in its output.

```bash
printf 'Your GitHub username: '
read -r TAP_OWNER
export TAP_OWNER
octomind tap init "$TAP_OWNER/release-kit" --agent release-kit:reviewer
```

### 2. Locate the manifest you will replace

The agent tag resolves to `agents/release-kit/reviewer.toml` inside the tap. The tap may also contain `skills/`,
`capabilities/`, `deps/`, `workflows/`, and `plugins/`; those are separate extension surfaces. This specialist needs only
its manifest because you will supply the release note in the conversation.

Inspect the generated file before replacing it:

```bash
cat octomind-release-kit/agents/release-kit/reviewer.toml
```

### 3. Save the release-review method

Replace `octomind-release-kit/agents/release-kit/reviewer.toml` with this complete manifest. Save it before running the
agent for the first time so the initial cached copy contains your instructions.

The title and description headers describe the specialist for discovery. The filename supplies the runtime tag, which
Octomind injects into the first role's `name`. Keep the explicit name consistent with the path.

`TEAM_PRODUCT` is a non-secret input. Substitution puts its value directly into the prompt; do not use this field for
credentials. Omit a model profile so each installer can choose their own model.

```toml
# Title: Release Readiness Reviewer
# Description: Review release notes for compatibility, rollout, verification, and rollback evidence.

[[roles]]
name = "release-kit:reviewer"
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

### 4. Supply the product input and start the specialist

Use the example product value for the first run. Each teammate can export a different value without editing the tap.
The same input appears in the welcome message, giving you a visible substitution check before sending a request.

If the variable is missing, manifest resolution can prompt and save a fallback in the current directory's `.env`.
Exporting it first also avoids input prompts during piped runs. Use simple text without TOML delimiter characters.

```bash
export TEAM_PRODUCT='Harbor Reports'
octomind run release-kit:reviewer
```

### 5. Exercise the review method

Type this deliberately incomplete release note. Wait for the review before entering `/info` and `/exit`.

Expect the four requested headings and questions about the missing rollout, verification, and rollback evidence.
`Needs details` is the requested verdict. These are acceptance criteria for the prompt, not guaranteed model output.
`/info` lets you confirm the active role is `release-kit:reviewer`.

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
  octomind run release-kit:reviewer --format plain
```

### 7. Publish the manifest and install it on another machine

In GitHub, create a repository named exactly `octomind-release-kit` under the account entered in step 1. Add your
`agents/release-kit/reviewer.toml` at that same relative path using GitHub's
[file upload instructions](https://docs.github.com/en/repositories/working-with-files/managing-files/adding-a-file-to-a-repository).
Save the repository change. The one manifest is sufficient for this specialist; it does not reference scaffold files.

Use a public repository for this example. On a second machine with Octomind installed and logged in, enter the
publisher's username and install it. The installer maps `user/release-kit` to `user/octomind-release-kit` on GitHub.

```bash
printf 'Publisher GitHub username: '
read -r TAP_OWNER
export TEAM_PRODUCT='Harbor Reports'
octomind tap "$TAP_OWNER/release-kit"
octomind run release-kit:reviewer
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
  octomind run release-kit:reviewer --format plain
```

Look for the four review headings, the missing details, and `Needs details`. Confirm product substitution separately
through the interactive welcome message. Compare the same input on the second machine to check the shared behavior.

## Variations

- **Share locally.** Give a teammate the tap directory. They can register it with
  `octomind tap "$TAP_OWNER/release-kit" ./octomind-release-kit` without publishing it.
- **Use another model.** Add `-m octohub:auto` to the run command, or configure a name-only `[taps]` model mapping as
  described in the tap guide. A manifest with an explicit role model takes precedence over that mapping.
- **Another specialist.** Copy the method into a new `agents/<category>/<variant>.toml`, change the explicit role name
  to match, and supply a checklist appropriate to your team's domain.

## Troubleshooting

**Initialization refuses the destination.** Choose an empty destination with `--dir ./release-kit-draft`, or use your
existing tap directory. Initialization refuses nonempty directories and already-registered tap IDs.

**The old prompt still runs after an edit.** The tag has a separate manifest cache, even for local symlinks. Remove only
`${OCTOMIND_DATA_DIR:-$HOME/.local/share/octomind}/agents/release-kit/reviewer.toml`, then start a new session. Setting
`cache_ttl_hours = 0` alone still permits a stale cached response while refresh happens in the background.

**The product input is wrong or the run prompts for it.** Export `TEAM_PRODUCT` in the launching shell. Check for a
later assignment in the user-scope or project `.env`, which can override exported values. An explicitly empty
environment variable is accepted as empty input; it does not trigger a prompt.

**GitHub installation cannot find the agent.** Check the `octomind-` repository prefix and the exact
`agents/release-kit/reviewer.toml` path. Other taps can shadow the same tag, and the manifest cache can preserve an
earlier resolution. Inspect `octomind tap` and the tag's cached manifest before changing the prompt again.

## See also

- [Tap System](../integration/04-tap-system.md)
- [Roles](../usage/06-roles.md)
- [Sessions](../usage/05-sessions.md)
- [Session Commands](../reference/02-session-commands.md)
- [Environment Variables](../reference/04-environment-variables.md)
