# Environment Variables

Use this reference when configuring credentials, relocating state, or diagnosing startup and script behavior. It
inventories direct environment reads in `src/`, delegated provider settings, and build/test-only variables.

## Get started

```bash
octomind login
octomind run
```

For a provider key you manage yourself, store its actual value in the environment or `.env`, then select a matching
model. `octomind config` shows credential presence/source without displaying the key.

## .env File Support

Octomind automatically loads `.env` files at startup, as an alternative to exporting variables in your shell. This is
useful for API keys:

```bash
# .env
OPENROUTER_API_KEY="YOUR_PROVIDER_KEY"
```

Two `.env` locations are loaded, in precedence order (later wins):

1. **User-scope** — `<config_dir>/.env` in the shared config directory (under `OCTOMIND_DATA_DIR` or the standard data root, independent of `OCTOMIND_CONFIG_PATH`). Shared across all projects.
2. **Project-local** — `./.env` in the current working directory. Overrides the user-scope file.

System environment variables are the base; both `.env` files override them.

Key behaviors:

- **`.env` overrides the system environment.** When a variable is defined in both, the `.env` value wins.
- **Project-local `.env` overrides user-scope `.env`.** A key set in the working directory wins over the same key in the shared config directory.
- **Empty values are treated as "not set."** A variable whose value is empty (or only whitespace) is reported as `NotFound` for API-key source detection — but they still overwrite a previous value. Individual providers need not treat empty strings as absent.
- **Source tracking.** The `EnvTracker` records whether each variable came from the system environment (`System`) or the `.env` file (`DotEnv`); the credential status rows in `octomind config` show this source. Tracking compares before/after values, so
  an identical `.env` value is still reported as the original environment source.

`YOUR_PROVIDER_KEY` is the credential you obtain from that provider. `.env` parsing happens before CLI/config loading;
parse errors print a warning and stop the remaining dotenv steps. Set `OCTOMIND_DATA_DIR` in the launch environment if
you want it to select the user-scope `.env` location.

## API Keys

All API keys are read from environment variables for security. **Never put API keys in config files** — the `octomind
config --api-key provider:key` command is intentionally rejected at runtime ("API keys can no longer be set in config
file for security reasons") and tells you to export the matching environment variable instead.

Provider authentication is delegated through [providers.rs](../../src/providers.rs) to the locked octolib 0.35.0
implementation. These names are explicit provider contracts, not a universal uppercase-prefix rule. Keys are read during
provider credential checks and requests; URL overrides are read when requests are built, except CLI backend settings,
which are captured when that backend is constructed.

### Provider credentials

Required unless the row says otherwise; unset credentials do not inherit from another provider.

| Variable | Model prefix | When read / effect |
|---|---|---|
| `OPENROUTER_API_KEY` | `openrouter` | OpenRouter credential. |
| `OPENAI_API_KEY` | `openai` | OpenAI credential; see OAuth caveat below. |
| `ANTHROPIC_API_KEY` | `anthropic` | Anthropic credential; see OAuth caveat below. |
| `DEEPSEEK_API_KEY` | `deepseek` | DeepSeek credential. |
| `GOOGLE_STUDIO_API_KEY` | `google-studio` | Google AI Studio credential. |
| `GOOGLE_VERTEX_CREDENTIAL_FILE` | `google-vertex` | Nonempty service-account JSON path, preferred over the standard Google variable. |
| `GOOGLE_APPLICATION_CREDENTIALS` | `google-vertex` | Fallback service-account JSON path. Also displayed by Octomind config inspection. |
| `AWS_BEARER_TOKEN_BEDROCK` | `amazon` | Nonempty Bedrock bearer token; this adapter does not use SigV4 access keys. |
| `AWS_ACCESS_KEY_ID` | Inspection only | Read by `octomind config` status rows; setting it does not authenticate the current Bedrock adapter. |
| `CLOUDFLARE_API_TOKEN` | `cloudflare` | Workers AI token. |
| `CLOUDFLARE_ACCOUNT_ID` | `cloudflare` | Required account identifier when forming a request. |
| `CEREBRAS_API_KEY` | `cerebras` | Cerebras credential. |
| `GROQ_API_KEY` | `groq` | Groq credential. |
| `TOGETHER_API_KEY` | `together` | Together credential. |
| `FIREWORKS_API_KEY` | `fireworks` | Fireworks credential. |
| `NVIDIA_API_KEY` | `nvidia` | NVIDIA credential. |
| `MINIMAX_API_KEY` | `minimax` | MiniMax credential. |
| `MOONSHOT_API_KEY` | `moonshot`, `kimi` | Shared credential for the canonical prefix and alias. |
| `XAI_API_KEY` | `xai` | xAI credential. |
| `ZAI_API_KEY` | `zai` | Z.AI credential. |
| `BYTEPLUS_API_KEY` | `byteplus` | BytePlus credential. |
| `ALIBABA_API_KEY` | `alibaba` | Alibaba credential. |
| `FEATHERLESS_API_KEY` | `featherless` | Featherless credential. |
| `HETZNER_API_KEY` | `hetzner` | Hetzner credential. |
| `OPENCODE_API_KEY` | `opencode-zen`, `opencode-go` | Shared credential for both OpenCode adapters. |
| `OCTOHUB_API_KEY` | `octohub` | Gateway credential; `octomind login` writes it to the shared `.env` and current process. Account lookup also reads it for legacy migration. |
| `OLLAMA_API_KEY` | `ollama` | Optional; omitted becomes an empty credential. |
| `LOCAL_API_KEY` | `local` | Optional; omitted becomes an empty credential. |

```bash
export OPENROUTER_API_KEY="YOUR_PROVIDER_KEY"
echo "Explain this project" | octomind run --model openrouter:anthropic/claude-sonnet-4 --format plain
```

### OAuth variables

| Variable | When read / effect |
|---|---|
| `OPENAI_OAUTH_ACCESS_TOKEN` | octolib’s OpenAI OAuth request path takes priority over API-key auth. |
| `OPENAI_OAUTH_ACCOUNT_ID` | Required alongside the OpenAI OAuth token by its request path. |
| `ANTHROPIC_OAUTH_ACCESS_TOKEN` | octolib’s Anthropic OAuth request path takes priority over API-key auth. |

The current CLI session preflight calls `get_api_key()`, which returns an error when either OAuth access-token variable
is present. Thus provider-level OAuth support does not make an OAuth-only `octomind run` startup work. For the API-key
path, remove the conflicting token from both `.env` and the launching environment:

```bash
unset OPENAI_OAUTH_ACCESS_TOKEN ANTHROPIC_OAUTH_ACCESS_TOKEN
octomind config
```

### Provider routing and endpoint overrides

Each URL variable replaces the adapter’s default request URL unless its row says “base URL”. Leave it unset to use the
built-in endpoint. There is no generic `GOOGLE_API_URL` or universal `<PREFIX>_API_URL` fallback.

| Variable | When read / effect |
|---|---|
| `GOOGLE_VERTEX_PROJECT_ID` | Nonempty value overrides `project_id` from the service-account JSON for the default Vertex URL. |
| `GOOGLE_VERTEX_LOCATION` | Region for the default Vertex URL; unset/blank defaults to `us-central1`. |
| `AWS_BEDROCK_REGION` | Region for the default Bedrock URL; unset defaults to `us-east-1`. |
| `OPENROUTER_API_URL` | OpenRouter request endpoint. |
| `OPENAI_API_URL` | OpenAI request endpoint. |
| `ANTHROPIC_API_URL` | Anthropic request endpoint. |
| `GOOGLE_VERTEX_API_URL` | Full Vertex endpoint; bypasses project/location URL construction. |
| `GOOGLE_STUDIO_API_URL` | Google AI Studio endpoint. |
| `AWS_BEDROCK_API_URL` | Bedrock endpoint, overriding regional URL construction. |
| `CLOUDFLARE_API_URL` | Full Workers AI endpoint override; the default URL is constructed from `CLOUDFLARE_ACCOUNT_ID`. |
| `CEREBRAS_API_URL` | Cerebras endpoint. |
| `GROQ_API_URL` | Groq endpoint. |
| `FIREWORKS_API_URL` | Fireworks endpoint. |
| `NVIDIA_API_URL` | NVIDIA endpoint. |
| `MINIMAX_API_URL` | MiniMax endpoint. |
| `ZAI_API_URL` | Z.AI endpoint. |
| `BYTEPLUS_API_URL` | BytePlus endpoint. |
| `ALIBABA_API_URL` | Alibaba endpoint. |
| `FEATHERLESS_API_URL` | Featherless endpoint. |
| `HETZNER_API_URL` | Hetzner endpoint. |
| `OPENCODE_ZEN_API_URL` | OpenCode Zen endpoint. |
| `OPENCODE_GO_API_URL` | OpenCode Go endpoint. |
| `XAI_API_URL` | xAI Responses endpoint. |
| `OCTOHUB_API_URL` | OctoHub base URL. |
| `OLLAMA_API_URL` | Ollama OpenAI-compatible chat endpoint. |
| `LOCAL_API_URL` | Self-hosted OpenAI-compatible chat endpoint. |

The locked DeepSeek, Moonshot, and Together adapters use fixed endpoints; their API URL variables are not read. For your
running local service (replace the model with one it serves):

```bash
export LOCAL_API_URL="http://127.0.0.1:8080/v1/chat/completions"
octomind run --model local:my-model
```

### CLI meta-provider backend variables

`cli:BACKEND/MODEL` runs an installed executable. Octomind’s provider credential preflight skips `cli`; the external
program manages its own authentication. The normalized backend name is uppercased in variable names.

| Variable | Default / override behavior at backend construction |
|---|---|
| `CLI_<BACKEND>_COMMAND` | Executable; normally backend name (`cursor` uses `cursor-agent`). |
| `CODEX_COMMAND` | Legacy Codex executable fallback after `CLI_CODEX_COMMAND`; default `codex`. |
| `CLI_<BACKEND>_EXTRA_ARGS` | Empty; extra arguments split on whitespace, without shell quote parsing. |
| `CLI_<BACKEND>_MODEL_FLAG` | `-m`; generic backend model argument flag. |
| `CLI_<BACKEND>_PROMPT_FLAG` | `-p`; generic backend prompt argument flag. |
| `CLI_CODEX_REASONING_EFFORT`, `CODEX_REASONING_EFFORT` | First variable wins; `low`, `medium`, `high`; unset/invalid becomes `high`. |
| `CLI_CODEX_SKIP_GIT_CHECK`, `CODEX_SKIP_GIT_CHECK` | Either truthy value enables skipping; default false. Truthy: `1`, `true`, `yes`, `on`, case-insensitive. |

```bash
export CLI_CODEX_COMMAND="codex"
export CLI_CODEX_REASONING_EFFORT="high"
octomind run --model cli:codex/gpt-5
```

## Octomind Configuration

| Variable | Description |
|----------|-------------|
| `OCTOMIND_DATA_DIR` | Override the directory holding durable Octomind state — config, sessions, logs, cache, learning. Host-local run sockets and dependency-owned model caches are separate. Read whenever a data path is resolved. Default: `~/.local/share/octomind` (Linux/macOS) or `%LOCALAPPDATA%\octomind` (Windows). Redirecting `HOME` does not work on Windows, so this is the portable way to run octomind against a throwaway state directory. |
| `OCTOMIND_CONFIG_PATH` | Read at startup to choose the primary config load/migration/save path. The value is the path to the primary config TOML; its parent directory becomes the config directory for multi-file merge (all `*.toml` files there are merged). Default file: `~/.local/share/octomind/config/config.toml` (Linux/macOS) or `%LOCALAPPDATA%\octomind\config\config.toml` (Windows). |
| `OCTOMIND_SKILLS` | Comma-delimited **exact skill names** to preload at session start. No aliases, globs, or semantic lookup; unknown names fail individually. |
| `OCTOMIND_CAPABILITIES` | Comma-delimited **exact installed capability names** to force-enable at session start. No provider/tool aliases or fuzzy matching; domain and required-environment gates still apply. |
| `OCTOMIND_API_URL` | Read when making account/device-login or telemetry calls; default `https://api.octomind.run`. Overrides the control-plane base URL, not model routing. |
| `OCTOMIND_PANEL_URL` | Read when constructing login links; replaces the prefix before `/app` with the configured panel URL. Unset, or URLs without `/app`, remain unchanged. |
| `OCTOMIND_MEDIA_ROOT` | Read per attachment resolution; directory the WebSocket server searches for exactly one attachment file whose name starts with `<id>.`. Default: `/home/octo/.octomind/media`. See [WebSocket Server](../integration/01-websocket-server.md). |
| `OCTOMIND_SHARE_URL` | Read when sharing/analyzing; base URL of the web viewer used by `/share` (upload endpoint) and `/analyze` (viewer link). Defaults to `https://octomind.run`. Override only when pointing at a self-hosted instance or a local dev server. |
| `OCTOMIND_TELEMETRY` | Read at telemetry initialization. Set to `0`/`false`/`off`/`no` to disable anonymous usage telemetry for this run, or to any other value to force it on regardless of the config. Unset = follow `telemetry` in the config (default on). See [Telemetry](#telemetry). |
| `DO_NOT_TRACK` | The cross-tool opt-out standard ([consoledonottrack.com](https://consoledonottrack.com)). Read at telemetry initialization. Any trimmed value other than empty/`0`/`false` (case-insensitive for `false`) disables telemetry, and is honoured **before** `OCTOMIND_TELEMETRY` and the config. |
| `RUST_LOG` | Tracing filter (standard `tracing`/`env_logger` syntax, e.g. `RUST_LOG=debug` or `RUST_LOG=octomind=debug`). At logging initialization in CLI mode (including `run --daemon`), setting it turns on the stderr tracing subscriber (unset = only the colored log macros, no tracing emitted). In ACP/WebSocket modes it overrides the `log_level`-derived filter for the per-mode debug log file. |

```bash
OCTOMIND_DATA_DIR=/tmp/octomind-demo octomind config
OCTOMIND_CONFIG_PATH=./octomind-config/config.toml octomind config --validate
RUST_LOG=octomind=debug octomind run
OCTOMIND_API_URL=http://127.0.0.1:8000 OCTOMIND_PANEL_URL=http://127.0.0.1:5173 octomind login --no-browser
OCTOMIND_MEDIA_ROOT=/srv/octomind/media octomind server
OCTOMIND_SHARE_URL=http://127.0.0.1:5173 octomind run
```

For preloading, select actual installed names from session `/skill` and capability discovery. For example, if
`code-review` and `filesystem` are installed and eligible:

```bash
OCTOMIND_SKILLS=code-review OCTOMIND_CAPABILITIES=filesystem octomind run developer:general
```

## Telemetry

Octomind reports anonymous usage so the CLI can be shaped by evidence rather than guesses. It is **on by default and
prints nothing** — turn it off with `DO_NOT_TRACK=1`, `OCTOMIND_TELEMETRY=0`, or `telemetry = false` in the config.
Opting out is local and instant: no request is made to announce it, and nothing is buffered.

**What is sent** — a `start` row per invocation, a `session` row from instrumented session/workflow exit paths, and an
`error` row when a command fails:

- subcommand name and the long flag **names** used (never their values)
- CLI version, OS, architecture, install source (brew/cargo/docker/source/binary)
- whether the run is interactive, in CI, signed in, or a first run
- session shape: agent tag, provider and model id, duration, turns, tool-call
  count, token counts, cost, compression count, MCP server count, outcome, resume/sandbox flags, and
  how many times you interrupted it with Ctrl+C
- per-tool call **counts**, per-tool failure **counts**, and per-slash-command
  **counts**. Built-in tool names are sent as themselves; every other (MCP) tool
  is reduced to a fixed category such as `ext:github`, because MCP tool names
  come from your config
- provider failure **counts** by fixed kind (`rate_limit`, `overloaded`, `auth`,
  `context_length`, `server`, `timeout`, `network`) — counts only, never the
  provider's message
- for `octomind workflow`: the workflow's declared `name` (the label inside the
  file, never the path it was loaded from), step count and totals
- a random local install id, generated on your machine, tied to no identity. If
  you are signed in, the event is attributed to your account.

**What is never sent** — your code, prompts, model responses, file paths, tool arguments, shell commands, environment
values, repository names or remotes, and error messages. Failures are reported only as a fixed slug (`network`,
`timeout`, `io`, `parse`, `other`).

The exact wire keys are:

| Object | Keys |
|---|---|
| Batch | `v`, `machine_id`, `version`, `os`, `arch`, `install`, `events` |
| Event identity | `name`, `ts`, `command`, `flags`, `kind`, `agent`, `provider`, `model`, `outcome`, `error_kind` |
| Event measurements | `duration_ms`, `turns`, `tool_calls`, `tokens_in`, `tokens_out`, `tokens_cached`, `tokens_reasoning`, `cost_micro`, `compressions`, `mcp_servers`, `cancels` |
| Event counts | `tools`, `tool_errors`, `commands`, `api_errors` |
| Event state | `resumed`, `sandbox`, `tty`, `ci`, `signed_in`, `first_run` |

Empty/default optional fields are omitted; cost is integer micro-USD.

Everything transmitted is a named field on a struct in `src/telemetry.rs`; the declared agent/model/workflow identifiers
are strings. Local supervisor statistics and learning-pack counters are not part of that wire schema. Events are
buffered in memory and sent once at exit behind a 2-second timeout — flush can add up to that timeout to exit, and
failures are only debug-logged.

`complete` and `distill` skip telemetry initialization. WebSocket has no dedicated session-exit event. Start rows also
read `CI` (truthy by the same rule as `DO_NOT_TRACK`) and `GITHUB_ACTIONS` (any present Unicode value, including empty);
these set the `ci` field and do not change session operation.

```bash
DO_NOT_TRACK=1 octomind run
OCTOMIND_TELEMETRY=0 octomind run
```

## Installation Script

Variables used by `install.sh` for automated/CI environments.

| Variable | Description |
|----------|-------------|
| `GITHUB_TOKEN` | GitHub API token for authenticated installation requests |
| `GH_TOKEN` | Alternative token variable (GitHub CLI convention) |
| `OCTOMIND_INSTALL_DIR` | Override installation directory (default: `~/.local/bin/`) |
| `OCTOMIND_VERSION` | Install a specific version instead of latest |

```bash
OCTOMIND_INSTALL_DIR="$HOME/.local/bin" bash install.sh
```

Run from a source checkout; `GITHUB_TOKEN`, `GH_TOKEN`, and `OCTOMIND_VERSION` are user-supplied values when needed.
These are installer inputs, not reads in the Rust runtime.

## OpenRouter-Specific

These attribution headers control how OpenRouter identifies and ranks the app.

| Variable | Default | Description |
|----------|---------|-------------|
| `OPENROUTER_APP_TITLE` | `"Octomind"` | Application title sent to OpenRouter |
| `OPENROUTER_HTTP_REFERER` | `"https://octomind.run"` | HTTP referer sent to OpenRouter |

You normally do not set these yourself: Octomind auto-sets them to the listed defaults at startup (during the `.env`
load step, which runs unconditionally even when no `.env` file is present) **only if they are not already defined**.
Export your own value to override the default.

```bash
OPENROUTER_APP_TITLE="My Octomind setup" OPENROUTER_HTTP_REFERER="https://example.org" octomind run
```

## Template Variables

Role prompt placeholders and `octomind vars` are documented in [Config
Reference](03-config-reference.md#template-variables); they are not process environment variables.

## Webhook Hook Environment Variables

Available to hook scripts when processing incoming webhooks.

| Variable | Description |
|----------|-------------|
| `HOOK_NAME` | Name of the hook that triggered |
| `HOOK_METHOD` | HTTP method, always `POST` because other methods are rejected before the script runs |
| `HOOK_PATH` | Request path |
| `HOOK_QUERY` | Query string |
| `HOOK_CONTENT_TYPE` | Content-Type header value |
| `HOOK_SESSION` | Session name the hook is attached to |
| `HOOK_HEADER_*` | Each HTTP header as `HOOK_HEADER_<NAME>` (uppercased, hyphens to underscores) |

For a webhook processing script, the raw body is stdin. This executable body prints a message to inject:

```bash
#!/bin/sh
printf 'Webhook %s received at %s\n' "$HOOK_NAME" "$HOOK_PATH"
cat
```

Configure its path with `[[hooks]].script`; see the runnable setup in [Config Reference](03-config-reference.md#hooks).

## Local Tool and Guardrail Script Variables

These are child-process contracts set by Octomind, not startup configuration:

| Variable | Script surface |
|----------|----------------|
| `OCTOMIND_TOOL_NAME` | Project-local tool name |
| `OCTOMIND_PARAM_<NAME>` | One local-tool parameter, with the parameter name uppercased; complex values are JSON strings |
| `OCTOMIND_WORKDIR` | Local tools, pipes, hooks, validators, and monitors; current session workdir |
| `OCTOMIND_ROLE` | Guardrail pipes and validators; current role |
| `PIPE_NAME`, `PIPE_RUN_COUNT`, `SESSION_MESSAGE_COUNT` | Guardrail `[[pipe]]` identity and per-session counters |
| `OCTOMIND_CAPABILITY`, `OCTOMIND_TOOL`, `OCTOMIND_SUCCESS` | Guardrail `[[hook]]` call metadata; success is `1` or `0` |
| `OCTOMIND_VALIDATOR` | Guardrail `[[validator]]` name |
| `OCTOMIND_MONITOR_ID` | Built-in monitor command identifier |

Use exported metadata inside the corresponding script, for example a local tool:

```bash
#!/bin/sh
printf 'Tool %s in %s\n' "$OCTOMIND_TOOL_NAME" "$OCTOMIND_WORKDIR"
```

MCP stdio children inherit the process environment; explicit server `env` entries override inherited values. See [MCP
Tools](../usage/07-mcp-tools.md).

## Dynamic environment names

| Surface | When read / override behavior |
|---|---|
| Tap `{{ENV:KEY}}` placeholders | During agent resolution, read the named variable; unset values may be requested interactively, appended to launch-directory `./.env`, and stored in the process; an explicitly empty value is accepted. |
| MCP stdio `command`, `args`, `env`, or HTTP `headers` containing `{{ENV:KEY}}` | Resolve from parent environment before spawn/connect. Missing/empty dependencies gate capability activation. Explicit child entries override inherited values. |
| Skill `env(KEY)` / `env(KEY=value)` rule predicates | During rule matching, compare the named variable to its specified exact value, or require nonempty presence when no value is specified. |
| Credential source inspection | `EnvTracker` snapshots all Unicode environment entries before dotenv and reads requested keys for status; it does not define another fixed list of settings. |

```toml
[[mcp.servers]]
name = "private-api"
type = "http"
url = "http://127.0.0.1:9000/mcp"
headers = { Authorization = "Bearer {{ENV:PRIVATE_API_TOKEN}}" }
timeout_seconds = 30
tools = []
auto_bind = ["developer:general"]
```

Set `PRIVATE_API_TOKEN` to your server’s credential before connecting; the key name is chosen by this config.

## Runtime and Platform Variables

| Variable | Effect |
|----------|--------|
| `XDG_RUNTIME_DIR` | On Unix, places session sockets/PID files under `$XDG_RUNTIME_DIR/octomind`; otherwise Octomind uses a per-user system temporary directory. |
| `KITTY_WINDOW_ID` | Signals Kitty graphics support for inline image display. |
| `TERM` | A value containing `kitty` also selects the Kitty inline-image protocol. |
| `TERM_PROGRAM` | `ghostty`/`WezTerm` select Kitty graphics; `iTerm.app`/`Tabby`/`vscode` select the iTerm2 image protocol. |
| `SHELL` | Read for prompt system information; unset displays `unknown`. It does not choose the tap dependency runner. |
| `ProgramFiles`, `ProgramFiles(x86)` | Windows dependency-script startup probes `Git/bin/bash.exe` under these directories before falling back to `bash` on PATH. |
| `HOME`, `LOCALAPPDATA` | Directory-library platform inputs for default data/home paths; `HOME` is also directly saved/restored in skill tests. |
| `TMPDIR` | Platform temporary-directory input on macOS/Unix; affects runtime files and extraction snapshots through `std::env::temp_dir()`. |
| `PATH` | Executable lookup by subprocesses/dependencies; directly saved/restored by source tests and read by the CLI provider. |

These inputs are read when resolving a directory, executable, prompt, or image protocol, not as TOML overrides.
`KITTY_WINDOW_ID` presence wins over `TERM`, which wins over `TERM_PROGRAM`. There is no Octomind `HF_HUB_CACHE` or
`HF_HOME` read in `src/`; the locked octolib Hugging Face loader sets `HF_HOME` to its shared model cache. Do not rely
on those old documented names to relocate Octomind’s embedding weights. Model downloads are separate from the vector
cache under `OCTOMIND_DATA_DIR/cache/embeddings`.

```bash
XDG_RUNTIME_DIR=/tmp octomind run --name runtime-demo
```

On Unix this uses `/tmp/octomind` with private-directory ownership/permission checks.

## Build and test-only variables

These are present in `src/` but are not production CLI settings. Setting them does not run a benchmark or test.

| Variable | When read / default / purpose |
|---|---|
| `CARGO_PKG_VERSION` | Compile-time `env!`; embedded in CLI version, branding, ACP/MCP implementation info, account client info, and telemetry. Cargo supplies it. |
| `CARGO_MANIFEST_DIR` | Compile-time `env!` in condenser tests; locates repository fixtures. |
| `LEARNING_BENCH_LIVE` | Ignored live learning benchmarks require exactly `1` before paid model calls. |
| `LEARNING_BENCH_MODEL` | Live retrieval/retention benchmarks; overrides the configured supervisor model name. |
| `LEARNING_BENCH_SPLIT` | Retrieval benchmark split: `calibration` (default), `holdout`, `challenge`, or `all`. |
| `LEARNING_BENCH_REWRITE_CACHE` | Retrieval benchmark cache path; default `target/learning-benchmark/rewrite-cache.json`. |
| `LEARNING_BENCH_REPORT` | Retrieval benchmark report output; default `target/learning-benchmark/{split}.json`. |
| `LONGMEMEVAL_ORACLE_JSON` | Required path to the oracle dataset in the ignored LongMemEval benchmark. |
| `LONGMEMEVAL_EXPECTED_SHA256` | Optional override of the benchmark’s pinned dataset checksum. |
| `OCTOMIND_LEARNING_REPLAY_SESSION` | Required real JSONL/ZST transcript path in ignored extraction replay tests. |
| `OCTOMIND_ACP_TEST_FIXTURE` | Nonempty marker selects the child-process ACP fixture; normally absent. |
| `OCTOMIND_RESOLVER_PROBE` | Temporary resolver test marker; saved/restored to verify dependency isolation. |
| `GIT_DIR`, `GIT_WORK_TREE` | Saved/restored in supervisor tests to isolate Git checks; not Octomind config overrides. |

Test environment guards also read/save/restore arbitrary test-selected names, including the runtime/provider variables
above. Those fixture names and values do not add production configuration options.

```bash
export LEARNING_BENCH_SPLIT=holdout
export LEARNING_BENCH_REPORT=/tmp/octomind-learning-report.json
```

## Common questions

- **Why did an exported key lose?** Both `.env` files override exports. Check the project file last; even an
  empty assignment masks the earlier value.
- **Why does a custom config path not load its `.env`?** User dotenv loading always uses the standard shared
  config directory under the data root. `OCTOMIND_CONFIG_PATH` only selects TOML load/save behavior.
- **Why does Bedrock say a key is missing despite config showing AWS set?** The status row checks
  `AWS_ACCESS_KEY_ID`; the provider requires `AWS_BEARER_TOKEN_BEDROCK`.
- **Why does OAuth fail before the first request?** CLI preflight requires `get_api_key()`; see the OAuth caveat.
- **Why does disabling telemetry still leave local files?** Telemetry opt-out does not disable session logs,
  local accounting, compression archives, or learning storage.

## Source map

Direct reads: [env_source.rs](../../src/config/env_source.rs), [loading.rs](../../src/config/loading.rs),
[directories.rs](../../src/directories.rs), [account.rs](../../src/account.rs), [telemetry.rs](../../src/telemetry.rs),
[client.rs](../../src/mcp/client.rs), [skill.rs](../../src/mcp/runtime/skill.rs),
[capability.rs](../../src/mcp/runtime/capability.rs). Provider delegation/preflight:
[providers.rs](../../src/providers.rs), [setup.rs](../../src/session/chat/session/setup.rs). Provider names and
variables were checked in the locked `octolib 0.35.0` source, `src/llm/factory.rs` and `src/llm/providers/*.rs`
(including `cli/mod.rs`); model-cache behavior is in its `src/embedding/provider/huggingface/mod.rs` and
`src/storage.rs`.

## See also

- [CLI Reference](01-cli-reference.md)
- [Session Commands](02-session-commands.md)
- [Config Reference](03-config-reference.md)
- [Providers Guide](../usage/04-providers.md)
