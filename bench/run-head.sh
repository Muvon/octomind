#!/usr/bin/env bash
set -u
HEAD_BIN=${HEAD_BIN:?set HEAD_BIN to the glibc octomind-head binary}
PY=${PY:-.venv/bin/python}
MATRIX=${MATRIX:-configs/run-matrix.octomind-glm.swebench.yaml}
OUT=${OUT:-results-head-0801}
MAXJOBS=${MAXJOBS:-4}; REPS=${REPS:-2}; MAXATT=${MAXATT:-4}
# 2400s since 2026-08-03: at 1800s the chronic instances (cfn-lint, jupyter-ai-1022)
# burn full-quota attempts that get killed unscored — pure waste. The 0801 baseline
# was measured at 1800s; re-anchor the baseline before leaning on solved-rate deltas
# for timeout-sensitive instances.
TIMEOUT=${TIMEOUT:-2400}
read -r -a INSTANCES <<<"${INSTANCES:-aiogram__aiogram-1594 aws-cloudformation__cfn-lint-3749 conan-io__conan-17366 falconry__falcon-2366 instructlab__instructlab-2526 jupyterlab__jupyter-ai-1022 jupyterlab__jupyter-ai-1125 matplotlib__matplotlib-29007 pydata__xarray-9586 run-llama__llama_deploy-330 run-llama__llama_deploy-356 run-llama__llama_deploy-372 run-llama__llama_deploy-384 streamlink__streamlink-6242 tox-dev__tox-3409}"
HERE=$(cd "$(dirname "$0")" && pwd)
LOGDIR=${LOGDIR:-logs-head-0801}

mkdir -p "$OUT/head" "$LOGDIR"

detect_infra() {
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
# Eval-phase flake: pytest died mid-run inside the eval container (truncated
# tests.log), the parser saw ZERO per-test results, and the run got scored
# resolved=False 0/all despite a clean agent phase. Not a genuine miss.
sw = r.get("swebench")
eval_dead = sw is not None and not sw.get("results_seen")
infra = ((res.get("exit_code") or 0) != 0) or (not tot) or fatal or empty_final or eval_dead
print("RETRY" if infra else "OK")
PY
}

cleanup_containers() {
  # Container names truncate the instance to 22 chars (obsweb-<inst:22>-octomi-<ts>),
  # so filtering on the full instance name silently matches nothing and timed-out
  # attempts leave zombie containers that starve the next attempt.
  local n=${1//\//_}
  docker rm -f $(docker ps -q --filter "name=obsweb-${n:0:22}" 2>/dev/null) 2>/dev/null || true
}

run_one() {
  local inst=$1 rep=$2 att out log
  out="$OUT/head/${inst//\//_}-r${rep}"
  for att in $(seq 1 "$MAXATT"); do
    rm -rf "$out"; log="$LOGDIR/${inst//\//_}-r${rep}-a${att}.log"
    echo "[$(date +%H:%M:%S)] START: $inst r$rep att $att (timeout ${TIMEOUT}s)"
    timeout --kill-after=10 "$TIMEOUT" env OCTOBENCH_JUDGE_MODEL=ollama:minimax-m3 OCTOMIND_BIN=$HEAD_BIN "$PY" -m cli.swebench --instance "$inst" --config "$MATRIX" --out "$out" --verbosity quiet >"$log" 2>&1
    local rc=$?
    cleanup_containers "$inst"
    if [ $rc -eq 124 ]; then
      echo "[$(date +%H:%M:%S)] TIMEOUT: $inst r$rep att $att (killed after ${TIMEOUT}s)"
    elif [ "$(detect_infra "$out")" = OK ]; then
      echo "[$(date +%H:%M:%S)] done: $inst r$rep (att $att)"; return
    else
      echo "[$(date +%H:%M:%S)] INFRA: $inst r$rep att $att -> fresh retry (backoff $((att*15))s)"
    fi
    sleep $((att*15))
  done
  echo "[$(date +%H:%M:%S)] done: $inst r$rep GAVEUP-infra"
}

echo "[$(date +%H:%M:%S)] START head-only: $(( ${#INSTANCES[@]} * REPS )) runs, maxjobs=$MAXJOBS, maxatt=$MAXATT, timeout=${TIMEOUT}s"
for inst in "${INSTANCES[@]}"; do for rep in $(seq 1 "$REPS"); do
  while [ "$(jobs -rp | wc -l)" -ge "$MAXJOBS" ]; do sleep 5; done
  run_one "$inst" "$rep" &
done; done
wait
echo "[$(date +%H:%M:%S)] ALL DONE -> compare vs baseline:"
"$PY" "$HERE/compare_to_baseline.py" "$OUT/head" "$HERE/baseline.json"
