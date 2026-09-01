# Learning Benchmark

This benchmark compares learning retrieval and consolidation against compact contracts; it detects architecture changes, not full LongMemEval performance.

## Benchmark Layers

1. `octomind-memory-contract-v1`: 52 curated cases covering exact,
   paraphrased, noisy, indirect, correction-vs-stale, and unrelated queries.
   Calibration, holdout, and challenge splits are explicit.
2. `longmemeval-cleaned-oracle-stratified-30-retrieval`: five questions from
   each of the six official LongMemEval task types. Their relevant sessions are
   combined into one 52-session distractor pool. This measures retrieval only,
   not final answer accuracy, and must be reported with that qualifier.

Both compare dense retrieval, equal RRF, fixed sparse weighting, and the current
adaptive production hybrid. Query rewrites are cached under
`target/learning-benchmark/`, so later runs can reuse validated rewrites.

## Server commands

The synced server exports provider credentials from its interactive global
environment, so use `zsh -ic`. Authenticate with `octomind login`; when
`LEARNING_BENCH_MODEL` is omitted, the harness uses the supervisor-purpose
profile, whose default is `octohub:auto`. The reproduction commands below set
the recorded `alibaba:qwen3.8-flash` override explicitly. Never print credential
values.

```bash
ssh dev 'zsh -ic '\''
  cd /home/box/work/muvon/octomind &&
  LEARNING_BENCH_LIVE=1 \
  LEARNING_BENCH_SPLIT=all \
  LEARNING_BENCH_MODEL=alibaba:qwen3.8-flash \
  cargo test --lib compact_learning_retrieval_frontier \
    -- --ignored --nocapture --test-threads=1
'\'''
```

Download the small official oracle file once (about 15 MB), then run the public
subset:

```bash
ssh dev 'wget -q \
  https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/98d7416c24c778c2fee6e6f3006e7a073259d48f/longmemeval_oracle.json \
  -O /tmp/longmemeval_oracle.json'

ssh dev 'zsh -ic '\''
  cd /home/box/work/muvon/octomind &&
  LEARNING_BENCH_LIVE=1 \
  LEARNING_BENCH_MODEL=alibaba:qwen3.8-flash \
  LONGMEMEVAL_ORACLE_JSON=/tmp/longmemeval_oracle.json \
  cargo test --lib compact_longmemeval_oracle_retrieval \
    -- --ignored --nocapture --test-threads=1
'\'''
```

The harness verifies SHA-256
`821a2034d219ab45846873dd14c14f12cfe7776e73527a483f9dac095d38620c`.
To evaluate a deliberately updated dataset, set
`LONGMEMEVAL_EXPECTED_SHA256` to its reviewed hash; dataset drift otherwise
fails closed.

Reports:

- `target/learning-benchmark/{calibration,holdout,challenge,all}.json`
- `target/learning-benchmark/longmemeval-oracle-30.json`
- `target/learning-benchmark/consolidation.json`

Run the four-case consolidation precision check separately because it can make
both proposer and verifier calls:

```bash
ssh dev 'zsh -ic '\''
  cd /home/box/work/muvon/octomind &&
  LEARNING_BENCH_LIVE=1 \
  LEARNING_BENCH_MODEL=alibaba:qwen3.8-flash \
  cargo test --lib compact_consolidation_precision \
    -- --ignored --nocapture --test-threads=1
'\'''
```

## Acceptance contract

The curated production mode must have:

- recall@5 at least `0.90`;
- abstention accuracy at least `0.75` whenever negatives are present;
- zero stale memories at rank one;
- zero rewrite transport failures.

The pinned public subset requires retrieval recall@5 of at least `0.95`. Always report
top-1, recall@5, MRR, model, rewrite calls/cache hits/rejections, question count,
and memory-session count. Do not call the subset a full LongMemEval score.

Use calibration for parameter exploration. Open holdout only after selecting a
candidate, then add a new challenge slice before any further tuning. A public or
challenge failure is evidence against the candidate; never lower the gate to
make it pass.

## Reference result — 2026-08-28

Historical override used for this recorded run: `alibaba:qwen3.8-flash`. These
figures establish the compact benchmark frontier only; they are not a full
LongMemEval score or a universal SOTA claim. Omit `LEARNING_BENCH_MODEL` to use
the shipped supervisor-purpose default, `octohub:auto`; set it when reproducing
or comparing an explicit override.

| Retrieval mode | Internal top-1 | Internal R@5 | Abstain | Stale@1 | Public top-1 | Public R@5 |
|---|---:|---:|---:|---:|---:|---:|
| Dense | 80.0% | 97.5% | 83.3% | 0 | 76.7% | 90.0% |
| Equal hybrid | 80.0% | 100% | 83.3% | 5 | 73.3% | 90.0% |
| Adaptive production | **87.5%** | **100%** | **83.3%** | **0** | **83.3%** | **96.7%** |

Production therefore leads every tested public-subset retrieval baseline,
removes the equal hybrid's stale-memory failures, and improves internal top-1
by 7.5 points over both baselines. The public gain comes from 128-token semantic
chunks with max-chunk late interaction; one-chunk memories retain their exact
legacy embedding input.

The consolidation check accepted one of two safe merges, compressed it from
445 to 126 estimated tokens, and rejected both unsafe pairs: zero false accepts,
50% safe-merge acceptance. Conservative rejection is preferable because it
costs storage efficiency rather than durable correctness.

A fresh 12-query challenge run measured the first-session retrieval cost:

- rewrite: 2,124 input and 240 output tokens total; 38.3 seconds total
  (`3.19 s/query`);
- local embedding ranking: 1.12 seconds total (`94 ms/query`);
- rewrite failures: zero;

Dense-only retrieval is therefore the latency baseline. Adaptive hybrid is on
the measured accuracy-latency frontier: it costs one rewrite on the first
session retrieval, improves internal top-1 and recall, removes stale winners,
and subsequent user turns remain embedding-only.

On the public 52-session pool, cached semantic scoring took 15.7 seconds for 30
queries (`523 ms/query`). This measures the longer heterogeneous-memory pool;
ordinary short lesson stores have a different workload shape.
