# octomind bench

Measure **token-efficiency-with-success** as octomind changes, and detect
regression/improvement between refs (e.g. `0.32.0` → `HEAD`).

Headline metrics:
- **success rate** — guardrail; must not regress.
- **cost-per-solved** — the win; total tokens (or $) ÷ tasks solved.
- **turns** — diagnostic.

## Architecture (best practice)
- **octobench** (`../octobench`, runs on the box) is the *instrument*: builds/fetches
  octomind for a ref, runs the suite in isolated Debian-12 containers (SWE-bench-Live —
  the instance's own tests are the objective oracle), emits normalized token telemetry.
- **this dir** (`octomind/bench/`) holds the *committed truth*: `config.yaml`, the frozen
  `baseline.json`, and `suite.txt`. The git history of `baseline.json` IS the trend.

## An "arm" = `{ref} × {config}`
- **released tag** (e.g. `0.32.0`) → octobench image bakes it (downloaded). The pilot
  baseline arm uses the image's stock 0.32.0 (no override).
- **unreleased** (branch/commit/`HEAD`) → build a **glibc (bookworm, 2.36)** binary so it
  runs in the Debian-12 SWE images (the box is glibc 2.39 → too new; ORT ships prebuilt
  glibc so it's a plain `cargo build --release`), then **mount it** over the image's
  binary via `OCTOMIND_BIN` (executor honors it — no image rebuilds).

Build HEAD for the SWE images:
```
docker run --rm -v /home/box/work/muvon/octomind:/src:ro -v /tmp/oct-head-out:/out \
  -e CARGO_TARGET_DIR=/out rust:bookworm \
  bash -c "cargo build --release --locked --manifest-path /src/Cargo.toml && cp /out/release/octomind /out/octomind-head"
# -> /tmp/oct-head-out/octomind-head  (glibc 2.36, runs in SWE images)
```

## Running (on the box)
```
cd /home/box/work/muvon/octobench
eval "$(grep '^export ' ~/.zshrc)"          # all API keys (BRAVE, OCTOHUB, ...)
export OCTOHUB_API_URL=https://octohub.muvon.dev   # PUBLIC (127.0.0.1:9595 is host-only, unreachable in containers)

# baseline arm (stock 0.32.0 in the image):
python -m cli.swebench --instance <id> --config configs/run-matrix.octomind-glm.swebench.yaml --out results-baseline

# candidate arm (HEAD mounted over the baked binary):
OCTOMIND_BIN=/tmp/oct-head-out/octomind-head \
python -m cli.swebench --instance <id> --config configs/run-matrix.octomind-glm.swebench.yaml --out results-head
```
Each run writes `results.json` with `swebench.resolved`, `tokens.*`, `cost_usd`, `elapsed_ms`.
Repeat k× per (instance, arm); compare paired-by-instance.

## Updating the baseline
Re-run the suite for the new ref, aggregate, and commit `bench/baseline.json` (old one stays
in git history). Compare with the paired-delta aggregator (success rate, cost-per-solved, turns,
+ a regression watchlist of instances the baseline solved but the candidate didn't).

## Known notes
- **GLM doesn't report prompt caching** (`cached: 0`). We do **not** rely on caching — metric
  is total tokens + solved. Ignore the cache fields for glm.
- **octocode (semantic search) times out (15s) in ephemeral reset repos** — it's launched by
  octomind's `codesearch` capability *inside the container*, so bench-config overrides don't
  reach it. It's **non-fatal** (runs still solve). Proper fix belongs in octomind: the
  `codesearch` capability should launch `octocode mcp --no-git` (or handle git-absence). Until
  then both arms run octocode-off equally, so the **delta isolates the steering rework** (just
  with higher absolute tokens than ideal).
- First proven data point (smoke, 0.32.0, glm, `jupyter-ai-1022`): **resolved=True (3/3 + 35/35),
  ~2.99M tokens, $3.04, ~8 min** — token efficiency (not caching) is the lever.
