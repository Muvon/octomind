#!/usr/bin/env python3
"""Summarize one octobench long-run octomind run dir: per-turn verdict/tokens, fold events, tool-call habits.
usage: analyze_run.py <out-dir>   (the results-* dir; picks every <ts>/<seq>/octomind__* under it)"""
import sys, json, re, subprocess, glob, os
from collections import Counter, defaultdict

def load_zst(path):
    out = subprocess.run(["zstd", "-dc", path], capture_output=True, check=True).stdout.decode("utf-8", "replace")
    return [json.loads(l) for l in out.splitlines() if l.strip()]

FOLD_PATTERNS = {
    "fold_spawned": r"Compression decision spawned in background",
    "fold_applied": r"Turn finished with a completed background fold|Compression applied|compressed .* messages|fold landed",
    "fold_failed": r"Background fold call failed",
    "fold_cancelled": r"Background fold cancelled",
    "fold_discarded": r"Background fold discarded",
    "fold_deferred": r"decision model deferred the fold",
    "fold_no_summary": r"returned no substantive summary",
    "fold_exit_wait": r"Run ending with a background fold in flight",
    "fold_exit_abandon": r"did not finish within .*before exit",
    "compression_failed": r"Conversation compression failed",
    "forced_ceiling": r"ceiling margin",
    "cut_oversized": r"Cut \d+ oversized tool result",
    "below_fire_line": r"Below compression fire line",
    "adaptive_fire_line": r"Adaptive compression fire line",
    "schema_invalid": r"Schema validation failed|invalid or unparseable",
    "condense": r"[Cc]ondens",
    "gate_verdict": r"gate verdict|GAPS|verify-gate",
}

def analyze(run_dir):
    print(f"\n### {run_dir}")
    results = None
    ts_dir = os.path.dirname(os.path.dirname(run_dir))
    for cand in glob.glob(os.path.join(ts_dir, "results*.json")):
        try:
            results = json.load(open(cand)); break
        except Exception: pass
    turns = sorted(glob.glob(os.path.join(run_dir, "turns", "turn_*")), key=lambda p: int(p.rsplit("_", 1)[1]))
    print(f"turns with logs: {len(turns)}")
    tot = Counter()
    for t in turns:
        n = t.rsplit("_", 1)[1]
        logs = os.path.join(t, "logs")
        stderr = open(os.path.join(logs, "provider.stderr.log"), errors="replace").read() if os.path.exists(os.path.join(logs, "provider.stderr.log")) else ""
        ev = {k: len(re.findall(p, stderr)) for k, p in FOLD_PATTERNS.items()}
        ev = {k: v for k, v in ev.items() if v}
        for k, v in ev.items(): tot[k] += v
        cost = None
        raw = os.path.join(logs, "provider.raw.jsonl")
        calls = 0
        if os.path.exists(raw):
            for line in open(raw, errors="replace"):
                try: r = json.loads(line)
                except Exception: continue
                if r.get("type") == "cost": cost = r
        val = ""
        vs = os.path.join(logs, "validate.stdout.log")
        if os.path.exists(vs):
            txt = open(vs, errors="replace").read()
            m = re.findall(r"(\d+ passed|\d+ failed|\d+ error)", txt)
            val = " ".join(m[-3:])
        elapsed = ""
        print(f"- turn {n}: validate[{val}] cost_rec={json.dumps({k: cost[k] for k in ('session_tokens','input_tokens','output_tokens','cache_read_tokens','session_cost') if cost and k in cost}) if cost else None} events={ev}")
    print("event totals:", dict(tot))
    # session-level
    for zf in glob.glob(os.path.join(run_dir, "state", "octomind", "sessions", "*.jsonl.zst")):
        recs = load_zst(zf)
        # The session log re-appends the surviving messages after every
        # COMPRESSION_POINT / RESTORATION_POINT (resume snapshot), so the same
        # message can appear several times. Dedupe by identity: tool_call ids for
        # assistant/tool messages, (role, timestamp, content) otherwise.
        msgs = []; seen = set()
        for r in recs:
            if "role" not in r: continue
            if r.get("tool_calls"): key = ("a", tuple(tc.get("id") for tc in r["tool_calls"]))
            elif r.get("tool_call_id"): key = ("t", r["tool_call_id"])
            else: key = (r["role"], r.get("timestamp"), r.get("content", "")[:200])
            if key in seen: continue
            seen.add(key); msgs.append(r)
        cps = [r for r in recs if r.get("type") in ("COMPRESSION_POINT", "RESTORATION_POINT")]
        print(f"  compression points: {len(cps)} {[ (c.get('compression_type'), c.get('messages_removed'), c.get('tokens_saved')) for c in cps]}")
        stats = [r for r in recs if r.get("type") == "STATS"]
        summaries = [r for r in recs if r.get("type") == "SUMMARY"]
        roles = Counter(m["role"] for m in msgs)
        tools = Counter(); views = []
        for m in msgs:
            for tc in m.get("tool_calls") or []:
                name = tc.get("name") or (tc.get("function") or {}).get("name"); tools[name] += 1
                if name == "view": views.append(json.dumps(tc.get("arguments"), sort_keys=True))
        vpaths = Counter()
        for v in views:
            try: vpaths[json.loads(v).get("path")] += 1
            except Exception: pass
        rereads = sum(c - 1 for c in vpaths.values() if c > 1)
        exact_dupes = sum(c - 1 for c in Counter(views).values() if c > 1)
        print(f"session {os.path.basename(zf)}: msgs={len(msgs)} roles={dict(roles)} SUMMARY={len(summaries)} STATS={len(stats)}")
        print(f"  tool calls: {tools.most_common(10)}")
        print(f"  view: {len(views)} calls over {len(vpaths)} paths; re-reads(same path)={rereads} exact-dupes={exact_dupes}; top: {vpaths.most_common(5)}")
        if stats:
            s = stats[-1]
            keep = {k: s.get(k) for k in s if k not in ("type",)}
            print("  last STATS:", json.dumps(keep)[:1500])
        for sm in summaries[-2:]:
            print("  SUMMARY rec keys:", list(sm.keys())[:12], "| tokens_before/after:", sm.get("tokens_before"), sm.get("tokens_after"), sm.get("original_tokens"), sm.get("summary_tokens"))

if __name__ == "__main__":
    for out in sys.argv[1:]:
        for run in sorted(glob.glob(os.path.join(out, "*", "*", "octomind__*"))):
            analyze(run)
