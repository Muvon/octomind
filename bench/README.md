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
- **this dir** (`octomind/bench/`) holds the *committed truth*: `config.yaml`, `baseline.json`
  (the current reference), `run-head.sh` + `compare_to_baseline.py` (the routine HEAD-only check),
  and `aggregate.py` (the paired base-vs-head view). The git history of `baseline.json` IS the trend.

## An "arm" = `{ref} × {config}`
- **released tag** (e.g. `0.32.0`) → octobench image bakes it (downloaded). The pilot
  baseline arm uses the image's stock 0.32.0 (no override).
- **unreleased** (branch/commit/`HEAD`) → build a **glibc (bookworm, 2.36)** binary so it
  runs in the Debian-12 SWE images (the box is glibc 2.39 → too new; ORT ships prebuilt
  glibc so it's a plain `cargo build --release`), then **mount it** over the image's
  binary via `OCTOMIND_BIN` (executor honors it — no image rebuilds).

Build HEAD for the SWE images (`$OCTOMIND` = octomind checkout, `$OUT` = a build dir on the build box):
```
docker run --rm -v "$OCTOMIND":/src:ro -v "$OUT":/out -e CARGO_TARGET_DIR=/out rust:bookworm \
  bash -c "cargo build --release --locked --manifest-path /src/Cargo.toml && cp /out/release/octomind /out/octomind-head"
# -> $OUT/octomind-head  (glibc 2.36, runs in the SWE images)
```

## Routine check: HEAD-only vs the baseline
The baseline (`arms.base` in `baseline.json`) is the **current verified-good version** — a moving
reference, not a fixed release. A routine check runs **only the new build (HEAD)** and compares to
it; do NOT re-run a base arm each time (the reference is already recorded).

1. Build the HEAD glibc binary (above) → `$OUT/octomind-head`.
2. Run HEAD over the suite, k reps per instance, with `OCTOMIND_BIN` pointed at that binary, into
   `<dir>/head/<instance>-r<rep>`. Each run writes `results.json` (`swebench.resolved`, `tokens.*`,
   `cost_usd`, `elapsed_ms`). The driver `run-head.sh` does the loop + concurrency + fresh-repeat;
   run it from the octobench instrument dir with `HEAD_BIN=$OUT/octomind-head`.
3. Compare: `python bench/compare_to_baseline.py <dir>/head bench/baseline.json` →
   **guardrail (HEAD success ≥ baseline)**, cost-per-solved, and a per-instance regression flag
   (any instance the baseline solved that HEAD did not). Infra-failed reps are excluded.

## Clean measurement: fresh-repeat on any provider error
A provider hiccup (HTTP 5xx/524, 429, an aborted run, an empty completion) must **not** count as a
task failure — it corrupts the measure. The bench detects it and **repeats the whole test fresh**
(new container, fresh process), up to ~3 retries, then excludes it if still failing. A result is
accepted only if it is a **clean completion**:
```
exit_code == 0  AND  tokens.total > 0  AND  non-empty final turn  AND  no "OctoHub API error" in stderr
```
(`exit_code != 0` is the catch-all for any aborting provider/runtime error.)

**This retry lives in the bench, NOT in octomind.** An in-loop retry inside octomind would add
tokens/partial state to the very run being measured and corrupt cost-per-solved — octomind stays a
clean single-shot under measurement.

## Advancing the baseline
After a build is **verified good** (success ≥ baseline, no per-instance regressions), advance the
baseline to it: rebuild `baseline.json` from the HEAD results and commit it (the old one stays in
git history — that history IS the trend). Exclude non-representative outliers (e.g. a rare runaway
rep) and record them in `meta.note`. Re-run a full paired base+head pass (`aggregate.py`) only to
re-anchor from scratch.

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
