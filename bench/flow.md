# Long-run efficiency campaign — flow log

Goal: make octomind maximally efficient on long-running (multi-turn, same-session) coding tasks.
Method: measure in Docker with a cheap model (Alibaba `deepseek-v4-flash` / `glm-5.3`), read the
logs, find the highest-leverage inefficiency, fix it, re-measure. Every step is logged here so the
next person (or agent) can continue without re-discovering anything.

Rules for this campaign:
- Everything runs in isolated Docker containers (octobench `--executor docker`), never on the host.
- Provider keys/URLs come from the host env by NAME only (forwarded into the container); never read values.
- Bench retries provider hiccups; octomind stays a clean single shot under measurement.

## 2026-09-22 — Step 0: reconnaissance
- HEAD = `4bffaab3` (0.54.1). The prebuilt bookworm binary `target-bookworm/release/octomind` was 0.52.0 (stale)
  → rebuilding HEAD in `rust:bookworm` with `CARGO_TARGET_DIR=target-bookworm` (cache reused). Output: `target-bookworm/octomind-head`.
- Instrument: `../octobench` (`cli.longrun`, `providers/octomind.py`, `docker/Dockerfile.agent.head`, image `octobench-agent:head` built 2026-09-12).
- Cheap models available in the registry (`configs/models.yaml`): `alibaba:deepseek-v4-flash-0731` ($0.14/$0.28), `zai:glm-5.3-flash` ($0.15/$0.5), `zai:glm-5.3` ($1.4/$4.4). Env has `ALIBABA_API_KEY/URL`, `ZAI_API_KEY/URL`, `OLLAMA_API_URL`.
- Untracked `specs/001..005` are un-implemented planning docs about evaluation gates / condense / fold-prune / distill-plan gates — context for what was considered before; not a mandate.

### Bench mechanics (from `../octobench`)
- One container per sequence (`octobench-agent:head`, only image built); each turn = `docker exec … octomind run developer --name lr-… --model <provider_model> --format=jsonl [-r <name>]`, prompt on stdin. Session resumes with `-r`.
- `OCTOMIND_BIN` mounts a host binary over `/usr/local/bin/octomind`; `OCTOBENCH_OCTOMIND_CONFIG` mounts the config at `/cfg/octomind.toml`; `OCTOMIND_DATA_DIR=/octobench-state/octomind` is bind-mounted to `<out>/<ts>/<seq>/octomind__<model>/state/octomind/` (sessions `*.jsonl.zst`, archive, learning) → readable from host after the run.
- Per-turn logs: `<run>/turns/turn_N/logs/provider.{stdout,stderr}.log`, `provider.raw.jsonl`, `validate.*`. Tokens = diff of the last `type=cost` JSONL record between turns.
- Sealing: default-deny iptables around the agent phase; allowed hosts include the Alibaba token-plan endpoint. `enforce_clean_bench` requires `OCTOBENCH_SEAL_NETWORK=1 OCTOBENCH_SYSTEM_PROMPT=configs/common/system_prompt.md OCTOMIND_AGENT=developer`.
- Debug logging inside the container: `log_level="debug"` in the config AND `RUST_LOG=debug` (jsonl mode suppresses the coloured log macros; RUST_LOG routes them to stderr). Grep keys: "Fold decision:", "Adaptive compression fire line", "Computed compression depth", "Cut N oversized tool result".
- Campaign config: `configs/octomind/octomind.toml` with compression+supervisor models switched from openrouter to `alibaba:deepseek-v4-flash-0731`, `log_level=debug` (copy kept outside the repo; see step 1 for the path used).

### Architecture facts that matter for long runs (verified against source)
- Compression fire line: `[compression] threshold=70000` absolute tokens; `decision.rs` folds ALWAYS at a turn boundary once above the line ("free" fold), mid-turn only if a cache-amortisation check passes. Depth = 2x..16x ratio computed from growth rate; forced 16x inline fold when `current + 5×growth ≥ ceiling`.
- Kept across a fold: preamble, live exchange (from last assistant msg), summary JSON, continuation wrapper, last 4 user tasks, active skills, recall ≤8k, `<file_context>` ≤8k, critical knowledge (25), analysis findings (6k). Drained messages archived to `sessions/archive/<session>/`.
- Token counting = tiktoken cl100k over the FULL context, re-tokenized on every call (`core.rs get_full_context_tokens`), called 3-5× per tool round; `trim_oversized_tool_results` re-tokenizes every tool message per check.
- Tool output cap `mcp_response_tokens_threshold` (bench: 8000, default 20000) keeps the HEAD only (tail with test summaries lost inline; spill file kept). Applied 3× per result.
- Dedup (`session/dedup.rs`): identical (tool,args,content) ≥500 chars → second call replaced with an ERROR placeholder. Suspicious for weak models re-reading a file legitimately.
- Supervisor per-turn cost: resolve 1-2 blocking calls per user turn; condense 1 blocking call (≤60 s) per tool round with a >tokens_threshold text result; gate 1-2 calls + up to 2 agent re-iterations per `done`; plan 1 call per signal. All on `[supervisor.model]`.

### What the previous campaigns already tell us (octobench GOLD long-run, glm-5.3(-flash), Sept 2026)
- Pass rate octomind ≈ opencode (66/75 flash, 67/75 glm-5.3) — but the flash column keeps the better of two runs on 3 sequences; first-run flash was 61/75. So correctness, not just tokens, is a lever.
- Cost/pass: octomind ~½ of opencode (opencode never compacts: eslint prompt grows 7k→245k over 15 turns). But octomind's published cost EXCLUDES the compression/supervisor model (bench bills main-model tokens only); duckdb session $4.48 real vs $3.41 reported (10 folds = 1.05M fold-input tokens, $0.82).
- Octomind context is a sawtooth that still climbs well past the 70k fire line (cli11 avg ctx/call 18k→96k, eslint 23k→145k, duckdb 165k) because folds FAIL or are DECLINED:
  - `Background fold call failed … Schema validation failed: provider 'openrouter' returned invalid or unparseable final structured output` — 34/90 flash turns, 55/96 glm turns
  - `Background fold cancelled before compression could be applied` — 24 / 30
  - `Compression not applied: decision model declined (force=false)` — 59 / 44
  → uncached-input share 2.8% (octomind) vs 0.5% (opencode): every landed fold breaks the prompt cache, every failed one lets the context grow.
- Agent-side waste: 351 of 785 `view` calls re-read a path already viewed in the same turn (mypy/checker.py 13× in one turn). Exact duplicate tool calls are rare (1.8%). Large outputs are NOT the cost driver; accumulation is.
- Failure taxonomy: wrong/incomplete fix while claiming tests pass (most common); rabbit-holing into /tmp probe scripts then provider timeout (eslint t12, 90 tool calls, no production edit); provider empty responses/timeouts.
- Learning: `packs = 0` in 15 of 21 sessions; `recall` called 3× in 3,749 tool calls → inert under this bench.

**Leverage ranking (initial):** (1) fold reliability + fold economics [compaction], (2) agent re-reading / context accumulation, (3) bench cost fidelity for compression tokens, (4) supervisor blocking latency. Learning is not a bench lever right now.

## 2026-09-22 — Step 1: baseline measurement (HEAD 0.54.1, deepseek-v4-flash via Alibaba)
- Build: `rust:bookworm` + `protobuf-compiler` + csukuangfj `onnxruntime-linux-x64-static_lib-1.24.2-glibc2_17` via `ORT_LIB_LOCATION` (a plain `cargo build` in the container fails to link `ort_sys` — the bench README recipe is incomplete; the CI download step is the fix). Binary snapshot kept as `octomind-base-0.54.1` (50 MB, static ORT).
- Gotcha: the bench config was v16; HEAD (v17) tries to rewrite the read-only `/cfg/octomind.toml` → every turn fails with `Device or resource busy`. Upgraded the campaign config inside Docker with `octomind config --upgrade` (v17 adds `[supervisor.evaluate] capabilities = false`). `configs/octomind/octomind.toml` in octobench needs the same bump before any HEAD run.
- Run A (baseline): `cases/dev/longrun/python/pytest`, matrix `deepseek-v4-flash` / `alibaba:deepseek-v4-flash-0731`, all models (main, compression, supervisor, judge) on the same cheap model, `RUST_LOG=octomind=debug`, sealed Docker. Out: `../octobench/results-lr-base-pytest-dsv4`.

## 2026-09-22 — Step 2: two compaction bugs found by reading the fold lifecycle (branch `feat/longrun-efficiency`)
While the baseline runs, code reading of `conversation_compression/mod.rs`, `session/cancellation.rs`, `main_loop.rs` and octolib 0.39 `llm/retry.rs` explains two of the three top fold-failure messages from the September campaigns:
1. **Background folds die at every turn boundary.** The spawned fold task used the per-operation cancellation receiver. `SessionCancellation::new_operation()` swaps the channel and DROPS the old sender at every boundary (next user prompt in interactive mode; every inbox round — e.g. a `monitor` completion — in piped mode). octolib's `cancellable()` treats `token.changed() == Err` (sender gone) as a cancel → the HTTP request is aborted → `Background fold cancelled before compression could be applied` (24–30× per campaign) and the paid fold is wasted, cooldown starts, context keeps growing.
   Fix: the `FoldJob` owns its own `watch::Sender` (dropping the job is the only cancel). `mod.rs FoldJob.cancel`.
2. **A one-shot run exits with the fold in flight.** `settle_pending_fold` at turn end only applies an already-finished fold; the piped loop then breaks and the process exits — the in-flight summary is lost. In the bench every turn is a new `octomind run -r` process, so a fold that did not finish inside the turn NEVER lands and the next turn re-pays it (the sawtooth to 145k–165k).
   Fix: `collect_fold_before_exit()` — non-daemon piped exit waits for the fold, bounded by the fold request's own budget (timeout × attempts + retry pauses), applies it, then saves. Daemons/interactive unchanged.
Tests: `compression_e2e_tests.rs` `verify_background_fold_survives_the_operation_sender_being_dropped`, `verify_exit_collects_a_running_fold_within_the_request_budget`.
Not yet explained: `Compression not applied: decision model declined (force=false)` (59×) — waiting for the baseline logs.

## 2026-09-22 — Step 3: baseline turn-1 data (pydantic, deepseek-v4-flash) and the estimator bug
Pydantic turn 1: PASS, 24 min, 129 tool calls / 108 assistant messages, 4.11M tokens (3.87M cache-read, 131k uncached input, 16k output), $0.24. Supervisor: 1 resolve + 1 condense call ($0.006, 2.5%). One fold spawned at estimated 74k; landed 2m39s later ("70 msgs → 22053 tokens saved, 37.6%"), $0.013.
Where the bytes are (stored session): thinking 457k chars, tool results 187k, assistant text 63k, system 28k. `view` = 149k of the 187k tool chars; 79 views over 24 paths, 55 same-path re-reads (different ranges). Analyzer gotcha: the session log re-appends the surviving messages after every `COMPRESSION_POINT`, so a naive parse shows phantom duplicate tool calls — dedupe by tool_call id (done in `analyze_run.py`); with that, byte-identical repeats are ~2 per sequence and the dedup placeholder works.

**Estimator vs provider (per API call, from `Provider usage` lines):**
| time | estimated context | actual prompt (input+cache_read) |
|---|---|---|
| 17:17 | 9.8k | 10.9k |
| 17:21 | 42k | 27.7k |
| 17:26 (fold fired) | 74k | 32.4k |
| 17:29 (after fold) | 57k | 37.3k → 31.7k |
| 17:36 | 102k | 61k |
Root cause (verified in octolib 0.39 request builders): `estimate_message_tokens` counts every stored assistant `thinking` block (as escaped JSON), but only Z.AI (all history), DeepSeek-native (tool requests) and Kimi K2.6+/K3 on Moonshot/Ollama ever replay reasoning; Alibaba/OpenAI-compat/Anthropic/OpenRouter/MiniMax/OctoHub drop it. In an agentic turn the reasoning is 2–3× the transcript, so the fire line (70k) is crossed at ~32k real tokens, the fold "saves" text the provider never saw (37.6% claimed, ~14% real: 36.9k→31.7k) and the fold's cache invalidation (uncached input 1k→23.5k on the next call) eats most of the gain.
Fix: `model_utils::model_replays_thinking(model)` mirrors octolib's policy; `token_counter::estimate_sent_*` variants count thinking only where replayed (and as the bare string, not JSON). All context decisions (fire line/ceiling in `get_full_context_tokens`, pre-flight size check in `completion.rs`, cache checkpoints, fold range/prompt/PACT packet sizing, prompt display) now use the sent size. Follow-up for octolib: expose the replay policy so octomind stops mirroring it.

## 2026-09-22 — Step 4: A/B protocol
- Arms: `octomind-base-0.54.1` (HEAD) vs `octomind-fix1` (HEAD + fold-survives-boundary + collect-fold-before-exit + sent-size estimator). Same campaign config, same model (`alibaba:deepseek-v4-flash-0731` for main/compression/supervisor), same sequences, sealed Docker, one container per sequence.
- Metrics per turn: validate PASS/FAIL (oracle), tokens (session_tokens delta, i.e. input+cache_read+output+reasoning), $, wall time, folds spawned/applied/lost, estimate-vs-actual prompt ratio, avg prompt tokens per API call.
- Runs: `results-lr-base-pydantic-dsv4`, `results-lr-base-pytest-dsv4` (baseline, started 17:15/17:16); `results-lr-fix1-pydantic-dsv4` (started 17:56). Pytest fix1 arm starts when its baseline container finishes (one Alibaba key; keep ≤3 concurrent sessions).
- Caveat: single rep per arm with a nondeterministic agent — treat differences under ~20% as noise; the fold-event counts and the estimate/actual ratio are the deterministic signals.

### Baseline pytest turn 1 (deepseek-v4-flash): the failure chain, end to end
FAIL after 50 min, exit 1 — `Error: context remains above the usable ceiling after compression (211512 > 200000 tokens) … applied folds=0`. Real prompt at that moment: ~70k tokens (input+cache_read). Sequence:
1. est 70.8k (actual 48.6k) → background fold → 5 min later `Provider 'alibaba' returned an empty response (finish_reason=Some("length"))` — the fold model (medium reasoning, `max_tokens=16000` in the bench config) spent its whole output budget thinking. Retried once by the generic empty-completion retry with the SAME params → same failure. ×3 background folds.
2. est climbs to 200k (actual ~62k) → `Context ceiling margin reached` → forced inline 16× fold → same length-cut failure, 2 attempts ≈ 5.5 min each time, ×4 (the agent is blocked meanwhile: 17:45→18:05 is mostly fold waits).
3. `ensure_context_within_ceiling` hard error → turn aborted → validation FAIL.
Also visible: 12 `view` calls with no `path` (model omitted the required field; each is a wasted round trip and the error text is a serde message) and reasoning bursts of 10k tokens (95–130 s per call) at large contexts.
Two octomind fixes follow from this: (a) the estimator (done in fix1) — no spurious ceiling; (b) a fold whose response is length-cut must be retried with reasoning off and a larger output budget, not with identical params (F4, implemented next).

## 2026-09-22 — Step 5: fold repair on length cut (fix F4)
- `completion.rs`: typed `EmptyCompletion { provider, model, finish_reason, attempts }` (callers match on it); the generic empty-completion retry now STOPS on `finish_reason == "length"` — an identical re-send cannot succeed and was costing one full fold (~2.7 min, ~$0.013) per attempt.
- `conversation_compression/ai.rs`: on a length-cut fold response, one repair request with `reasoning_effort = None` and `max_tokens × 2`; a second failure surfaces to the existing cooldown. `decision_params()` builds both requests.
- Tests: `verify_length_cut_fold_is_repaired_once_with_reasoning_off`, `verify_second_length_cut_fails_the_fold_without_identical_retries` (compression_e2e). Full `cargo test --lib`: 3803 passed, 1 failed = `workflow::run::tests::resolve_workdir_makes_relative_paths_absolute`, passes alone (cwd-sensitive under parallel run, unrelated).
- Binary `octomind-fix2` = F1+F2+F3(estimator)+F4.

## 2026-09-22 — Step 6: baseline pydantic complete (HEAD 0.54.1, deepseek-v4-flash)
| turn | verdict | tokens | uncached in | out | notes |
|---|---|---|---|---|---|
| 1 | PASS | 4.11M | 131k | 16k | 129 tool calls; 1 fold landed mid-turn (est 74k, real 32k) |
| 2 | PASS | 0.86M | 79k | 4k | boundary fold spawned, turn ended first → lost |
| 3 | PASS | 3.02M | 74k | 7k | boundary fold length-cut after 6 min; est hit ceiling at real 94k → forced 16× fold (89% of history gone), 2m16s inline |
| 4 | PASS | 0.53M | 53k | 2k | fold spawned, lost at exit |
| 5 | FAIL | 1.18M | 28k | 7k | wrong fix (1 of 17 tests); fold length-cut again (5 min wasted) |
Total 9.69M tokens, 4/5 turns, 61 min wall. Compression: 6 folds spawned, 2 landed (one of them the forced 16×), 3 died on length cut, 1 lost at exit. Estimate/real prompt ratio 1.3–2.3× throughout.

## 2026-09-22 — Step 7: fix2 arms
- 18:19 launched `results-lr-fix2-pytest-dsv4` (binary `octomind-fix2` = F1+F2+F3+F4) against the baseline pytest run still in progress. `results-lr-fix1-pydantic-dsv4` (F1+F2+F3 only) keeps running; its turn 1 is at 25 min vs 24 min for the baseline.

## 2026-09-22 — Step 8: prompt-size self-calibration (F5)
fix1 turn 1 showed the corrected estimator at a steady 0.89–0.90 of the provider's prompt count (cl100k undercounts DeepSeek's tokenizer by ~10%); GLM/others will differ. Each usage report now pairs the provider's prompt tokens (input+cache_read+cache_write) with the estimate of the same messages (`ChatSession::raw_context_estimate`, taken before the reply is appended, on both the initial and follow-up paths) → EMA factor (α=0.5, clamped 0.5–2.0, samples ≥2k tokens) in `prompt_calibration` (runtime-only). `get_full_context_tokens` and `calculate_range_tokens` return calibrated values, so fire line, ceiling margin and fold depth all work in the provider's units after the first call of each process. Tests: `core_methods_tests.rs` `prompt_calibration_scales_context_decisions_into_provider_units`, `raw_context_estimate_needs_cached_tools`.
- Baseline pytest turn 2 (HEAD): FAIL, exit 1 after 47 min — same chain: est 145k–204k vs real 49k (ratio 3–4× in this thinking-heavy session), three forced 16× folds all length-cut, `context remains above the usable ceiling (204056 > 200000)`. Baseline pytest so far 0/2 turns (turn 3 also FAIL by validation).
- 19:06 launched `results-lr-fix3-pydantic-dsv4` (binary `octomind-fix3` = F1–F5) — the definitive pydantic fix arm; `fix1-pydantic` continues as a partial-fix data point. Clippy clean on the final tree.
- Baseline pytest turn 4 (HEAD): FAIL with ZERO agent calls — the boundary check read est 205k (>200k ceiling) on the resumed session, forced fold length-cut twice (5.5 min), hard error. Once HEAD's estimate crosses the ceiling on a resumed session, every following turn dies the same way: the session is wedged.

## 2026-09-22 — Step 9: first fix-arm evidence (fix1 = F1–F3, pydantic)
- Turn 1 PASS (54 min, 100 calls, 6.7M tok): estimate/real 0.89–0.90 all turn (HEAD: 0.9→2.3); the fold fired at est 70.6k = real 78k as intended; it then died on the length cut (fix1 has no F4) — 10 min of fold waiting, no fold landed.
- Turn 2 PASS (21 min, 43 calls, 2.9M tok): boundary fold at est 88k/real 98k landed 4 min later — `261 msgs → 54187 tokens saved (68.2%)`, real prompt 98k → 56k (−43%). With HEAD the same turn lost its fold at exit and turn 3 started at real 77k / est 128k.

## 2026-09-22 — Step 10: baseline pytest complete (HEAD 0.54.1): 0/5
| turn | verdict | fold events (stderr) |
|---|---|---|
| 1 | FAIL (exit 1, 50 min) | 3 folds spawned, 11 length-cut responses, 4 forced 16× folds, hard ceiling error; est/real up to 2.6× |
| 2 | FAIL (exit 1, 47 min) | 1 fold landed, then 7 length cuts, 4 forced folds, hard error; est/real up to 3.1× |
| 3–5 | FAIL (exit 1, ~6 min each, 0 agent calls) | resumed session already reads above the 200k ceiling → forced fold length-cut → hard error before the first agent call |
Bench instrument note: on `exit 1` the harness records `tokens: null`, so the ~20 min of fold calls per crashed turn are unbilled — the baseline's real token spend is understated (known octobench issue).
Test suite on the final tree: `cargo test --lib` 3810+ tests; the only failures (`embeddings::*`, `mcp::runtime::capability::*`, 7 tests) are ONNX-model tests that pass in isolation and passed in the earlier full run — they flaked under the concurrent Docker builds. Clippy clean, fmt clean.

## 2026-09-22 — Step 11: fix2 pytest turn 1 — PASS where HEAD crashed
| arm | verdict | wall | calls | tokens | folds |
|---|---|---|---|---|---|
| HEAD | FAIL (exit 1) | 50 min | ~100 + 20 min of fold waits | unbilled (crash) | 3 spawned, 0 landed, 11 length cuts, 4 forced, hard error |
| fix2 | PASS | 74 min | 120 | 7.80M (7.21M cache read, 220k uncached, 19k out, 359k reasoning) | 1 spawned, 1 landed (86 msgs, 77% est / real 78k→27k), no length cut, no forced fold |
Estimate/real: 0.91–0.93 the whole turn (HEAD 0.9→2.6). The single fold took 8.5 min on deepseek-v4-flash (medium reasoning) — fold latency is now the dominant compaction cost; no F4 repair was needed here. After the fold the ladder doubled the fire line to 140k, so the turn ended at real 84k unfolded (by design). Curiosity: the agent tried to `cd` into `/octobench-state/octomind/sessions/archive/<session>` after the fold — the summary's recall pointer invites the model to read its own archive; 1 wasted call.
- fix1 pydantic turn 3: PASS (26 min, 46 calls, 3.96M tok, est/real 0.91–0.92). Three folds spawned (est 70k, 77k, 85k), ALL length-cut (6 min, 5 min, 2 min wasted). The exit path worked as designed — `Run ending with a background fold in flight — waiting for it (≤610s)` — and collected the third fold's failure 2 min later. Without F4 (fix1 lacks it) deepseek's medium-reasoning fold reliably blows the 16k output budget on 70–85k ranges; fix3 carries the repair. Cumulative fix1 pydantic: 3/3 PASS so far vs HEAD 3/3 at the same point.
- fix1 pydantic turn 4: PASS (5 min, 14 calls). Boundary fold spawned at est 87k/real 95k; the exit wait held the process 2m20s and collected a length-cut failure. Net so far for fix1: F1/F2 work mechanically (folds survive boundaries, exit collects them), but with the 16k fold budget + medium reasoning every fold on ≥70k dies → F4 is the piece that makes them land (fix3 pydantic turn 1 shows a landed fold: 173 msgs, 33.6k saved).

## 2026-09-22 — Step 12: fix3 (F1–F5) pydantic turn 1 — PASS, estimator exact
- `Prompt calibration: estimate=10987 actual=12184 sample=1.11 factor=1.11` after the first call; from then on est/real = 1.00 (±0.01) for all 113 calls (HEAD: 0.9→2.3; fix1/fix2: 0.89–0.93).
- Fold fired at est 70,012 / real 70,557 — the configured 70k line, exactly — landed after 6.5 min ($0.014): `173 msgs → 33597 tokens saved (55.1%)`, real prompt 86k → 42.6k. No length cut, no forced fold.
- Turn: PASS, 42 min, 113 calls, 6.15M tokens (5.75M cache read, 169k uncached, 24k out, 212k reasoning).
- fix3 pydantic turn 2: PASS (6 min, 9 calls, 0.54M tok); context started at real 50k (the turn-1 fold had landed and was persisted) and stayed under the line; calibration re-learned on call 1 (factor 1.08) then est/real 1.00. Judge score 8.65 is an instrument flake: `judge.raw.log` holds `{"score":0,"reasoning":""}` (empty judge reply on the host); validation passed.

## 2026-09-22 — Step 13: HEAD vs fix1 on pydantic (complete)
| arm | turns | tokens | cache read | uncached in | reasoning | wall | folds |
|---|---|---|---|---|---|---|---|
| HEAD | 4/5 | 9.69M | 9.08M | 365k | 213k | 61 min | 6 spawned, 2 landed (one forced 16×), 3 length-cut, 1 lost at exit |
| fix1 (F1–F3) | 5/5 | 19.70M | 18.62M | 416k | 605k | 128 min | 9 spawned, 1 landed, 8 length-cut (2 collected at exit) |
Reading: fix1 is correct (5/5) but spends 2× the tokens. Two reasons, both expected from the mechanism: (a) HEAD's inflated estimate effectively folded at ~32k REAL tokens (cheap contexts when a fold happened to land; crashes when it did not — see pytest 0/5), whereas the fixed estimator folds at the configured 70k real tokens; (b) fix1 has no F4, so 8 of 9 folds died on the 16k length cut and the big contexts persisted (turns 3 and 5 ran 3 folds each, all wasted). Trajectory variance adds noise (turn 1: 100 vs 129 calls yet 6.7M vs 4.1M; deepseek's reasoning tripled). Conclusions: the fire line is now a real knob — for token-metric benches on cheap-cache models a lower `threshold` (≈40k) should recover HEAD's context size without its fragility; and F4 (fix3) is required for folds to land at all with this fold model. Launching `fix3` with `threshold = 40000` on pydantic to test.

## 2026-09-22 — Step 14: fix2 pytest turn 2 — a new failure exposed by F4's retry change
FAIL, exit 1 after 35 min: `Follow-up API call failed … empty response (finish_reason=Some("length")) … after 1 attempt(s)` — the MAIN model (deepseek-v4-flash, 32k max_tokens) spent its entire output budget reasoning at a real 125k context and returned nothing; my "no identical retry on a length cut" rule (F4, generic in `completion.rs`) then skipped the one retry HEAD would have made, so the turn died at once. Two folds before that were length-cut (7 and 9 min) even with the repair — logs emitted inside the spawned fold task are dropped (`log_*!` read a thread-local config that tokio tasks do not carry), so the repair's own line is invisible; the durations (2 requests each) say it ran and was cut again: `reasoning_effort=None` does not switch thinking off on Alibaba's DeepSeek-V4 (octolib only sends `enable_thinking=true` when an effort is set, never false), and 32k was still not enough.
Fixes (fix4): identical-retry skip only for calls that own a repair (compression), repair budget 4× (bounded by the model's max output when octolib knows it), repair failure wrapped with context so the main-task log says the repair ran. Main-model length cuts remain a model/config issue (reasoning budget vs max_tokens) — noted for the report.
- fix4 = fix3 + (identical-retry skip scoped to compression via `without_identical_retry_on_length_cut()`, repair budget 4×, repair failure carries context). e2e 21/21. Building `octomind-fix4`; a fresh pytest arm with it starts when the build lands.
- fix3 pydantic turn 3: PASS (25 min, 63 calls, 3.99M tok). Calibration 1.08 on call 1 then est/real 1.00; fold at est 70.1k / real 71.5k, landed in 7.5 min ($0.014): `159 msgs → 30616 tokens saved (50%)`, real 78.6k → 46.6k. fix3 folds so far: 3 spawned, 3 landed, 0 length cuts (fix1: 9 spawned, 1 landed). Fold latency (6.5–8.5 min each on deepseek-v4-flash medium reasoning) is now the dominant compaction cost.
- 20:24 launched `results-lr-fix4-pytest-dsv4` (binary `octomind-fix4` = F1–F5 + fix4 retry scoping) — the definitive pytest fix arm; `fix2-pytest` continues as a partial-fix data point.
- Final tree: `cargo test --lib` → 3808 passed, 0 failed, 9 ignored (469 s); clippy `-D warnings` clean; fmt clean.
- fix3 pydantic turn 4: PASS (9 min, 17 calls). Boundary-ish fold at est 70.2k / real 70.2k (calibration exact), turn ended 30 s later → exit wait held the process 6 min and collected a length cut (fix3's 2× repair also cut). Cost of a failing fold at exit = up to the fold budget in wall time with nothing to show; fix4's 4× budget is the test.

## Summary (as of 2026-09-22 20:40; runs still in progress — see the tables below for final numbers)
**What was measured.** Two 5-turn long-run sequences (pydantic, pytest) from `../octobench`, deepseek-v4-flash via Alibaba for every model call, sealed Docker, one container per sequence, HEAD 0.54.1 vs successive fix binaries.

**Root causes found (all verified in code and in logs):**
1. Context estimator counted stored assistant `thinking` that the provider never receives → estimate 1.3–4× the real prompt → folds fired at ~32k real tokens, "tokens saved" was mostly phantom, and resumed sessions hit the 200k ceiling at a real 50–70k → forced 16× folds → hard error (`context remains above the usable ceiling`). Baseline pytest died this way in all 5 turns.
2. Background folds were cancelled at every turn/inbox boundary (operation channel swap drops the sender; octolib treats that as cancel).
3. One-shot piped runs exited with the fold in flight; the paid summary was lost and the next `-r` turn re-paid it.
4. Fold responses length-cut by reasoning (16k budget, medium effort) were retried unchanged; each retry cost ~3 min and a fold.
5. cl100k under/over-counts every non-OpenAI tokenizer (deepseek: −10%); nothing corrected it.

**Fixes on `feat/longrun-efficiency` (uncommitted):** job-owned fold cancellation; bounded exit-time fold collection; provider-aware "sent-size" token estimate; typed `EmptyCompletion` + compression-only no-identical-retry + one repair (no effort, 4× budget); per-session prompt calibration from provider usage. 3808 tests pass, clippy clean.

**Measured effect (same model, same config):** pydantic HEAD 4/5 → fix1 5/5, fix3 4/4 so far; pytest HEAD 0/5 (crashes) → fix2 turn 1 PASS, 2/3 so far; estimate/real 0.9–2.6 → 1.00±0.01 (fix3). Token totals are NOT lower yet: the fixed estimator folds at the configured 70k real tokens (HEAD effectively folded at ~32k when it worked), so cache-read volume per turn is higher; the `threshold = 40000` arm (fix3t40) tests recovering that. Fold latency (6–9 min per fold on deepseek medium reasoning) is now the dominant compaction cost.

**Recommended next steps:** (1) lower the default `[compression] threshold` or make it a fraction of the ceiling now that it means real tokens; (2) default `[compression.model] reasoning_effort = "low"` (or expose `enable_thinking=false` in octolib) — folds do not need medium reasoning and it is what causes the length cuts and the 6–9 min latency; (3) octolib: expose the thinking-replay policy so `model_replays_thinking` stops mirroring it; (4) main-model length-cut empty replies (agent reasoned away its 32k budget at 125k context) still kill a turn after one retry — a nudge-and-retry or a reasoning-budget cap would save those turns; (5) octobench: bill compression/supervisor tokens (read `session_cost`/STATS) and record tokens on crashed turns; bump `configs/octomind/octomind.toml` to v17.

## 2026-09-22 — Step 15: fire line 40k (fix3t40) pydantic turn 1
PASS, 36 min, 82 calls, **3.85M tokens** (HEAD 4.11M, fix3@70k 6.15M). Fold fired at est 40,355 / real ~40.4k, landed in 2m40s ($0.010, small range → no length cut): `66 msgs → 19955 tokens saved (69.5%)`, real 35k → 28k; ladder doubled to 80k, turn ended at real 69k. Estimator 1.00 all turn. First evidence for the tuning recommendation: with an exact estimator, a 40k line reproduces HEAD's cheap-context regime without its fragility.
Instrument note: judge scores of 6–9 with `{"score":0,"reasoning":""}` in `judge.raw.log` keep recurring on the host-side judge (deepseek-v4-flash, `max_tokens = 8192`, medium reasoning) — the same reasoning-eats-the-budget length cut; use a non-thinking judge model or a larger judge budget. Validation (the oracle) is unaffected.

## 2026-09-22 — Step 16: pydantic complete for HEAD, fix1, fix3 (per-turn tables from compare_runs.py)
## HEAD: longrun_python_pydantic (alibaba:deepseek-v4-flash-0731)
| 1 | PASS | 4.11M | 131k | 3.87M | 15k | 87k | 0.90–2.34 | {'spawned': 1, 'landed': 1} |
| 2 | PASS | 0.86M | 78k | 0.77M | 3k | 11k | 1.38–1.61 | {'spawned': 1} |
| 3 | PASS | 3.02M | 74k | 2.87M | 6k | 63k | 1.24–6.45 | {'spawned': 1, 'landed': 1, 'length_cut': 1, 'failed': 1, 'forced_ceiling': 1} |
| 4 | PASS | 0.53M | 53k | 0.47M | 2k | 7k | 1.30–1.39 | {'spawned': 1} |
| 5 | FAIL | 1.18M | 28k | 1.10M | 6k | 44k | 1.09–1.91 | {'spawned': 1, 'length_cut': 1, 'failed': 1} |
| **all** | 4/5 | 9.69M | 365k | 9.08M | 35k | 213k | | cost_usd=0.146339 |
## fix1: longrun_python_pydantic (alibaba:deepseek-v4-flash-0731)
| 1 | PASS | 6.71M | 140k | 6.28M | 21k | 270k | 0.73–0.90 | {'spawned': 1, 'length_cut': 1, 'failed': 1} |
| 2 | PASS | 2.94M | 86k | 2.74M | 10k | 97k | 0.78–2.40 | {'spawned': 1, 'landed': 1} |
| 3 | PASS | 3.96M | 115k | 3.70M | 9k | 132k | 0.85–0.92 | {'spawned': 3, 'length_cut': 3, 'failed': 3, 'exit_wait': 1} |
| 4 | PASS | 1.56M | 23k | 1.53M | 2k | 4k | 0.80–0.91 | {'spawned': 1, 'length_cut': 1, 'failed': 1, 'exit_wait': 1} |
| 5 | PASS | 4.53M | 50k | 4.37M | 9k | 100k | 0.82–0.93 | {'spawned': 3, 'length_cut': 3, 'failed': 3} |
| **all** | 5/5 | 19.70M | 416k | 18.62M | 54k | 605k | | cost_usd=0.295495 |
## fix3: longrun_python_pydantic (alibaba:deepseek-v4-flash-0731)
| 1 | PASS | 6.15M | 168k | 5.75M | 23k | 212k | 0.81–1.97 | {'spawned': 1, 'landed': 1} |
| 2 | PASS | 0.54M | 58k | 0.48M | 3k | 6k | 0.93–1.00 | {} |
| 3 | PASS | 3.99M | 109k | 3.73M | 13k | 138k | 0.85–1.78 | {'spawned': 1, 'landed': 1} |
| 4 | PASS | 1.19M | 76k | 1.11M | 3k | 5k | 0.93–1.00 | {'spawned': 1, 'length_cut': 1, 'failed': 1, 'exit_wait': 1} |
| 5 | PASS | 4.33M | 65k | 4.17M | 14k | 82k | 0.78–1.00 | {'spawned': 2, 'landed': 1, 'length_cut': 1, 'failed': 1} |
| **all** | 5/5 | 16.21M | 478k | 15.23M | 58k | 444k | | cost_usd=0.250585 |

Wall time: HEAD 61 min (17:16→18:17), fix1 128 min (17:56→20:04), fix3 103 min (19:06→20:49). fix3: 5 folds spawned, 3 landed at the exact 70k line, 2 length-cut (the 2× repair was not enough), estimate exact after call 1 of every turn. Token totals track the fire line, not correctness: HEAD's broken estimate folded at ~32k real; fix3 at 70k real. fix3t40 (40k line) turn 1 = 3.85M vs HEAD 4.11M.
- fix3t40 pydantic turn 2: PASS (15 min, 34 calls, 2.08M tok). Boundary fold at real 70k landed in 6.5 min (140 msgs, 23.6k saved, 43%). Session after 2 turns: 5.93M (HEAD 4.97M, fix3@70k 6.70M).
- fix3t40 pydantic turn 3: FAIL (exit 1) — MAIN-model length-cut empty reply at a real 57k context (deepseek reasoned its 32k budget away) and fix3's generic no-identical-retry rule ended the turn after 1 attempt; second occurrence of the fix3 regression that fix4 scopes back to compression only. The fold in that turn landed in 57 s (85 msgs). fix3t40 after 3 turns: 2/3, 7.5M tokens.

## 2026-09-22 — Step 17: fix4 pytest turn 1 — PASS (HEAD: crash)
60 min, 140 calls, 8.83M tokens (8.31M cache read, 236k uncached, 21k out, 268k reasoning). Calibration 1.12 on call 1, est/real 1.00 for all 140 calls. One fold at est 72.5k / real 72.5k, landed after 13 min ($0.017; 2 requests = original + 4× repair, both silent from the task): `151 msgs → 17655 tokens saved (29%)`, real 87.6k → 58.5k. Turn ended at real 86k (ladder at 140k).

## Status at hand-off (2026-09-22 21:30) — still running in Docker, results land under `../octobench/`
- `results-lr-fix4-pytest-dsv4` (definitive pytest fix arm): turn 1 PASS, turn 2 running.
- `results-lr-fix2-pytest-dsv4` (F1–F4 with 2× repair): 3/4, turn 5 running at a real ~190k context (its folds were cut; watch for the ceiling).
- `results-lr-fix3t40-pydantic-dsv4` (fix3 + `threshold=40000`): 2/3 (turn 3 = the fix3 retry regression), turns 4–5 running.
Re-run `python3 bench/longrun/compare_runs.py HEAD=results-lr-base-<seq>-dsv4 fixN=results-lr-fixN-<seq>-dsv4` from `../octobench` for the final tables. Code is on branch `feat/longrun-efficiency`, uncommitted (21 files, +845/−91), fmt/clippy/tests clean.

## 2026-09-23 — Step 18: the follow-ups, implemented
Final arm results from yesterday: `fix3t40-pydantic` 4/5 (turn 3 = the fix3 agent-retry regression, fixed in fix4), `fix2-pytest` 4/5 (turn 2 = same regression), `fix4-pytest` 4/4 with turn 5 running (HEAD: 0/5).
- **octolib 0.40.0 (`../octolib`, path dependency until published):** `ReasoningEffort::None` = no reasoning + thinking switched off per provider (Alibaba `enable_thinking=false`, DeepSeek/Z.AI `thinking.type=disabled`, OpenRouter `reasoning.enabled=false`, OpenAI gpt-5.x `effort=none`, Cloudflare only when the catalog lists it — GLM-5.3 there normalizes "none" to "max"); `AiProvider::replays_thinking(model)` (true for Z.AI, DeepSeek, Kimi K2.6+/K3 on Moonshot/Ollama). 625 existing tests + 9 new pass.
- **octomind:** `reasoning_effort = "none"` (also `/effort none`); `[compression.model]` default is now `"none"`; the fold repair asks for `None` explicitly; `model_replays_thinking` asks the provider instead of mirroring octolib; the agent-loop retry after a length-cut empty reply goes out with reasoning off (was: identical re-send, then dead turn); the cost frame carries `aux_input_tokens`/`aux_output_tokens` (compression + supervisor) and is also emitted when a piped turn exits on a provider error.
- **octobench:** `configs/octomind/octomind.toml` folds and judge on `reasoning_effort = "none"`; `providers/octomind.py` reads the aux tokens (7-tuple session baseline), `scoring.compute_aux_cost` prices them, `cli/longrun.py` and `cli/main.py` add them to `total`/`cost_usd`; `docs/PROVIDER_INTERFACE.md` documents the fields. `bench_selftest.py` 62/62. Crashed turns now carry tokens because octomind emits the frame on failure — no runner change needed.
- **octomind bench/README.md:** the HEAD build recipe now includes `protoc` and the static ONNX Runtime (`ORT_LIB_LOCATION`), which the plain recipe lacked.
- `fix4-pytest` final: 4/5. Turn 5 died at the ceiling: turn 4 had ended at a real 184k (its folds never landed), the boundary fold on that 184k range was length-cut, the repair ("reasoning off" in fix4 = effort merely absent, so Alibaba kept thinking; 64k budget) hit the 300 s request timeout, the forced 16× fold did the same, hard error after 27 min with 1 agent call. Two lessons already acted on: real thinking-off needs `ReasoningEffort::None` (fix5/octolib 0.40), and a fold that cannot land inside a turn must not let the session grow to the ceiling — the fire line is the first defence, `threshold = 40000` on this model keeps folds small enough (2.7 min, 57 s) to land.
- Final tree: `cargo test --lib` 3808 passed; the one failure in the parallel run (`mcp::agent::functions::command_tests::forward_session_update_maps_every_rendered_kind`, a channel `try_recv` race unrelated to these changes) passes 3/3 alone and also on the pre-change tree. Clippy clean (octomind `--all-features`; octolib with `llm,embeddings,evaluation` — its `--all-features` pulls an Apple-only crate on Linux, pre-existing).

## 2026-09-23 — Step 19: fold with `reasoning_effort = "none"` (fix5 = octolib 0.40 + octomind final, pydantic turn 1)
First fold at ~70k real tokens: `156 msgs → 47825 tokens saved`, **compression API time 49 s** (medium reasoning: 6.5–13 min per fold, or a length cut), 34.4k input / 4.5k output tokens (no reasoning tokens), $0.021 — the PACT record `compression_api_time_ms=49137`, wall from decision to apply 173 s (the background task waits for the current agent round). Run continues as `results-lr-fix5-pydantic-dsv4`; compare with `python3 bench/longrun/compare_runs.py HEAD=results-lr-base-pydantic-dsv4 fix5=results-lr-fix5-pydantic-dsv4` from `../octobench` once it finishes.
