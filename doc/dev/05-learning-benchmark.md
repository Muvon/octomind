# Learning Accuracy-Efficiency Benchmark

This compact benchmark is an architecture detector, not a replacement for the
full LongMemEval leaderboard. It has two layers:

1. `octomind-memory-contract-v1`: 52 curated cases covering exact,
   paraphrased, noisy, indirect, correction-vs-stale, and unrelated queries.
   Calibration, holdout, and challenge splits are explicit.
2. `longmemeval-cleaned-oracle-stratified-30-retrieval`: five questions from
   each of the six official LongMemEval task types. Their relevant sessions are
   combined into one 52-session distractor pool. This measures retrieval only,
   not final answer accuracy, and must be reported with that qualifier.

Both compare dense retrieval, equal RRF, fixed sparse weighting, and the current
adaptive production hybrid. Query rewrites are cached under
`target/learning-benchmark/`, so only the first run spends model tokens.

## Server commands

The synced server exports provider credentials from its interactive global
environment, so use `zsh -ic`. Never print credential values.

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

The public subset requires retrieval recall@5 of at least `0.80`. Always report
top-1, recall@5, MRR, model, rewrite calls/cache hits/rejections, question count,
and memory-session count. Do not call the subset a full LongMemEval score.

Use calibration for parameter exploration. Open holdout only after selecting a
candidate, then add a new challenge slice before any further tuning. A public or
challenge failure is evidence against the candidate; never lower the gate to
make it pass.

## Reference result — 2026-08-28

Model: `alibaba:qwen3.8-flash`. These figures establish the compact benchmark
frontier only; they are not a full LongMemEval score or a universal SOTA claim.

| Retrieval mode | Internal top-1 | Internal R@5 | Abstain | Stale@1 | Public top-1 | Public R@5 |
|---|---:|---:|---:|---:|---:|---:|
| Dense | 80.0% | 97.5% | 83.3% | 0 | 56.7% | 73.3% |
| Equal hybrid | 80.0% | 100% | 83.3% | 5 | 66.7% | 86.7% |
| Adaptive production | **87.5%** | **100%** | **83.3%** | **0** | **66.7%** | **86.7%** |

Production therefore matches the best public-subset retrieval and removes the
equal hybrid's stale-memory failures, while improving the internal top-1 score
by 7.5 points over both baselines.

The consolidation check accepted one of two safe merges, compressed it from
445 to 126 estimated tokens, and rejected both unsafe pairs: zero false accepts,
50% safe-merge acceptance. Conservative rejection is preferable because it
costs storage efficiency rather than durable correctness.

A fresh 12-query challenge run measured the first-session retrieval cost:

- rewrite: 2,124 input and 240 output tokens total; 38.3 seconds total
  (`3.19 s/query`);
- local embedding ranking: 1.12 seconds total (`94 ms/query`);
- rewrite failures: zero;
- provider-reported monetary cost: unavailable (`0.0` in the response), so it
  must not be described as free.

Dense-only retrieval is therefore the latency baseline. Adaptive hybrid is on
the measured accuracy-latency frontier: it costs one rewrite on the first
session retrieval, improves internal top-1 and recall, removes stale winners,
and subsequent user turns remain embedding-only.
