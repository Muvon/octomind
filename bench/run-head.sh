#!/usr/bin/env bash
# Routine HEAD-only bench run vs bench/baseline.json. See bench/README.md.
#
# Run it FROM the octobench instrument dir (so `cli.swebench` + its venv resolve), with a
# pre-built HEAD glibc binary. Repeats any provider-error run FRESH (bench-level, not octomind):
# a result is kept only if it is a clean completion (exit 0, real tokens, non-empty final turn,
# no OctoHub API error). Anything else is discarded and the whole test is re-run fresh.
#
# Env (override as needed):
#   HEAD_BIN  - path to the glibc octomind-head binary (required)
#   PY        - python with cli.swebench available           (default: .venv/bin/python)
#   MATRIX    - octobench run-matrix config                  (default: glm swebench matrix)
#   OUT       - results dir                                   (default: results-headonly)
#   MAXJOBS/REPS/MAXATT - concurrency / reps-per-instance / max fresh attempts (default 4/2/4)
#   INSTANCES - space-separated instance ids (default: the 7-instance pilot)
set -u
HEAD_BIN=${HEAD_BIN:?set HEAD_BIN to the glibc octomind-head binary}
PY=${PY:-.venv/bin/python}
MATRIX=${MATRIX:-configs/run-matrix.octomind-glm.swebench.yaml}
OUT=${OUT:-results-headonly}
MAXJOBS=${MAXJOBS:-4}; REPS=${REPS:-2}; MAXATT=${MAXATT:-4}
read -r -a INSTANCES <<<"${INSTANCES:-instructlab__instructlab-2526 jupyterlab__jupyter-ai-1022 jupyterlab__jupyter-ai-1125 run-llama__llama_deploy-330 run-llama__llama_deploy-356 run-llama__llama_deploy-372 run-llama__llama_deploy-384}"
HERE=$(cd "$(dirname "$0")" && pwd)   # bench/ dir (compare_to_baseline.py + baseline.json live here)

rm -rf "$OUT" logs-headonly; mkdir -p "$OUT/head" logs-headonly

detect_infra() {   # arg: out dir. prints RETRY (provider error -> repeat fresh) or OK (clean -> keep)
  "$PY" - "$1" <<'PY'
import json, sys, glob, re
fs = glob.glob(sys.argv[1] + "/*/results.json")
if not fs:
    print("RETRY"); raise SystemExit
try:
    r = json.load(open(fs[0]))["results"][0]
except Exception:
    print("RETRY"); raise SystemExit
res = r.get("result") or {}
tot = (r.get("tokens") or {}).get("total")
stderr = res.get("stderr") or ""
out = res.get("stdout") or ""
fatal = bool(re.search(r'OctoHub API error|error code: 5\d\d|Too Many Requests| 429', stderr))
empty_final = '"type":"assistant","content":""' in out[-600:]
infra = ((res.get("exit_code") or 0) != 0) or (not tot) or fatal or empty_final
print("RETRY" if infra else "OK")
PY
}

run_one() {
  local inst=$1 rep=$2 att out log
  out="$OUT/head/${inst//\//_}-r${rep}"
  for att in $(seq 1 "$MAXATT"); do
    rm -rf "$out"; log="logs-headonly/${inst//\//_}-r${rep}-a${att}.log"
    OCTOMIND_BIN=$HEAD_BIN "$PY" -m cli.swebench --instance "$inst" --config "$MATRIX" --out "$out" --verbosity quiet >"$log" 2>&1
    if [ "$(detect_infra "$out")" = OK ]; then echo "[$(date +%H:%M:%S)] done: $inst r$rep (att $att)"; return; fi
    echo "[$(date +%H:%M:%S)] INFRA: $inst r$rep att $att -> fresh retry (backoff $((att*15))s)"; sleep $((att*15))
  done
  echo "[$(date +%H:%M:%S)] done: $inst r$rep GAVEUP-infra"
}

echo "[$(date +%H:%M:%S)] START head-only: $(( ${#INSTANCES[@]} * REPS )) runs, maxjobs=$MAXJOBS, maxatt=$MAXATT"
for inst in "${INSTANCES[@]}"; do for rep in $(seq 1 "$REPS"); do
  while [ "$(jobs -rp | wc -l)" -ge "$MAXJOBS" ]; do sleep 5; done
  run_one "$inst" "$rep" &
done; done
wait
echo "[$(date +%H:%M:%S)] ALL DONE -> compare vs baseline:"
"$PY" "$HERE/compare_to_baseline.py" "$OUT/head" "$HERE/baseline.json"
