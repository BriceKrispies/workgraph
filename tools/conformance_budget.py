#!/usr/bin/env python3
"""Choose which conformance scenarios to keep under the 2-minute test budget.

Reads a `wg-conformance check` report JSON (files[].scenario, duration_ms), applies
duration overrides, then does a budget-capped weighted greedy set cover over each scenario's `covers`
ids (cost = duration) using only scenarios no longer than --max-single-ms.  Ids that only
over-long scenarios cover are reported as lost.  With --apply, deletes the unselected
scenario files and goldens and rewrites the generated block of coverage-allowlist.toml.

  python tools/conformance_budget.py REPORT.json [--override ID=MS ...] [--apply]
"""
import argparse, json, pathlib, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent / "tests" / "conformance"
BEGIN = "# BEGIN conformance_budget (generated)"
END = "# END conformance_budget"
REASON = "cut by the 2-minute test budget (2026-10-30)"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("report")
    ap.add_argument("--override", action="append", default=[], help="ID=MS duration override")
    ap.add_argument("--max-single-ms", type=int, default=30000)
    ap.add_argument("--budget-ms", type=int, default=480000)
    ap.add_argument("--apply", action="store_true")
    a = ap.parse_args()

    dur = {f["scenario"]: f["duration_ms"] for f in json.load(open(a.report, encoding="utf-8"))["files"]}
    for o in a.override:
        k, v = o.rsplit("=", 1)
        dur[k] = int(v)
    sc = {}
    for p in ROOT.joinpath("scenarios").rglob("*.json"):
        d = json.load(open(p, encoding="utf-8"))
        sc[d["id"]] = (p, set(d.get("covers", [])))
    missing = [i for i in sc if i not in dur]
    if missing:
        sys.exit(f"no duration for: {missing}")
    allids = set().union(*(c for _, c in sc.values()))
    elig = {i for i in sc if dur[i] <= a.max_single_ms}
    coverable = set().union(*(sc[i][1] for i in elig))
    lost = sorted(allids - coverable)
    kept, covered, total = [], set(), 0
    while True:
        cand = [((dur[i] + 1) / len(sc[i][1] - covered), i) for i in elig
                if i not in kept and sc[i][1] - covered and total + dur[i] <= a.budget_ms]
        if not cand:
            break
        best = min(cand)[1]
        kept.append(best)
        covered |= sc[best][1]
        total += dur[best]
    lost = sorted(allids - covered)
    print(f"kept {len(kept)} deleted {len(sc) - len(kept)} sum {total/1000:.1f}s "
          f"max {max(dur[i] for i in kept)/1000:.1f}s budget {a.budget_ms/1000:.0f}s")
    print("lost ids:", ", ".join(lost))
    if total > a.budget_ms:
        print("WARNING: over budget")
    if a.apply:
        for i, (p, _) in sc.items():
            if i not in kept:
                p.unlink()
                g = ROOT / "golden" / f"{i}.json"
                if g.exists():
                    g.unlink()
        al = ROOT / "coverage-allowlist.toml"
        t = al.read_text(encoding="utf-8")
        if BEGIN in t:
            t = t[:t.index(BEGIN)].rstrip("\n") + "\n"
        block = "".join(f'\n[[exclusion]]\nid = "{i}"\nreason = "{REASON}"\n' for i in lost)
        al.write_text(t + f"\n{BEGIN}\n{block}{END}\n", encoding="utf-8")


main()
