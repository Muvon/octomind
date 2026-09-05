# Put spending checkpoints around each task

Set request and session spending checkpoints so every agent task has a visible, predictable cost.

## The problem

You start a small task and leave the agent working, then find that retries and tool follow-ups cost more than you
expected. A single account balance does not tell you which request caused the increase. You need a request threshold,
a session checkpoint, and a way to compare the costs of individual tasks.

## What you will set up

- [Spending thresholds and configuration inspection](../reference/03-config-reference.md).
- [Per-role model profiles](../usage/06-roles.md) and [per-tap model mappings](../integration/04-tap-system.md).
- [`/usage`, `/info`, and `/report`](../reference/02-session-commands.md) for account and session accounting.
- [Cache keepalive settings](../usage/08-compression.md) that avoid idle pings in this example.
- A [local tool](../usage/17-local-tools.md) for a small, observable tool-follow-up checkpoint.

## Prerequisites

Use a Unix shell with Python 3.11 or newer. The example uses `octohub:auto` and your existing Octomind login.
Git is needed when resolving a tap for the first time. Check each prerequisite:

```bash
octomind --version
octomind login
python3 --version
git --version
```

An existing login reports that you are already signed in. No external MCP server is required: you will create the
only project tool used in the walkthrough.

## Steps

### 1. Create a separate configuration for the exercise

Keep this terminal open through the steps. The environment override selects a complete generated configuration
under the demo project; it does not move your saved login. Configuration files in that directory merge together.

```bash
mkdir "$HOME/octomind-spend-demo"
cd "$HOME/octomind-spend-demo"
mkdir -p .agents/tools spend-tap/agents/spend
export OCTOMIND_CONFIG_PATH="$PWD/.octomind-config/config.toml"
octomind config --validate
octomind config --show
```

### 2. Set the checkpoints and model profiles

Save the entire block as `.octomind-config/90-budget.toml`. Root keys must precede every table header.
The USD values below are exercise thresholds, not estimates of what a particular task will cost.

The session threshold asks whether to continue after the accumulated cost since its last checkpoint reaches $1.
Accepting resets that checkpoint. The request threshold stops further execution at a checked boundary after $0.10
has accrued since the current user request began. Setting either threshold to `0.0` disables it.

```toml
max_session_spending_threshold = 1.0
max_request_spending_threshold = 0.10
cache_keepalive_enabled = false
cache_keepalive_max_idle_seconds = 1800
auto_capabilities = false

[model]
name = "octohub:auto"
reasoning_effort = "medium"
max_tokens = 1024
temperature = 0.3
top_p = 0.7
top_k = 20
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300

[taps]
"spend:brief" = "octohub:auto"

[supervisor]
enabled = false

[supervisor.learning]
enabled = false

[[roles]]
name = "budget"
system = "Answer briefly. When asked to call budget_probe, call it once and report its result."
welcome = "Budget exercise ready."

[roles.model]
name = "octohub:auto"
reasoning_effort = "medium"
max_tokens = 512
temperature = 0.3
top_p = 0.7
top_k = 20
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300

[roles.mcp]
server_refs = ["core"]
allowed_tools = ["core:*"]
```

### 3. Add a tap agent with an independent model mapping

Save `spend-tap/agents/spend/brief.toml`. This is a local tap you author, so no unpublished remote agent is assumed.
Its manifest omits a role model profile: the `[taps]` mapping above selects its model name, and it inherits the main
profile's other parameters. The plain `budget` role instead has its own 512-output-token profile.

```toml
[[roles]]
name = "spend_brief"
system = "Give concise answers. Use budget_probe once when the user requests it."
welcome = "Brief-answer exercise ready."

[roles.mcp]
server_refs = ["core"]
allowed_tools = ["core:*"]
```

Register the tap, then validate. A runtime `--model` override wins over role selection; a role model name wins over
the tap mapping; the tap mapping wins over the main model name. Both examples use the logged-in gateway here.

```bash
octomind tap tutorial/spend ./spend-tap
octomind config --validate
```

### 4. Install the checkpoint probe

This script has no external dependencies. Its filename becomes the tool name under the synthetic `local` server.
The first model request can ask for this tool; processing its result reaches the spending checks before another
model call.

```bash
cat > .agents/tools/budget_probe <<'EOF'
#!/usr/bin/env python3
# @description Return a fixed marker for the spending checkpoint exercise.

print("PROBE OK")
EOF
chmod +x .agents/tools/budget_probe
.agents/tools/budget_probe
octomind run budget
```

### 5. Compare account usage with individual requests

Send each line separately and wait for the answer before the next line. `/usage` fetches your Octomind account's
spending and quotas; it is not a per-session total and does not summarize bills from independent provider accounts.
`/info` includes session cost and tokens. `/report` reads the session log and groups costs and tool use by request.

Look for the two requests in the report. Exact dollar amounts depend on actual provider accounting; no fixed amount
is expected. Keepalive is disabled, and supervisor and learning calls are disabled for this isolated comparison.

```text
/mcp full
/usage
Call budget_probe once, then report its result in one sentence.
Explain the difference between a request threshold and a session threshold in two sentences.
/info
/report
/exit
```

### 6. Observe a low request threshold

Save `.octomind-config/99-probe.toml`. Its later filename overrides the request threshold from step 2.
This tiny positive threshold makes the first charged tool round a candidate for stopping before its follow-up.
It does not make the first model call free or cap that call in advance.

```toml
max_request_spending_threshold = 0.000000001
```

Start a fresh session so the new configuration loads:

```bash
octomind config --validate
octomind run budget
```

If the first call has a positive recorded cost and calls the tool, look for
`REQUEST SPENDING THRESHOLD EXCEEDED` and the cancelled tool-follow-up notice. A text-only response or a zero-cost
record does not establish that this checkpoint fired. Inspect the tool schema and retry the explicit tool request.

```text
Call budget_probe exactly once. Wait for its result before answering.
/report
/exit
```

### 7. Restore the working request threshold

Replace `.octomind-config/99-probe.toml` with this complete root-level override document:

```toml
max_request_spending_threshold = 0.10
```

Run the tap agent for one non-interactive task. The optional argument is a tag; the message arrives through stdin.

```bash
printf '%s\n' 'Call budget_probe once and report its result.' | octomind run spend:brief --format plain
```

## Verify it works

Validate the merged configuration, then inspect the saved settings and startup summary:

```bash
octomind config --validate
python3 -c 'import tomllib; print(tomllib.load(open(".octomind-config/99-probe.toml", "rb")))'
octomind config --show
```

Look for a valid configuration and `max_request_spending_threshold` equal to `0.1` in the Python output.
`config --show` is a selected summary, not a full TOML dump: it does not print every budget or keepalive field.
With a custom config path, its displayed path can still be the default path; keep using the exported path above.
The threshold notice from step 6 is the behavioral check, while `/report` supplies the observed request costs.

These thresholds use costs already recorded at checkpoints. In-flight requests can overshoot, and a single final
answer need not reach a tool-follow-up check. They are not prepaid per-task reservations. Detached exit learning and
independent external services also do not become part of a shared hard ceiling.

## Variations

- With your own OpenAI credential, check `test -n "$OPENAI_API_KEY"` and use `openai:gpt-5.6-sol` as the role model
  name or the `"spend:brief"` mapping. Follow [provider setup](../usage/04-providers.md); compare measured reports.
- Enable supervision for tasks that need completion checks, then include its calls in your spending comparison.
- Enable `cache_keepalive_enabled = true` only when you want supported-provider idle refreshes. Keep a finite
  `cache_keepalive_max_idle_seconds`; `0` removes that idle cutoff. Providers without a keepalive policy are skipped.

## Troubleshooting

**The bill exceeds the threshold.** The check uses accrued cost, not a quote for the next request. Lower the threshold,
reduce output limits, and inspect tool follow-ups and separate services. Do not use it as a provider billing ceiling.

**The tap uses a different model than expected.** Check the resolved role with `/role` and `/info`. A manifest's role
model profile can override `[taps]`, and `--model` can override both. Tap mappings accept model-name strings only.

**There is no account usage.** Run `octomind login` again if `/usage` reports that you are not signed in. Independent
provider keys can run sessions without an Octomind account, but their account bills are outside this command.

**Keepalive produces no pings.** It requires an eligible provider, a usable conversation snapshot, and enough idle
time before the finite cutoff. Enabling it does not guarantee a cache hit or savings; compare actual session reports.

## See also

- [Configuration reference](../reference/03-config-reference.md)
- [Roles](../usage/06-roles.md)
- [Tap system](../integration/04-tap-system.md)
- [Session commands](../reference/02-session-commands.md)
- [Compression and cache keepalive](../usage/08-compression.md)
- [Local tools](../usage/17-local-tools.md)
- [Providers](../usage/04-providers.md)
