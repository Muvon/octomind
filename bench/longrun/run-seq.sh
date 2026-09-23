#!/usr/bin/env bash
# usage: run-seq.sh <sequence-dir-relative-to-octobench> <matrix.yaml> <out-dir> [max-turns]
set -euo pipefail
SEQ=$1; MATRIX=$2; OUT=$3; MAXT=${4:-}
S=${OCTOBENCH_SCRATCH:-$(dirname "$0")}
cd "${OCTOBENCH_DIR:-$(dirname "$0")/../../../octobench}"
export OCTOBENCH_SEAL_NETWORK=1 OCTOBENCH_CLEAN_WORKSPACE=1 OCTOMIND_AGENT=developer
export OCTOBENCH_SYSTEM_PROMPT=configs/common/system_prompt.md
export OCTOBENCH_GIT_MIRRORS=$HOME/.cache/octobench/git-mirrors
export OCTOBENCH_OCTOMIND_CONFIG=${OCTOBENCH_OCTOMIND_CONFIG:?path to a v17 campaign config (configs/octomind/octomind.toml upgraded with octomind config --upgrade, compression+supervisor models on the cheap provider)}
export OCTOMIND_BIN=${OCTOMIND_BIN:?glibc-2.36 octomind binary built in rust:bookworm}
export OCTOBENCH_JUDGE_MODEL=${OCTOBENCH_JUDGE_MODEL:-alibaba:deepseek-v4-flash-0731}
export RUST_LOG=${RUST_LOG:-octomind=debug}
ARGS=(--sequence "$SEQ" --config "$MATRIX" --executor docker --image octobench-agent:head --out "$OUT" --verbosity normal)
[ -n "$MAXT" ] && ARGS+=(--max-turns "$MAXT")
exec .venv/bin/python -m cli.longrun run "${ARGS[@]}"
