#!/usr/bin/env python3
"""Compare a HEAD-only octobench result set against the frozen base arm in baseline.json.

Routine regression check = run HEAD only, compare to the recorded base (no base re-run).
Infra failures (provider 5xx/429/524, aborted run with null tokens, empty completion) are
EXCLUDED from HEAD's numbers — they are not genuine task misses.

Usage:  compare_to_baseline.py <head_results_dir> <baseline.json>
  head layout: <head_results_dir>/<instance>-r<rep>/<timestamp>/results.json
"""
import json, glob, sys, re, statistics as st

head_dir = sys.argv[1] if len(sys.argv) > 1 else "results-headonly/head"
base_file = sys.argv[2] if len(sys.argv) > 2 else "baseline.json"

bl = json.load(open(base_file))
base_arm = bl["arms"]["base"]
base_task = bl.get("per_task", {})

def is_infra(r):
    res = r.get("result") or {}
    tot = (r.get("tokens") or {}).get("total")
    stderr = res.get("stderr") or ""
    out = res.get("stdout") or ""
    fatal = bool(re.search(r'OctoHub API error|error code: 5\d\d|Too Many Requests| 429', stderr))
    empty_final = '"type":"assistant","content":""' in out[-600:]
    return ((res.get("exit_code") or 0) != 0) or (not tot) or fatal or empty_final

rows = []
for f in glob.glob(head_dir + "/*/*/results.json"):
    inst = f.split("/")[-3].rsplit("-r", 1)[0]
    try:
        r = json.load(open(f))["results"][0]
    except Exception:
        continue
    sw = r.get("swebench", {}) or {}
    rows.append(dict(inst=inst, resolved=bool(sw.get("resolved")),
                     total=(r.get("tokens") or {}).get("total") or 0,
                     cost=r.get("cost_usd") or 0.0,
                     judge=(r.get("judge") or {}).get("score") or 0,
                     infra=is_infra(r)))

genuine = [x for x in rows if not x["infra"]]
infra_n = sum(1 for x in rows if x["infra"])
med = lambda xs: st.median(xs) if xs else 0
mean = lambda xs: st.mean(xs) if xs else 0
solved = [x for x in genuine if x["resolved"]]
hn, hs = len(genuine), len(solved)
hcps = sum(x["cost"] for x in genuine) / hs if hs else float("inf")

print(f"reference (baseline.json {bl.get('meta', {}).get('date')}): {bl.get('meta', {}).get('ref', 'baseline')}")
print("candidate = fresh HEAD run\n")
print(f"{'ARM':22} {'n':>3} {'solved':>6} {'succ%':>6} {'$/solved':>9} {'medTok(solv)':>13} {'judge':>6}")
print(f"{'baseline (file)':22} {base_arm['n']:>3} {base_arm['solved']:>6} "
      f"{base_arm['success_rate']*100:>5.0f}% {base_arm['cost_per_solved']:>9.2f} "
      f"{base_arm.get('median_tokens_solved',0):>13,.0f} {base_arm.get('avg_judge',0):>6.1f}")
print(f"{'HEAD (fresh)':22} {hn:>3} {hs:>6} {hs/hn*100 if hn else 0:>5.0f}% {hcps:>9.2f} "
      f"{med([x['total'] for x in solved]):>13,.0f} {mean([x['judge'] for x in genuine]):>6.1f}   [infra-excluded: {infra_n}]")

print("\nper-instance (frozen base -> fresh head, infra excluded):")
for inst in sorted(set(x["inst"] for x in rows) | set(base_task.keys())):
    b = base_task.get(inst, {}).get("base", {})
    hsx = [x for x in genuine if x["inst"] == inst]
    hsol = sum(1 for x in hsx if x["resolved"])
    bsol, bn = b.get("solved"), b.get("n")
    flag = "   <-- REGRESSION? base solved, head 0/N" if (bsol and bsol >= 1 and hsx and hsol == 0) else ""
    print(f"  {inst:34} base {bsol}/{bn} -> head {hsol}/{len(hsx)}{flag}")

print(f"\nGUARDRAIL: head success {hs/hn*100 if hn else 0:.0f}% vs base {base_arm['success_rate']*100:.0f}%  "
      f"| cost/solved head {hcps:.2f} vs base {base_arm['cost_per_solved']:.2f}")
