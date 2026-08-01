#!/usr/bin/env python3
"""Full comparison table: HEAD vs baseline, all metrics per instance."""
import json, glob, sys, re, statistics as st
from datetime import datetime

head_dir = sys.argv[1] if len(sys.argv) > 1 else "results-head-0801/head"
base_file = sys.argv[2] if len(sys.argv) > 2 else "baseline.json"
out_file = sys.argv[3] if len(sys.argv) > 3 else "/tmp/bench_comparison.txt"

bl = json.load(open(base_file))
base_arm = bl["arms"]["base"]
base_task = bl.get("per_task", {})

def is_infra(r):
    res = r.get("result") or {}
    tot = (r.get("tokens") or {}).get("total")
    stderr = res.get("stderr") or ""
    out = res.get("stdout") or ""
    fatal = bool(re.search(r"OctoHub API error|error code: 5\d\d|Too Many Requests| 429", stderr))
    empty_final = '"type":"assistant","content":""' in out[-600:]
    return ((res.get("exit_code") or 0) != 0) or (not tot) or fatal or empty_final

# Collect HEAD results
head_runs = {}
for f in glob.glob(head_dir + "/*/*/results.json"):
    inst = f.split("/")[-3].rsplit("-r", 1)[0]
    rep = f.split("/")[-3].rsplit("-r", 1)[1]
    try:
        r = json.load(open(f))["results"][0]
    except Exception:
        continue
    sw = r.get("swebench", {}) or {}
    tk = r.get("tokens") or {}
    judge = r.get("judge") or {}
    scoring = r.get("scoring", {}) or {}
    res = r.get("result") or {}
    head_runs.setdefault(inst, []).append({
        "rep": rep,
        "resolved": bool(sw.get("resolved")),
        "fail_to_pass": sw.get("fail_to_pass", "?"),
        "pass_to_pass": sw.get("pass_to_pass", "?"),
        "tokens_input": tk.get("input", 0),
        "tokens_output": tk.get("output", 0),
        "tokens_reasoning": tk.get("reasoning", 0),
        "tokens_total": tk.get("total", 0),
        "cost_usd": r.get("cost_usd", 0),
        "judge_score": judge.get("score", 0),
        "judge_error": judge.get("_judge_parse_error", False),
        "elapsed_ms": res.get("elapsed_ms", 0),
        "exit_code": res.get("exit_code", -1),
        "infra": is_infra(r),
        "efficiency_score": scoring.get("efficiency_score", 0),
        "final_score": scoring.get("final_score", 0),
    })

# All instances (union of baseline and head)
all_insts = sorted(set(base_task.keys()) | set(head_runs.keys()))

lines = []
lines.append("=" * 120)
lines.append("HEAD vs BASELINE COMPARISON - generated " + datetime.now().strftime("%Y-%m-%d %H:%M:%S"))
lines.append("=" * 120)
lines.append("")

# Summary section
genuine = []
for inst, runs in head_runs.items():
    for r in runs:
        if not r["infra"]:
            genuine.append(r)
infra_n = sum(1 for inst, runs in head_runs.items() for r in runs if r["infra"])
total_runs = sum(len(runs) for runs in head_runs.values())
solved = [x for x in genuine if x["resolved"]]
hn, hs = len(genuine), len(solved)
hcps = sum(x["cost_usd"] for x in genuine) / hs if hs else float("inf")
med_tok_solved = st.median([x["tokens_total"] for x in solved]) if solved else 0
avg_judge = st.mean([x["judge_score"] for x in genuine]) if genuine else 0

lines.append("SUMMARY")
lines.append("-" * 120)
lines.append("{:30} {:>15} {:>15} {:>15}".format("Metric", "Baseline", "HEAD", "Delta"))
lines.append("{:30} {:>15} {:>15} {:>+15}".format("Runs", base_arm["n"], total_runs, total_runs - base_arm["n"]))
lines.append("{:30} {:>15} {:>15} {:>+15}".format("Genuine (non-infra)", base_arm["n"], hn, hn - base_arm["n"]))
lines.append("{:30} {:>15} {:>15} {:>+15}".format("Solved", base_arm["solved"], hs, hs - base_arm["solved"]))
pct_b = base_arm["success_rate"] * 100
pct_h = hs / hn * 100 if hn else 0
lines.append("{:30} {:>14.0f}% {:>14.0f}% {:>+14.0f}%".format("Success rate", pct_b, pct_h, pct_h - pct_b))
lines.append("{:30} {:>15.2f} {:>15.2f} {:>+15.2f}".format("Cost per solved ($)", base_arm["cost_per_solved"], hcps if hs else 0, (hcps if hs else 0) - base_arm["cost_per_solved"]))
lines.append("{:30} {:>15,.0f} {:>15,.0f} {:>+15,.0f}".format("Median tokens (solved)", base_arm.get("median_tokens_solved", 0), med_tok_solved, med_tok_solved - base_arm.get("median_tokens_solved", 0)))
lines.append("{:30} {:>15.1f} {:>15.1f} {:>+15.1f}".format("Avg judge score", base_arm.get("avg_judge", 0), avg_judge, avg_judge - base_arm.get("avg_judge", 0)))
lines.append("{:30} {:>15} {:>15}".format("Infra-excluded runs", "N/A", infra_n))
lines.append("{:30} {:>15} {:>15}".format("Median seconds (baseline)", base_arm.get("median_sec", 0), "TBD"))
lines.append("")

# Per-instance table
lines.append("PER-INSTANCE COMPARISON")
lines.append("-" * 120)
header = "{:34} {:>5} {:>4} {:>10} {:>5} | {:>5} {:>4} {:>10} {:>5} {:>8} {:>7} {:>5} {:>12}".format(
    "Instance", "B_slv", "B_n", "B_tok", "B_jdg", "H_slv", "H_n", "H_tok", "H_jdg", "H_cost", "H_sec", "H_inf", "Status")
lines.append(header)
lines.append("-" * 120)

for inst in all_insts:
    b = base_task.get(inst, {}).get("base", {})
    bsol, bn = b.get("solved", "?"), b.get("n", "?")
    btok = b.get("median_tokens", 0)
    bjudge = b.get("avg_judge", 0)

    hruns = head_runs.get(inst, [])
    hgenuine = [r for r in hruns if not r["infra"]]
    hsol = sum(1 for r in hgenuine if r["resolved"])
    hn_inst = len(hgenuine)
    htok = st.median([r["tokens_total"] for r in hgenuine]) if hgenuine else 0
    hjudge = st.mean([r["judge_score"] for r in hgenuine]) if hgenuine else 0
    hcost = sum(r["cost_usd"] for r in hgenuine) if hgenuine else 0
    hsec = st.median([r["elapsed_ms"] / 1000 for r in hgenuine]) if hgenuine else 0
    hinf = sum(1 for r in hruns if r["infra"])

    if not hruns:
        status = "PENDING"
    elif hinf > 0 and not hgenuine:
        status = "ALL_INFRA"
    elif bsol and bsol >= 1 and hsol == 0:
        status = "REGRESSION!"
    elif bsol == 0 and hsol > 0:
        status = "IMPROVED!"
    elif hsol > 0 and bsol and hsol >= bsol:
        status = "OK"
    elif hsol > 0:
        status = "PARTIAL"
    else:
        status = "UNSOLVED"

    lines.append("{:34} {:>5} {:>4} {:>10,.0f} {:>5.1f} | {:>5} {:>4} {:>10,.0f} {:>5.1f} {:>8.2f} {:>7.0f} {:>5} {:>12}".format(
        inst, str(bsol), str(bn), btok, bjudge, hsol, hn_inst, htok, hjudge, hcost, hsec, hinf, status))

lines.append("-" * 120)
lines.append("")

# Detailed per-run breakdown
lines.append("DETAILED PER-RUN BREAKDOWN (HEAD)")
lines.append("-" * 120)
header2 = "{:34} {:>4} {:>8} {:>6} {:>10} {:>10} {:>8} {:>6} {:>8} {:>5}".format(
    "Instance", "Rep", "Resolved", "F2P", "P2P", "Tokens", "Cost$", "Judge", "Elapsed", "Infra")
lines.append(header2)
lines.append("-" * 120)

for inst in sorted(head_runs.keys()):
    for r in sorted(head_runs[inst], key=lambda x: int(x["rep"])):
        f2p = str(r["fail_to_pass"] or "?")
        p2p = str(r["pass_to_pass"] or "?")
        tok = r["tokens_total"] or 0
        cost = r["cost_usd"] or 0.0
        jdge = r["judge_score"] or 0
        secs = (r["elapsed_ms"] or 0) / 1000
        lines.append("{:34} r{:>3} {:>8} {:>6} {:>10} {:>10,.0f} {:>8.2f} {:>6.1f} {:>7.0f}s {:>5}".format(
            inst, r["rep"], str(r["resolved"]), f2p, p2p,
            tok, cost, jdge, secs,
            "YES" if r["infra"] else "no"))

lines.append("-" * 120)
lines.append("")
lines.append("Baseline meta:")
lines.append(json.dumps(bl.get("meta", {}), indent=2))
lines.append("")

output = "\n".join(lines)
with open(out_file, "w") as f:
    f.write(output)
print(output)