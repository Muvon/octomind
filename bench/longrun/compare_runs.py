#!/usr/bin/env python3
"""Per-turn table from octobench longrun results.json files: compare_runs.py <label=out-dir> ..."""
import sys, json, glob, os, re
from collections import Counter
FOLD = {"spawned": r"Compression decision spawned in background", "landed": r"Compressed \d+ messages", "length_cut": r"finish_reason=Some\(\"length\"\)", "failed": r"Background fold call failed", "exit_wait": r"Run ending with a background fold in flight", "forced_ceiling": r"ceiling margin reached", "repair": r"retrying once with reasoning off", "hard_error": r"context remains above the usable ceiling"}
for arg in sys.argv[1:]:
    label, out = arg.split("=", 1) if "=" in arg else (arg, arg)
    for res in sorted(glob.glob(os.path.join(out, "*", "results.json"))):
        d = json.load(open(res))
        seqs = [d] if "turns" in d else [v for v in (d.get("sequences") or d.get("results") or []) if isinstance(v, dict) and "turns" in v]
        for seq in seqs:
            run_dir = glob.glob(os.path.join(os.path.dirname(res), seq.get("sequence_id", "*"), "octomind__*"))
            run_dir = run_dir[0] if run_dir else None
            print(f"\n## {label}: {seq.get('sequence_id')} ({seq.get('provider_model')})")
            print("| turn | pass | total tok | uncached in | cached | out | reasoning | est/real | folds |")
            print("|---|---|---|---|---|---|---|---|---|")
            tot = Counter()
            for t in seq["turns"]:
                tk = t.get("tokens") or {}
                ok = (t.get("validation") or {}).get("passed")
                n = t.get("turn")
                ev = Counter(); ratio = ""
                if run_dir:
                    se = os.path.join(run_dir, "turns", f"turn_{n}", "logs", "provider.stderr.log")
                    if os.path.exists(se):
                        txt = re.sub(r"\x1b\[[0-9;]*m", "", open(se, errors="replace").read())
                        for k, p in FOLD.items():
                            c = len(re.findall(p, txt))
                            if c: ev[k] = c
                        est = None; pairs = []
                        for m in re.finditer(r"(Below compression fire line \(current: (\d+)|fire line reached: current=(\d+)|Provider usage \[[a-z-]+\]: provider=\w+, input=(\d+), output=\d+, cache_read=(\d+))", txt):
                            if m.group(2) or m.group(3): est = int(m.group(2) or m.group(3))
                            elif est: pairs.append(est / max(1, int(m.group(4)) + int(m.group(5))))
                        if pairs: ratio = f"{min(pairs):.2f}–{max(pairs):.2f}"
                print(f"| {n} | {'PASS' if ok else 'FAIL'} | {(tk.get('total') or 0)/1e6:.2f}M | {(tk.get('input') or 0)//1000}k | {(tk.get('cached_input') or 0)/1e6:.2f}M | {(tk.get('output') or 0)//1000}k | {(tk.get('reasoning') or 0)//1000}k | {ratio} | {dict(ev)} |")
                for k in ("total", "input", "cached_input", "output", "reasoning"): tot[k] += tk.get(k, 0) or 0
                tot["pass"] += 1 if ok else 0
            a = seq.get("aggregate") or {}
            print(f"| **all** | {tot['pass']}/{len(seq['turns'])} | {tot['total']/1e6:.2f}M | {tot['input']//1000}k | {tot['cached_input']/1e6:.2f}M | {tot['output']//1000}k | {tot['reasoning']//1000}k | | cost_usd={a.get('total_cost_usd')} |")
