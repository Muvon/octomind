#!/usr/bin/env python3
"""Aggregate octobench pilot results into a paired 0.32.0-vs-HEAD comparison.

Reports token-efficiency-WITH-success AND quality (judge), not just pass/fail:
  per arm  : success rate, median tokens (all + solved-only), cost-per-solved,
             avg judge score (quality), median time.
  per task : base->head median tokens + ratio (the "is the 3x real?" view).

Usage (on the box):  python3 aggregate.py [results-pilot]
Layout expected:     <root>/<arm>/<instance>-r<rep>/<timestamp>/results.json
"""
import json, glob, statistics as st, sys

root = sys.argv[1] if len(sys.argv) > 1 else "results-pilot"
rows = []
for f in glob.glob(f"{root}/*/*/*/results.json"):
    parts = f.split("/")
    arm, dirn = parts[-4], parts[-3]            # results-pilot/ARM/INSTANCE-rREP/TS/results.json
    try:
        r = json.load(open(f))["results"][0]
    except Exception:
        continue
    sw, t = r.get("swebench", {}), r.get("tokens", {})
    rows.append(dict(
        arm=arm,
        inst=dirn.rsplit("-r", 1)[0],
        resolved=bool(sw.get("resolved")),
        total=t.get("total") or 0,
        cost=r.get("cost_usd") or 0.0,
        ms=(r.get("result") or {}).get("elapsed_ms") or 0,
        judge=(r.get("judge") or {}).get("score") or 0,
    ))

if not rows:
    print(f"no results under {root}/*/*/*/results.json"); sys.exit(1)

med = lambda xs: st.median(xs) if xs else 0
mean = lambda xs: st.mean(xs) if xs else 0

print(f"{'ARM':5} {'n':>3} {'solved':>6} {'succ%':>6} {'medTok':>10} {'medTok(solv)':>13} {'$/solved':>9} {'avgJudge':>8} {'med_s':>6}")
for arm in sorted({x["arm"] for x in rows}):
    g = [x for x in rows if x["arm"] == arm]
    solved = [x for x in g if x["resolved"]]
    cps = sum(x["cost"] for x in g) / len(solved) if solved else float("inf")
    print(f"{arm:5} {len(g):>3} {len(solved):>6} {len(solved)/len(g)*100:>5.0f}% "
          f"{med([x['total'] for x in g]):>10,.0f} {med([x['total'] for x in solved]):>13,.0f} "
          f"{cps:>9.2f} {mean([x['judge'] for x in g]):>8.1f} {med([x['ms'] for x in g])/1000:>6.0f}")

print("\nper-task median tokens (base -> head):   [the 'is 3x real / our-update?' view]")
for inst in sorted({x["inst"] for x in rows}):
    b = [x["total"] for x in rows if x["inst"] == inst and x["arm"] == "base" and x["resolved"]]
    h = [x["total"] for x in rows if x["inst"] == inst and x["arm"] == "head" and x["resolved"]]
    jb = [x["judge"] for x in rows if x["inst"] == inst and x["arm"] == "base"]
    jh = [x["judge"] for x in rows if x["inst"] == inst and x["arm"] == "head"]
    if b and h:
        print(f"  {inst:34} {med(b):>9,.0f} -> {med(h):>9,.0f}  ({med(h)/med(b):.2f}x)  "
              f"judge {mean(jb):.0f}->{mean(jh):.0f}  (solved b={len(b)} h={len(h)})")
    else:
        print(f"  {inst:34} INCOMPLETE  base_solved={len(b)} head_solved={len(h)}")
