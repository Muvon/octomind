# Long-run measurement helpers

Companions to `bench/flow.md` (the campaign log). Everything runs in Docker through
`../octobench` (`cli.longrun`, one container per sequence).

- `run-seq.sh <sequence-dir> <matrix.yaml> <out-dir> [max-turns]` — sealed run of one
  long-run sequence with a locally built binary (`OCTOMIND_BIN`) and a campaign config
  (`OCTOBENCH_OCTOMIND_CONFIG`, must be the current config version — run
  `octomind config --upgrade` on it inside the agent image first). Sets `RUST_LOG=octomind=debug`
  so fold decisions land in `turns/turn_N/logs/provider.stderr.log`.
- `analyze_run.py <out-dir>` — per-turn fold events + session-level tool-call habits
  (dedupes messages re-logged after `COMPRESSION_POINT`).
- `compare_runs.py label=<out-dir> …` — per-turn table from `results.json`: verdict, tokens,
  estimate/real prompt ratio (from the debug log), fold events.

Build the binary the bench can run (glibc 2.36 + static ONNX Runtime):
```
docker run --rm -v $PWD:/src -v $PWD/target-bookworm:/out -e CARGO_TARGET_DIR=/out \
  -e CARGO_HOME=/out/cargo-home rust:bookworm bash -c '
  apt-get update -qq && apt-get install -y -qq protobuf-compiler unzip
  A=onnxruntime-linux-x64-static_lib-1.24.2-glibc2_17
  [ -d /out/tmp/$A/lib ] || { curl -sL https://github.com/csukuangfj/onnxruntime-libs/releases/download/v1.24.2/$A.zip -o /out/tmp/ort.zip && unzip -q -o /out/tmp/ort.zip -d /out/tmp; }
  export ORT_LIB_LOCATION=/out/tmp/$A/lib
  cd /src && cargo build --release --locked && cp /out/release/octomind /out/octomind-head'
```
