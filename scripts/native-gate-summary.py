#!/usr/bin/env python3
"""Summarize the reports of scripts/native-gate.sh.

Pools the raw samples of every round of a document size, per phase, and
prints and writes summary.json / summary.md: p50, p95, p99 and max of the
frame interval, the update CPU time and the process CPU time; the ratio of the
largest document to the smallest; and the verdict for each criterion of the
plan, which is "not proven" unless the numbers show it.

Criteria (PLAN.md, phase 2):
  - ratio: a frame of the largest document costs at most 1.5x one of the
    smallest. Both the update time (CPU of the frame's systems) and the frame
    interval are compared, p50 and p95.
  - scroll at 120 fps: the interval between frames of the scrolling phases has
    p50 within 5% of 1000/120 ms, p99 at most 1.5 frames, and the display
    really runs at 120 Hz (otherwise 120 fps cannot be shown and the gate
    stays unproven).
"""
import glob
import json
import math
import os
import re
import sys

SCROLL = ["scroll_down", "scroll_up", "scroll_middle"]
METRICS = ["interval_ms", "update_ms", "cpu_ms", "render_interval_ms"]


def pct(sorted_values, p):
    if not sorted_values:
        return math.nan
    rank = max(1, math.ceil(p / 100 * len(sorted_values)))
    return sorted_values[min(rank, len(sorted_values)) - 1]


def stats(values):
    s = sorted(values)
    return {
        "n": len(s),
        "p50": pct(s, 50),
        "p95": pct(s, 95),
        "p99": pct(s, 99),
        "max": s[-1] if s else math.nan,
    }


def main(out):
    runs = {}
    for path in sorted(glob.glob(os.path.join(out, "blocks-*-round-*.json"))):
        m = re.search(r"blocks-(\d+)-round-(\d+)\.json", path)
        with open(path) as f:
            runs.setdefault(int(m.group(1)), []).append((int(m.group(2)), json.load(f)))
    summary = {"sizes": {}, "criteria": {}}
    for blocks, items in sorted(runs.items()):
        pooled = {}
        for _, report in items:
            for phase, data in report["phases"].items():
                for metric in METRICS:
                    pooled.setdefault(phase, {}).setdefault(metric, []).extend(data["raw"][metric])
        first = items[0][1]
        loads = []
        for round_no, _ in items:
            lp = os.path.join(out, f"blocks-{blocks}-round-{round_no}.load")
            if os.path.exists(lp):
                loads.append(open(lp).read().strip().replace("\n", " | "))
        summary["sizes"][blocks] = {
            "rounds": len(items),
            "monitors": first["monitors"],
            "window": first["window"],
            "document": first["document"],
            "scrolled_px_total_per_round": [r["scrolled_px_total"] for _, r in items],
            "frames_total_per_round": [r["frames_total"] for _, r in items],
            "load": loads,
            "phases": {
                phase: {metric: stats(v) for metric, v in metrics.items()}
                for phase, metrics in pooled.items()
            },
            "scroll_pooled": {
                metric: stats(sum((pooled[p][metric] for p in SCROLL if p in pooled), []))
                for metric in METRICS
            },
            "scroll_frames_over_16.7ms": sum(
                1 for p in SCROLL if p in pooled for v in pooled[p]["interval_ms"] if v > 1000 / 60
            ),
            "scroll_frames": sum(len(pooled[p]["interval_ms"]) for p in SCROLL if p in pooled),
            "per_round_worst": [
                {
                    "round": round_no,
                    "interval_max_ms": max(
                        (data["interval_ms"]["max"] for ph, data in r["phases"].items() if ph != "open"),
                        default=math.nan,
                    ),
                    "update_max_ms": max(
                        (data["update_ms"]["max"] for ph, data in r["phases"].items() if ph != "open"),
                        default=math.nan,
                    ),
                }
                for round_no, r in items
            ],
        }

    sizes = sorted(summary["sizes"])
    lo, hi = (sizes[0], sizes[-1]) if sizes else (None, None)
    if lo is not None and hi != lo:
        a, b = summary["sizes"][lo], summary["sizes"][hi]
        ratios = {}
        for metric in ("update_ms", "interval_ms"):
            for p in ("p50", "p95"):
                for phase in ("idle", "scroll_pooled"):
                    src = lambda s: (s["scroll_pooled"] if phase == "scroll_pooled" else s["phases"][phase])[metric][p]
                    ratios[f"{phase}.{metric}.{p}"] = src(b) / src(a) if src(a) else math.nan
        summary["criteria"]["ratio_largest_over_smallest"] = {
            "smallest": lo,
            "largest": hi,
            "ratios": ratios,
            "limit": 1.5,
            "met_update_p50": ratios["scroll_pooled.update_ms.p50"] <= 1.5,
            "met_update_p95": ratios["scroll_pooled.update_ms.p95"] <= 1.5,
            "met_interval_p50": ratios["scroll_pooled.interval_ms.p50"] <= 1.5,
        }
    target = 1000.0 / 120.0
    for blocks, data in summary["sizes"].items():
        refresh = max((m.get("refresh_rate_millihertz") or 0 for m in data["monitors"]), default=0)
        s = data["scroll_pooled"]["interval_ms"]
        p50_ok = s["p50"] <= target * 1.05
        p99_ok = s["p99"] <= target * 1.5
        data["scroll_120fps"] = {
            "display_refresh_hz": refresh / 1000.0,
            "interval_p50_ms": s["p50"],
            "interval_p99_ms": s["p99"],
            "interval_max_ms": s["max"],
            "frames_over_16.7ms": f"{data['scroll_frames_over_16.7ms']} of {data['scroll_frames']}",
            "p50_within_5pct_of_8.33ms": p50_ok,
            "p99_within_1.5_frames": p99_ok,
            "verdict": "met" if refresh >= 120000 and p50_ok and p99_ok else
                       ("unproven: display below 120 Hz" if refresh < 120000 else "not met"),
        }
    with open(os.path.join(out, "summary.json"), "w") as f:
        json.dump(summary, f, indent=2)

    lines = ["# Native gate summary", ""]
    for blocks, data in summary["sizes"].items():
        lines.append(f"## {blocks} blocks ({data['rounds']} rounds)")
        lines.append(f"monitors: {data['monitors']}")
        lines.append(f"load: {data['load']}")
        lines.append("")
        lines.append("| phase | metric | n | p50 | p95 | p99 | max |")
        lines.append("|---|---|---|---|---|---|---|")
        for phase, metrics in data["phases"].items():
            for metric in ("interval_ms", "update_ms", "cpu_ms", "render_interval_ms"):
                s = metrics[metric]
                lines.append(
                    f"| {phase} | {metric} | {s['n']} | {s['p50']:.2f} | {s['p95']:.2f} | {s['p99']:.2f} | {s['max']:.2f} |"
                )
        v = data["scroll_120fps"]
        lines.append("")
        lines.append(f"scroll 120 fps: **{v['verdict']}** (p50 {v['interval_p50_ms']:.2f} ms, p99 {v['interval_p99_ms']:.2f} ms, max {v['interval_max_ms']:.2f} ms, {v['frames_over_16.7ms']} frames over 16.7 ms, display {v['display_refresh_hz']:.0f} Hz)")
        lines.append("")
    c = summary["criteria"].get("ratio_largest_over_smallest")
    if c:
        lines.append(f"## Ratio {c['largest']} / {c['smallest']} blocks (limit 1.5)")
        for k, v in c["ratios"].items():
            lines.append(f"- {k}: {v:.2f}")
    text = "\n".join(lines) + "\n"
    with open(os.path.join(out, "summary.md"), "w") as f:
        f.write(text)
    print(text)


if __name__ == "__main__":
    main(sys.argv[1])
