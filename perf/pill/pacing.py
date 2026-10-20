#!/usr/bin/env python3
"""Frame pacing from a capsule pacing log (`SCRAWL_PILL_PACING=<file>`).

    perf/pill/pacing.py log.jsonl [more.jsonl ...] [--time-scale 1] [--json]

For each run of consecutive frames (a morph), every frame should reach the screen exactly one
refresh after the one before and its springs should have advanced by exactly that interval.
Reports, over all runs in a log:

  Hz         the compositor's refresh (from presentation times) / the display link's
  frames     frames drawn while animating
  dropped    compositor refreshes inside a run that showed no new frame (a stall)
  doubled    frames presented on the same refresh as the one before
  replaced   frames drawn but never presented (a newer one replaced it: expected when the
             compositor runs slower than the link, e.g. ProMotion in Low Power Mode)
  skipped    display link ticks inside a run that drew nothing
  worst      longest gap between presented frames (ms)
  jitter     RMS of (spring time advanced - on-screen time advanced) between consecutive
             presented frames (ms): how unevenly the motion moves
  worst-err  largest such per-frame error (ms)
  late       95th percentile of draw start - link tick (ms), the wait for the drawing thread
"""

import json
import math
import sys

REST_GAP_S = 0.15  # a longer pause between frames ends a run


def load(path):
    frames, presented = {}, {}
    for line in open(path):
        try:
            rec = json.loads(line)
        except ValueError:
            continue
        if rec.get("type") == "frame":
            frames[rec["n"]] = rec
        elif rec.get("type") == "presented":
            presented[rec["n"]] = rec["at"]
    return [frames[n] for n in sorted(frames)], presented


def runs(frames):
    out, cur = [], []
    for f in frames:
        if cur and f["start"] - cur[-1]["start"] > REST_GAP_S:
            out.append(cur)
            cur = []
        cur.append(f)
    if cur:
        out.append(cur)
    return [r for r in out if len(r) >= 3]


def pct(values, p):
    if not values:
        return 0.0
    values = sorted(values)
    return values[min(len(values) - 1, int(round(p / 100 * (len(values) - 1))))]


def analyze(path, scale):
    frames, presented = load(path)
    periods = [f["period"] for f in frames if f["period"] > 0]
    link = pct(periods, 50) or 1 / 60
    stats = dict(frames=0, dropped=0, doubled=0, superseded=0, skipped=0, worst=0.0, errs=[], late=[])
    gaps = []
    for run in runs(frames):
        stats["frames"] += len(run)
        shown = []
        for a, b in zip(run, run[1:]):
            stats["skipped"] += max(0, b["tick"] - a["tick"] - 1)
            stats["late"].append((b["start"] - b["linkNow"]) * 1000 if b["linkNow"] else 0.0)
        for f in run:
            at = presented.get(f["n"], 0.0)
            if at > 0:
                shown.append((f, at))
            else:
                # Drawn, then replaced by a newer frame before the compositor took it.
                stats["superseded"] += 1
        for (fa, pa), (fb, pb) in zip(shown, shown[1:]):
            gaps.append((pb - pa, (fb["t"] - fa["t"]) / scale))
    # The compositor's refresh: the usual gap, as a whole number of link periods.
    shortest = pct([g for g, _ in gaps if g > link * 0.5], 50) if gaps else link
    period = max(1, round(shortest / link)) * link
    for gap, step in gaps:
        k = round(gap / period)
        if k == 0:
            stats["doubled"] += 1
        elif k > 1:
            stats["dropped"] += k - 1
        stats["worst"] = max(stats["worst"], gap * 1000)
        stats["errs"].append((step - gap) * 1000)
    errs = stats.pop("errs")
    late = stats.pop("late")
    stats["jitter"] = math.sqrt(sum(e * e for e in errs) / len(errs)) if errs else 0.0
    stats["worst_err"] = max((abs(e) for e in errs), default=0.0)
    stats["late_p95"] = pct(late, 95)
    stats["link_hz"] = round(1 / link)
    stats["hz"] = round(1 / period)
    return stats


def main(argv):
    scale, as_json, paths = 1.0, False, []
    it = iter(argv)
    for arg in it:
        if arg == "--time-scale":
            scale = float(next(it))
        elif arg == "--json":
            as_json = True
        else:
            paths.append(arg)
    results = {p: analyze(p, scale) for p in paths}
    if as_json:
        print(json.dumps(results, indent=2))
        return
    head = f"{'log':<28} {'Hz':>7} {'frames':>6} {'dropped':>7} {'doubled':>7} {'replaced':>8} {'skipped':>7} {'worst ms':>8} {'jitter ms':>9} {'worst-err':>9} {'late p95':>8}"
    print(head)
    for path, s in results.items():
        name = path.rsplit("/", 1)[-1].removesuffix(".jsonl")
        print(
            f"{name:<28} {str(s['hz']) + '/' + str(s['link_hz']):>7} {s['frames']:>6} {s['dropped']:>7} {s['doubled']:>7} {s['superseded']:>8} {s['skipped']:>7} "
            f"{s['worst']:>8.1f} {s['jitter']:>9.2f} {s['worst_err']:>9.2f} {s['late_p95']:>8.2f}"
        )


if __name__ == "__main__":
    main(sys.argv[1:])
