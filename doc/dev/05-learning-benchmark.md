# Learning Benchmark: Moved With the Learning System

The learning system, and the benchmarks that tested its retrieval and consolidation, are no longer part of this
repository. Octomind now stores nothing across sessions on its own. The Besydr plugin provides learning; see
[Cross-session learning](../usage/13-learning.md) and [Besydr's learning page](https://besydr.com/docs/learning).

The test harnesses this page used to describe (`compact_learning_retrieval_frontier`,
`compact_longmemeval_oracle_retrieval`, `compact_consolidation_precision`) and the environment variables
`LEARNING_BENCH_LIVE`, `LEARNING_BENCH_MODEL`, `LEARNING_BENCH_SPLIT`, `LEARNING_BENCH_REWRITE_CACHE`,
`LEARNING_BENCH_REPORT`, `LONGMEMEVAL_ORACLE_JSON` and `LONGMEMEVAL_EXPECTED_SHA256` no longer exist here. Besydr's
documentation does not describe a benchmark for its learning, so there is none to point to.

What this repository still tests about plugins is the protocol and the host, in `src/plugins/`. See
[Architecture](02-architecture.md) and `plugin-spec.md` in the repository root.
