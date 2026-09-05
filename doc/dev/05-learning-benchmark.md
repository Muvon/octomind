# Learning Benchmark

Use these live contract benchmarks when changing learning retrieval or consolidation. They compare ranking and merge
decisions on compact fixtures; they do not measure full LongMemEval answer accuracy.

## Benchmark Layers

- `octomind-memory-contract-v1`: 52 curated cases covering exact, paraphrased, noisy, indirect,
  correction-vs-stale, and unrelated queries. Calibration, holdout, and challenge splits are explicit.
- `longmemeval-cleaned-oracle-stratified-30-retrieval`: five questions from each of the six task types named by
  the harness. The first five matching items of each type in source order contribute their haystack sessions
  to one deduplicated distractor pool; its size is reported as `memory_sessions`. This measures retrieval only,
  not final answer accuracy, and must be reported with that qualifier.

Both compare dense retrieval, equal reciprocal-rank fusion (RRF), fixed sparse weighting, and an adaptive hybrid using
production ranking helpers. The curated harness also reports raw keyword and raw hybrid modes. Query rewrites are cached
under `target/learning-benchmark/`, so later runs can reuse validated rewrites.

## Prepare the environment

Run from the repository root on the machine where you build and test. Complete the native dependency setup in [Building
from Source](01-building-from-source.md), then authenticate that machine:

```bash
octomind login
octomind config --validate
```

The harness loads your real configuration and credentials via `Config::load` and `EnvTracker::load_dotenv_override`.
User-scope `config/.env` loads first, then the current directory's `.env`; both override process environment values. No
particular SSH host or interactive shell is required. When `LEARNING_BENCH_MODEL` is omitted, the harness resolves the
supervisor profile from that configuration; its shipped model name is `octohub:auto`. A model override changes the name,
retaining the profile's other fields.

### Environment variables

| Variable | Default / requirement | Scope |
|----------|-----------------------|-------|
| `LEARNING_BENCH_LIVE` | Must equal `1` | All three ignored benchmarks; permits provider calls |
| `LEARNING_BENCH_MODEL` | Resolved supervisor model | All three benchmarks |
| `LEARNING_BENCH_SPLIT` | `calibration`; accepts `calibration`, `holdout`, `challenge`, `all` | Curated retrieval |
| `LEARNING_BENCH_REWRITE_CACHE` | `target/learning-benchmark/rewrite-cache.json` | Curated retrieval only |
| `LEARNING_BENCH_REPORT` | `target/learning-benchmark/{split}.json` | Curated retrieval only |
| `LONGMEMEVAL_ORACLE_JSON` | Required path to downloaded JSON | Public retrieval only |
| `LONGMEMEVAL_EXPECTED_SHA256` | Pinned hash below | Public retrieval; override only for a reviewed dataset change |
| `OCTOMIND_CONFIG_PATH` | Standard config file path | Selects configuration and merges its TOML siblings |
| `OCTOMIND_DATA_DIR` | Platform data directory | Relocates config, learning, sessions, and Octomind cache |

These benchmark variables are read in the three test files linked under [Source
reference](#production-scope-and-source-reference). The config and data overrides are implemented in
`src/config/loading.rs` and `src/directories.rs`.

## Run the benchmarks

### Curated retrieval

Start with calibration; change `LEARNING_BENCH_SPLIT` only when you are ready to evaluate that split:

```bash
LEARNING_BENCH_LIVE=1 \
LEARNING_BENCH_SPLIT=calibration \
LEARNING_BENCH_MODEL=octohub:auto \
cargo test --lib compact_learning_retrieval_frontier \
  -- --ignored --nocapture --test-threads=1
```

The fixtures generate 24 calibration cases, 16 holdout cases, and 12 challenge cases (52 total). For an all-split run
with a fresh curated rewrite cache and a separate report:

```bash
bench_run_dir=$(mktemp -d)
LEARNING_BENCH_LIVE=1 \
LEARNING_BENCH_SPLIT=all \
LEARNING_BENCH_REWRITE_CACHE="$bench_run_dir/rewrite-cache.json" \
LEARNING_BENCH_REPORT="$bench_run_dir/all.json" \
cargo test --lib compact_learning_retrieval_frontier \
  -- --ignored --nocapture --test-threads=1
python3 -m json.tool "$bench_run_dir/all.json"
```

### Public retrieval subset

Download the revision pinned by the harness, then run the 30-question retrieval subset:

```bash
mkdir -p target/learning-benchmark
oracle_revision=98d7416c24c778c2fee6e6f3006e7a073259d48f
curl -fL \
  "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/$oracle_revision/longmemeval_oracle.json" \
  -o target/learning-benchmark/longmemeval_oracle.json

LEARNING_BENCH_LIVE=1 \
LONGMEMEVAL_ORACLE_JSON="$PWD/target/learning-benchmark/longmemeval_oracle.json" \
cargo test --lib compact_longmemeval_oracle_retrieval \
  -- --ignored --nocapture --test-threads=1
```

The harness verifies SHA-256 `821a2034d219ab45846873dd14c14f12cfe7776e73527a483f9dac095d38620c` before parsing. For a
deliberately reviewed dataset revision, supply its reviewed hash through `LONGMEMEVAL_EXPECTED_SHA256`; do not bypass a
mismatch by blindly trusting the downloaded bytes. You can explicitly enforce the current pin:

```bash
LEARNING_BENCH_LIVE=1 \
LONGMEMEVAL_EXPECTED_SHA256=821a2034d219ab45846873dd14c14f12cfe7776e73527a483f9dac095d38620c \
LONGMEMEVAL_ORACLE_JSON="$PWD/target/learning-benchmark/longmemeval_oracle.json" \
cargo test --lib compact_longmemeval_oracle_retrieval \
  -- --ignored --nocapture --test-threads=1
```

### Consolidation precision

Run the four-case check separately: two safe orientation-memory pairs and two unsafe pairs exercise the proposer and
independent verifier. These are model calls, and this test has no rewrite-result cache.

```bash
LEARNING_BENCH_LIVE=1 \
cargo test --lib compact_consolidation_precision \
  -- --ignored --nocapture --test-threads=1
```

## Acceptance contract

The curated production mode must have:

- recall@5 at least `0.90`;
- abstention accuracy at least `0.75` whenever negatives are present;
- zero stale memories at rank one;
- zero rewrite transport failures.

The pinned public subset requires retrieval recall@5 of at least `0.95` and zero rewrite failures. Consolidation
requires zero unsafe accepts and at least one of the two safe merges accepted. Always report top-1, recall@5, MRR,
model, rewrite calls/cache hits/rejections, question count, and memory-session count. Do not call the subset a full
LongMemEval score.

Use calibration for parameter exploration. Open holdout only after selecting a candidate, then add a new challenge slice
before any further tuning. A public or challenge failure is evidence against the candidate; never lower the gate to make
it pass.

## Read reports and compare runs

| Benchmark | Default report | Production mode key |
|-----------|----------------|---------------------|
| Curated | `target/learning-benchmark/{calibration,holdout,challenge,all}.json` | `production_adaptive_hybrid` |
| Public subset | `target/learning-benchmark/longmemeval-oracle-30.json` | `production` |
| Consolidation | `target/learning-benchmark/consolidation.json` | Per-case `accepted` and aggregate acceptance |

Reports are written before the final assertions, so a failed gate still leaves diagnostics. For example:

```bash
python3 -m json.tool target/learning-benchmark/calibration.json
python3 -m json.tool target/learning-benchmark/longmemeval-oracle-30.json
python3 -m json.tool target/learning-benchmark/consolidation.json
```

The public `recall_at_5` is the fraction of questions with **any** answer session in the top five, not the fraction of
all required sessions recovered. MRR uses the first relevant session. Curated top-1, recall@5, and MRR use positive
cases as their denominator; abstention accuracy uses negative cases.

Both retrieval reports include rewrite calls, cache hits, rejections, failures, latency, and supervisor usage. A
rejected rewrite is distinct from a transport failure: the curated harness falls back to raw keywords; the public
harness falls back to no keyword patterns. The gates do not require zero rejections.

Curated cache keys contain model name and query; public keys also contain question ID. They do not include the rewrite
prompt or all model parameters. Use a fresh curated cache when those change; for public runs, preserve and move aside
the fixed cache before rerunning if it exists:

```bash
if [ -f target/learning-benchmark/longmemeval-rewrite-cache.json ]; then
  bench_cache_backup=$(mktemp -d)
  mv target/learning-benchmark/longmemeval-rewrite-cache.json "$bench_cache_backup/"
fi
```

Record whether each run used warm rewrite and embedding caches. Curated lesson vectors are warmed outside the measured
query interval. Public production scoring is measured after baseline scoring for the same question. Neither interval is
an end-to-end session latency measurement. Publish the generated report with the source revision and resolved profile;
this guide does not assert a current measured score.

## Common Questions

| Problem | What to check |
|---------|---------------|
| Test is skipped | Include `--ignored`; these tests are deliberately opt-in |
| Test rejects the live setting | Set `LEARNING_BENCH_LIVE=1` before the command |
| Authentication fails | Log in on the test machine; check project `.env` overrides and the selected model |
| Native linking or model loading fails | Follow the build guide's ONNX and embedding setup |
| Dataset hash mismatch | Redownload the pinned revision; review any intentional revision change |
| Rerun makes no rewrite calls | Inspect `rewrite.cache_hits`; choose a fresh cache for uncached measurements |
| No report appears | Setup failed before report generation; inspect the test error output |
| Report says pass but answers are poor | These harnesses score retrieval or merge decisions, not generated answers |

## Production scope and source reference

The ranking harnesses call production scoring helpers on fixtures. They do not exercise the entire file store, Active
Memory Pack, outcome attribution, experience formation, evolution, or hot/cold retention lifecycle. The four-case
consolidation harness calls `propose_and_verify`; it does not run file archival or retention budgets.

Production retrieval uses 128-token semantic chunks with max-chunk scoring in `src/supervisor/learning/backend/file.rs`.
In `src/session/chat/session/api_executor.rs`, the first session recall uses hybrid retrieval; later genuine user turns
use embedding-only scoped retrieval. The runtime replaces a bounded memory pack and drops it when request headroom is
insufficient. Benchmark rewrite caches are test artifacts, not the session pack.

| Source | Contract |
|--------|----------|
| [Curated harness](../../src/supervisor/learning/backend/file_benchmark_tests.rs) | Cases, modes, env vars, reports, gates |
| [Public harness](../../src/supervisor/learning/backend/longmemeval_benchmark_tests.rs) | Dataset pin, selection, metrics, cache |
| [Consolidation harness](../../src/supervisor/learning/retention_benchmark_tests.rs) | Safe/unsafe pairs, acceptance assertions |
| [File backend](../../src/supervisor/learning/backend/file.rs) | Retrieval, chunk scoring, cold recall |
| [Retention](../../src/supervisor/learning/retention.rs) | Budgets, verifier, archival |
| [Memory injection](../../src/supervisor/learning/inject.rs) | Rewrite validation and Active Memory Pack |

## See also

- [Building from Source](01-building-from-source.md)
- [Architecture](02-architecture.md)
- [Learning](../usage/13-learning.md)
- [Supervisor](../usage/14-supervisor.md)
