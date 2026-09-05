# Keep private code in local model sessions

Configure local model sessions that keep private code on your machine and make any cloud switch explicit.

## The problem

You need help understanding private code, but your team cannot send it to a hosted model. You may also want routine
explanations to use your own machine instead of a paid API. Selecting a local-looking model name is not enough: the
endpoint, internal model requests, and any later switch to a cloud role all matter.

## What you will set up

- [Ollama provider routing](../usage/04-providers.md) through an explicit local `OLLAMA_API_URL`.
- [An isolated configuration](../usage/03-configuration.md) with local main, supervisor, and compression profiles.
- [Per-role model selection](../usage/06-roles.md) for private and public conversations.
- [A tap model mapping](../integration/04-tap-system.md) for a locally authored specialist.
- [Runtime overrides and inspection](../reference/02-session-commands.md) using `-m` and `/info`.

## Prerequisites

Use Bash on macOS or Linux. Check Octomind, your existing login for the later cloud example, and the utilities used to
configure the tutorial:

```bash
octomind --version
octomind login
bash --version
git --version
python3 --version
curl --version
```

Install Ollama and download a model your machine can run using its [CLI documentation](https://docs.ollama.com/cli).
Choose a locally stored model, not a cloud model. Check the installed command and the server's model list:

```bash
command -v ollama
ollama ls
curl --fail http://localhost:11434/api/tags
```

The last command should return a `models` array containing your chosen model, as documented by Ollama's
[list-models API](https://docs.ollama.com/api/tags). Model availability, memory requirements, and supported features
depend on your installation. You will enter the actual installed name instead of downloading a hardcoded model here.
Tap startup also needs Git and initial network access to download its built-in fallback tap.

## Steps

### 1. Disable Ollama's cloud features

Follow Ollama's [cloud-disable instructions](https://docs.ollama.com/faq) for an existing service and restart it. Confirm
its logs contain `Ollama cloud disabled: true`. The setting must reach the Ollama server process, not just Octomind.

If you run the server manually, stop the existing instance first and launch this in a separate terminal. Leave it
running while you continue in your original terminal:

```bash
OLLAMA_NO_CLOUD=1 ollama serve
```

### 2. Select the installed model and local endpoint

Enter a model name exactly as listed by `ollama ls`, without an `ollama:` provider prefix. The shell adds the prefix for
Octomind. For example, `ollama:glm-5.3` appears in the provider guide, but it is usable only if your server can actually
host that model. Choose the model you already have.

Octolib's `ollama:` adapter defaults to `https://ollama.com/v1/chat/completions`. Set the full local Chat Completions URL
explicitly. Ollama documents this API under [OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility).

```bash
printf 'Installed local Ollama model name: '
read -r PRIVATE_OLLAMA_MODEL
test -n "$PRIVATE_OLLAMA_MODEL"
export PRIVATE_MODEL="ollama:$PRIVATE_OLLAMA_MODEL"
export OLLAMA_API_URL='http://localhost:11434/v1/chat/completions'
```

### 3. Create a separate configuration for these sessions

Create a new experiment directory outside your private repository. Octomind generates its complete default config
when the specified config directory has no TOML files. Your existing login stays available because this changes only
the config location, not the data directory.

The initial validation generates and checks defaults; it does not send a code prompt. Keep this shell open because
the following commands use its environment variables.

```bash
mkdir octomind-private-lab
cd octomind-private-lab
mkdir settings public
export OCTOMIND_CONFIG_PATH="$PWD/settings/config.toml"
octomind config --validate
unset OCTOMIND_CAPABILITIES OCTOMIND_SKILLS
```

### 4. Save the model routing configuration

Save this complete overlay at `octomind-private-lab/settings/privacy.toml`. It merges with the generated `config.toml`.
The next step replaces the example local name with your actual selection before any conversation begins.

The main baseline and both internal model profiles stay local. The private role has its own model table; the public
role explicitly uses OctoHub. Empty server references and disabled automatic activation keep this example focused on
pasted text. Start it in this new directory, without project-local tools or instructions.

```toml
auto_capabilities = false
telemetry = false

[model]
name = "ollama:glm-5.3"
reasoning_effort = "medium"
max_tokens = 4096
temperature = 0.3
top_p = 0.7
top_k = 20
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300

[supervisor.model]
name = "ollama:glm-5.3"
max_tokens = 4096

[compression.model]
name = "ollama:glm-5.3"
max_tokens = 4096

[skills]
auto_activation = false
auto_validation = false

[taps]
"private-notes:review" = "ollama:glm-5.3"

[[roles]]
name = "private_review"
system = "Explain the code supplied in this conversation. Do not request external tools or delegate work."
welcome = "Local code discussion. Check /info before pasting code."

[roles.model]
name = "ollama:glm-5.3"

[roles.mcp]
server_refs = []
allowed_tools = []

[[roles]]
name = "public_brief"
system = "Write short explanations from public information supplied by the user."
welcome = "Cloud conversation: public information only."

[roles.model]
name = "octohub:auto"

[roles.mcp]
server_refs = []
allowed_tools = []
```

### 5. Substitute your local model and confirm the endpoint

This small script replaces every example local model in the overlay. JSON string quoting also produces a valid TOML
basic string for an ordinary model identifier. It writes only `settings/privacy.toml`.

Also save a project `.env` with the explicit endpoint. Octomind loads the user-scope `.env` after exported values, then
the project `.env` last. The project assignment prevents an old user-scope endpoint from redirecting this session.

```bash
python3 - <<'PY'
import json
import os
from pathlib import Path
path = Path('settings/privacy.toml')
text = path.read_text()
path.write_text(text.replace('"ollama:glm-5.3"', json.dumps(os.environ['PRIVATE_MODEL'])))
PY
printf '%s\n' 'OLLAMA_API_URL=http://localhost:11434/v1/chat/completions' > .env
octomind config --validate
```

### 6. Verify a local answer before pasting private code

Use a fresh session. The explicit `-m` override selects the main model for this invocation; the two internal profiles
remain governed by the overlay you just saved.

```bash
octomind run private_review --name local-code-check -m "$PRIVATE_MODEL"
```

Type each entry separately. `/info` should show your `ollama:` model and `private_review` role. Use this harmless sample
first, then inspect again after the reply. The correct explanation is that the slice excludes the last item.

```text
/info
Explain this Python function: def total(items): return sum(items[:-1])
/info
/exit
```

### 7. Apply a name-only mapping to a tap specialist

Create your own local tap directory and save the manifest below at
`octomind-private-lab/octomind-private-notes/agents/private-notes/review.toml`. This tag is supplied by the file you
create; it is not assumed to exist in the built-in tap.

```bash
mkdir -p octomind-private-notes/agents/private-notes
```

The role omits `[roles.model]`, allowing the `[taps]` mapping in `privacy.toml` to select its main model. The environment
input is a non-secret workspace label and is resolved when the manifest loads.

```toml
# Title: Private Code Notes
# Description: Explain supplied code and record unresolved questions for a local workspace.

[[roles]]
name = "private-notes:review"
system = "Explain code supplied for {{ENV:PRIVATE_WORKSPACE}}. Use only conversation content."
welcome = "Local notes for {{ENV:PRIVATE_WORKSPACE}}. Check /info before adding code."

[roles.mcp]
server_refs = []
allowed_tools = []
```

### 8. Run the mapped specialist

Register the directory as a local tap. `tutorial/private-notes` is only a local registration in this example; no remote
repository is needed. Check `/info` in the resulting session, then `/exit` before continuing.

```bash
export PRIVATE_WORKSPACE='Private code lab'
octomind tap tutorial/private-notes ./octomind-private-notes
octomind run private-notes:review
```

### 9. Keep cloud work in a fresh public conversation

Run the cloud role from the empty `public` directory, using a new process and session. The absolute config path still
points to your lab settings. Do not resume a private conversation with a cloud override: its history can be sent to
the new provider.

```bash
cd public
printf '%s\n' 'Explain what a CSV file is in two sentences.' | octomind run public_brief --format plain
cd ..
```

## Verify it works

Start a new local conversation and inspect it before and after a harmless request:

```bash
octomind run private_review -m "$PRIVATE_MODEL"
```

```text
/info
Reply with the word local.
/info
```

Look for your selected `ollama:` model. In a second terminal, `ollama ps` should show the model loaded on your server.
Check that `settings/privacy.toml` assigns the same local provider to main, supervisor, and compression. `/info` reports
the main session model; it is not an audit of every internal request or proof of zero network traffic. This setup
controls model routing, not OS network permissions.

## Variations

- **Another installed model.** Change the local model names in all five overlay locations. A main-only `-m` change
  does not update supervisor or compression. Choose a model with the features and context size your work requires.
- **Private-only configuration.** Remove the `public_brief` role declaration and its nested tables to omit the cloud
  example. Keep all internal profiles local.
- **Return to normal settings.** In a new shell your exported config path is absent. In this shell, use
  `unset OCTOMIND_CONFIG_PATH` before a new run to restore the default config location.

## Troubleshooting

**The request goes to a cloud endpoint.** Check the project and user-scope `.env` assignments, the full
`OLLAMA_API_URL`, and the Ollama server's cloud-disabled log message. Restart the server after changing its settings.

**The model is missing or fails during a request.** Compare its exact name with `ollama ls`. Config validation does not
load model weights or prove API compatibility. Check Ollama's logs, available memory, and the model's supported context
and structured-output features; internal supervisor requests may need more than basic text completion.

**The tap mapping seems ignored.** An explicit manifest role model takes precedence over `[taps]`; `-m` takes precedence
over both. The example manifest deliberately omits a role model. Check the tag's cached manifest if you edited it after
its first run, following the tap guide's cache procedure.

**The first local reply works, but later requests use another model.** Inspect the supervisor and compression profile
names, any role overrides, and the selected config directory. Never use `/model` to switch a private conversation to
a cloud provider. Start public work in a fresh conversation without private instructions or files in its context.

## See also

- [Providers](../usage/04-providers.md)
- [Configuration](../usage/03-configuration.md)
- [Roles](../usage/06-roles.md)
- [Tap System](../integration/04-tap-system.md)
- [Session Commands](../reference/02-session-commands.md)
- [Compression](../usage/08-compression.md)
- [Supervisor](../usage/14-supervisor.md)
