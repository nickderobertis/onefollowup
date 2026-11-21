#!/usr/bin/env python3
"""Spike: others' GitHub quota burn from a trace, and candidate governor rules replayed over it.

A trace is JSON lines, one reading each (see sample.py and convert_apply_logs.py):
  {"t", "source", "own": {res: points|null}, "own_writes"?, "excluded"?,
   "graphql": {"limit", "remaining", "used", "reset"}, "core": {...}}

Others' spend over an interval is the fall in remaining less this host's own known spend:
`own[res]` where recorded, else `own_writes` times a per-write cost (default: the trace's
smallest observed fall per write, an upper bound on the pass's own cost, so others' figures
are lower bounds). Across a reset, the spend after the reset is `limit - remaining`; the
part before it is taken at the previous interval's rate.

Commands:
  burn TRACE                      others' mean, peak over 1/5/15 min, spikes, resets
  replay --trace T --rule R --pass full|weekly [--resource graphql|core] [--offset S]
                                  one rule over one trace; prints the result as JSON
  table TRACE...                  every rule x pass x trace, for the report (markdown)
  stress TRACE...                 the full pass with others' spend x3 and x5 (markdown)

Rules: none | floor:<fraction> | adaptive:<factor> | pace
  floor:F       wait (re-read every step) while remaining - batch < F * limit
  adaptive:K[:W] wait while remaining - batch < 0.2 * limit + K * peak * (reset - now),
                peak = the trace's peak W-minute (default 5) others' burn rate, re-read
                before each batch
  pace          onetaskgraph_live::pace: go at or above 40%; under it go only if
                used * to_reset / elapsed + batch < remaining, else wait until the reset

The replay runs on a 30 s grid. Past the trace's end, others' spend repeats the trace
cyclically and the window resets hourly after its last known reset.
Throwaway spike code on the spike's branch, never landed: a committed suite would guard
nothing, so it is proved by the one-off runs its spike report quotes (a hand-built trace
with known answers, and the rule arithmetic worked by hand).
"""

from __future__ import annotations

import argparse
import bisect
import json
import statistics
import sys
from dataclasses import dataclass
from pathlib import Path

RESOURCES = ("graphql", "core")
WINDOW = 3600
STEP = 30.0
FLOOR = 0.2
PASSES = {"full": 1500.0, "weekly": 150.0}
PASS_RATE = 1500.0 / 3600.0  # points per second: a full pass's ~1,500 points over ~an hour
PACE_PAUSE_BELOW = 0.4


@dataclass
class Interval:
    t0: float
    t1: float
    spend: float  # others' points over [t0, t1)
    reset: float | None  # a window reset inside (t0, t1]


@dataclass
class Series:
    resource: str
    limit: int
    start: float
    end: float
    remaining0: int
    intervals: list[Interval]
    resets: list[float]
    own_per_write: float
    clipped: int  # intervals whose own estimate exceeded the fall (others' spend set to 0)
    readings: int
    dropped: int  # excluded or foreign readings left out


def load(path: Path) -> list[dict]:
    rows = []
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        try:
            rec = json.loads(line)
            for r in RESOURCES:
                for k in ("limit", "remaining"):
                    int(rec[r][k])
            float(rec["t"])
        except (ValueError, KeyError, TypeError) as exc:
            sys.exit(f"error: {path}:{n} is not a trace reading ({exc}); see sample.py for the shape")
        rows.append(rec)
    if len(rows) < 2:
        sys.exit(f"error: {path} holds {len(rows)} reading(s); a trace needs at least two")
    return rows


def foreign(rows: list[dict], k: int) -> bool:
    """A reading whose reset disagrees with both neighbours, which agree, is another pool's.

    GET /rate_limit now and then answers with figures of a different window for the same
    token (seen in both traces), so a reading is trusted only when its window matches.
    """
    if not 0 < k < len(rows) - 1:
        return False
    for r in RESOURCES:
        before, here, after = (rows[i][r].get("reset") for i in (k - 1, k, k + 1))
        if None not in (before, here, after) and before == after != here:
            return True
    return False


def kept(rows: list[dict]) -> list[dict]:
    """Drop excluded and foreign readings, folding their own writes into the next kept one."""
    out, carry = [], 0
    for k, rec in enumerate(rows):
        if "excluded" in rec or foreign(rows, k):
            carry += rec.get("own_writes", 0)
            continue
        rec = dict(rec, own_writes=rec.get("own_writes", 0) + carry)
        carry = 0
        out.append(rec)
    return out


def own_bound(rows: list[dict], res: str) -> float:
    """Smallest fall in remaining per own write, over intervals with no reset."""
    best = None
    for a, b in zip(rows, rows[1:]):
        w = b.get("own_writes", 0)
        fall = a[res]["remaining"] - b[res]["remaining"]
        if w and fall >= 0 and b[res]["reset"] == a[res]["reset"]:
            best = fall / w if best is None else min(best, fall / w)
    return best or 0.0


def series(rows: list[dict], res: str, own_per_write: float | None = None,
           scale: float = 1.0) -> Series:
    """Others' spend per interval; `scale` multiplies it, to stress a rule past the trace."""
    total_rows = len(rows)
    rows = kept(rows)
    if own_per_write is None:
        own_per_write = own_bound(rows, res)
    limit = int(rows[0][res]["limit"])
    intervals: list[Interval] = []
    resets: list[float] = []
    clipped = 0
    for a, b in zip(rows, rows[1:]):
        t0, t1 = float(a["t"]), float(b["t"])
        own = b["own"].get(res)
        own = float(own) if own is not None else b.get("own_writes", 0) * own_per_write
        r0, r1 = a[res]["remaining"], b[res]["remaining"]
        reset_a = a[res].get("reset")
        did_reset = r1 > r0 or (reset_a is not None and b[res].get("reset") not in (None, reset_a)
                                and reset_a <= t1)
        if did_reset:
            tau = float(reset_a) if reset_a is not None and t0 < reset_a <= t1 else (t0 + t1) / 2
            after = (t1 - tau) / (t1 - t0)
            prev_rate = intervals[-1].spend / (intervals[-1].t1 - intervals[-1].t0) if intervals else 0.0
            spend = max(0.0, limit - r1 - own * after) + prev_rate * (tau - t0)
            intervals.append(Interval(t0, t1, spend, tau))
            resets.append(tau)
        else:
            spend = r0 - r1 - own
            if spend < 0:
                clipped += 1
                spend = 0.0
            intervals.append(Interval(t0, t1, spend, None))
    first_reset = rows[0][res].get("reset")
    if first_reset is not None and (not resets or first_reset < resets[0]) and first_reset > rows[0]["t"]:
        resets.insert(0, float(first_reset))
    if not resets and rows[-1][res].get("reset") is not None:
        resets.append(float(rows[-1][res]["reset"]))
    for iv in intervals:
        iv.spend *= scale
    return Series(res, limit, float(rows[0]["t"]), float(rows[-1]["t"]), int(rows[0][res]["remaining"]),
                  intervals, sorted(set(resets)), own_per_write, clipped, total_rows,
                  total_rows - len(rows))


def cumulative(s: Series):
    """Others' cumulative spend C(t), linear inside each interval, cyclic past the end."""
    ts = [s.start] + [iv.t1 for iv in s.intervals]
    cs = [0.0]
    for iv in s.intervals:
        cs.append(cs[-1] + iv.spend)
    span, total = s.end - s.start, cs[-1]

    def c(t: float) -> float:
        k, u = divmod(t - s.start, span)
        t = s.start + u
        i = min(bisect.bisect_right(ts, t) - 1, len(ts) - 2)
        f = (t - ts[i]) / (ts[i + 1] - ts[i])
        return k * total + cs[i] + f * (cs[i + 1] - cs[i])

    return c


def resets_from(s: Series, t: float, horizon: float) -> list[float]:
    out = [x for x in s.resets]
    if out:
        while out[-1] < horizon:
            out.append(out[-1] + WINDOW)
    else:
        out = [s.start + WINDOW * k for k in range(1, int((horizon - s.start) / WINDOW) + 2)]
    return [x for x in out if x > t - WINDOW]


def window_rates(s: Series, minutes: int) -> list[float]:
    """Others' spend per minute over every window of `minutes` inside the trace (30 s grid)."""
    c = cumulative(s)
    w = minutes * 60.0
    if s.end - s.start < w:
        return []
    out, t = [], s.start
    while t + w <= s.end + 1e-6:
        out.append((c(t + w) - c(t)) / minutes)
        t += STEP
    return out


def burn(s: Series) -> dict:
    total_min = (s.end - s.start) / 60
    total = sum(iv.spend for iv in s.intervals)
    mean = total / total_min
    out = {"resource": s.resource, "minutes": round(total_min, 1), "readings": s.readings,
           "dropped": s.dropped, "others_spend": round(total),
           "own_per_write": round(s.own_per_write, 3), "clipped_intervals": s.clipped,
           "mean_per_min": round(mean, 1), "windows": {}}
    for m in (1, 5, 15):
        rates = window_rates(s, m)
        out["windows"][f"{m}m"] = (
            {"mean": round(statistics.mean(rates), 1), "peak": round(max(rates), 1)} if rates
            else None)
    # spikes: an interval spending at >= 3x the trace's mean rate and >= 40 points a minute
    spikes = []
    for iv in s.intervals:
        rate = iv.spend / ((iv.t1 - iv.t0) / 60)
        if rate >= max(3 * mean, 40) and iv.reset is None:
            spikes.append({"t": round(iv.t0), "seconds": round(iv.t1 - iv.t0), "points": round(iv.spend),
                           "per_min": round(rate, 1)})
    out["spikes"] = spikes
    c = cumulative(s)
    resets = []
    for tau in s.resets:
        if s.start < tau < s.end:
            before = (c(tau) - c(max(s.start, tau - 300))) / ((tau - max(s.start, tau - 300)) / 60)
            after_end = min(s.end, tau + 300)
            after = (c(after_end) - c(tau)) / ((after_end - tau) / 60) if after_end > tau else None
            resets.append({"t": round(tau), "per_min_5m_before": round(before, 1),
                           "per_min_5m_after": None if after is None else round(after, 1)})
    out["resets"] = resets
    return out


def peak_rate(s: Series, minutes: int = 5) -> float:
    rates = window_rates(s, minutes) or window_rates(s, 1) or [0.0]
    return max(rates) / 60.0  # points per second


def replay(s: Series, rule: str, points: float, offset: float = 0.0) -> dict:
    c = cumulative(s)
    limit = s.limit
    t = s.start + offset
    horizon = t + 24 * 3600
    resets = resets_from(s, t, horizon)

    def next_reset(now: float) -> float:
        return resets[bisect.bisect_right(resets, now)]

    # others-only remaining at t: replay the trace from its start
    def others_remaining(at: float) -> float:
        prev = [x for x in resets if s.start < x <= at]
        if prev:
            return limit - (c(at) - c(prev[-1]))
        return s.remaining0 - c(at)

    peak = peak_rate(s, int(rule.split(":")[2]) if rule.count(":") == 2 else 5)
    left, own_window, waited, first_wait_left = points, 0.0, 0.0, None
    lowest = alone = others_remaining(t)
    window_end = next_reset(t)
    pass_end = None
    while t < horizon:
        if t >= window_end:
            own_window = 0.0
            window_end = next_reset(t)
        rem = others_remaining(t) - own_window
        if left <= 1e-9:
            # the pass's spend stays off the pool until this window resets
            if pass_end is None:
                pass_end = t
            if t + STEP >= window_end:
                break
        else:
            batch = min(PASS_RATE * STEP, left)
            go, wait_until = decide(rule, rem, batch, limit, t, window_end, peak)
            if go:
                own_window += batch
                left -= batch
            else:
                if first_wait_left is None:
                    first_wait_left = left
                if wait_until is not None and wait_until > t + STEP:
                    # pace waits to the reset; track the lowest the others see meanwhile
                    while t + STEP < wait_until:
                        t += STEP
                        waited += STEP
                        if t >= window_end:
                            own_window = 0.0
                            window_end = next_reset(t)
                        lowest = min(lowest, others_remaining(t) - own_window)
                        alone = min(alone, others_remaining(t))
                    continue
                waited += STEP
        t += STEP
        lowest = min(lowest, others_remaining(t) - own_window)
        alone = min(alone, others_remaining(t))
    no_wait = points - (first_wait_left if first_wait_left is not None else 0.0)
    return {"rule": rule, "resource": s.resource, "points": points, "offset_s": offset,
            "wait_min": round(waited / 60, 1), "lowest_remaining": round(lowest),
            "lowest_fraction": round(lowest / limit, 3), "crossed_20pct": lowest < FLOOR * limit,
            "starved": lowest < 0, "others_alone_lowest": round(alone),
            "pass_crossed_20pct": lowest < FLOOR * limit <= alone, "no_wait_fraction": round(no_wait / points, 3),
            "finished": left <= 1e-9}


def decide(rule: str, rem: float, batch: float, limit: int, now: float, reset: float,
           peak: float) -> tuple[bool, float | None]:
    if rule == "none":
        return True, None
    if rule.startswith("floor:"):
        return rem - batch >= float(rule.split(":", 1)[1]) * limit, None
    if rule.startswith("adaptive:"):
        k = float(rule.split(":")[1])
        reserve = k * peak * max(0.0, reset - now)
        return rem - batch >= FLOOR * limit + reserve, None
    if rule == "pace":
        if rem >= PACE_PAUSE_BELOW * limit or reset <= now:
            return True, None
        elapsed = now - (reset - WINDOW)
        if elapsed > 0:
            used = limit - rem
            projected = used * (reset - now) / elapsed
            if projected + batch < rem:
                return True, None
        return False, reset
    sys.exit(f"error: unknown rule {rule!r}; use none, floor:F, adaptive:K or pace")


RULES = ["none", "floor:0.2", "floor:0.6", "adaptive:1", "adaptive:1.5", "adaptive:2", "pace"]
STRESS_RULES = RULES + ["adaptive:1:15", "adaptive:1.5:15"]


def table(paths: list[Path]) -> str:
    lines = []
    for p in paths:
        rows = load(p)
        for res in RESOURCES:
            s = series(rows, res)
            b = burn(s)
            lines.append(f"### {p.name} — {res}\n")
            lines.append(f"burn: `{json.dumps(b)}`\n")
            span = s.end - s.start
            offsets = [k * 300.0 for k in range(int(span // 300) + 1)]
            lines.append("| rule | pass | wait min (t0) | lowest (t0) | <20% (t0) | no-wait (t0) "
                         "| wait min (worst / mean over offsets) | lowest (worst) | <20% (any) |")
            lines.append("|---|---|---|---|---|---|---|---|---|")
            for rule in RULES:
                for name, pts in PASSES.items():
                    rs = [replay(s, rule, pts, o) for o in offsets]
                    r0 = rs[0]
                    waits = [r["wait_min"] for r in rs]
                    lines.append(
                        f"| {rule} | {name} | {r0['wait_min']} | {r0['lowest_remaining']} "
                        f"| {'yes' if r0['crossed_20pct'] else 'no'} | {r0['no_wait_fraction']:.0%} "
                        f"| {max(waits)} / {statistics.mean(waits):.1f} "
                        f"| {min(r['lowest_remaining'] for r in rs)} "
                        f"| {sum(r['crossed_20pct'] for r in rs)}/{len(rs)} |")
            lines.append("")
    return "\n".join(lines)


def stress(paths: list[Path], scales: tuple[float, ...] = (3.0, 5.0)) -> str:
    """The full pass on GraphQL with others' spend scaled up, worst case over start offsets."""
    lines = ["| trace | others x | rule | wait min (worst / mean) | lowest (worst) | <20% (offsets) "
             "| <20% only with the pass (offsets) | others refused (offsets) |",
             "|---|---|---|---|---|---|---|---|"]
    for p in paths:
        rows = load(p)
        for k in scales:
            s = series(rows, "graphql", scale=k)
            offsets = [o * 300.0 for o in range(int((s.end - s.start) // 300) + 1)]
            for rule in STRESS_RULES:
                rs = [replay(s, rule, PASSES["full"], o) for o in offsets]
                waits = [r["wait_min"] for r in rs]
                lines.append(
                    f"| {p.name} | {k:g} | {rule} | {max(waits)} / {statistics.mean(waits):.1f} "
                    f"| {min(r['lowest_remaining'] for r in rs)} "
                    f"| {sum(r['crossed_20pct'] for r in rs)}/{len(rs)} "
                    f"| {sum(r['pass_crossed_20pct'] for r in rs)}/{len(rs)} "
                    f"| {sum(r['starved'] for r in rs)}/{len(rs)} |")
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description="quota-burn spike: burn rates and governor replay")
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("burn")
    b.add_argument("trace", type=Path)
    r = sub.add_parser("replay")
    r.add_argument("--trace", type=Path, required=True)
    r.add_argument("--rule", required=True)
    r.add_argument("--pass", dest="pass_", choices=sorted(PASSES), required=True)
    r.add_argument("--resource", choices=RESOURCES, default="graphql")
    r.add_argument("--offset", type=float, default=0.0, help="seconds after the trace's start")
    r.add_argument("--scale", type=float, default=1.0, help="multiply others' spend (stress)")
    t = sub.add_parser("table")
    t.add_argument("traces", type=Path, nargs="+")
    st = sub.add_parser("stress")
    st.add_argument("traces", type=Path, nargs="+")
    args = ap.parse_args()
    if args.cmd == "burn":
        rows = load(args.trace)
        print(json.dumps([burn(series(rows, res)) for res in RESOURCES], indent=1))
    elif args.cmd == "replay":
        s = series(load(args.trace), args.resource, scale=args.scale)
        print(json.dumps(replay(s, args.rule, PASSES[args.pass_], args.offset)))
    elif args.cmd == "stress":
        print(stress(args.traces))
    else:
        print(table(args.traces))
    return 0


if __name__ == "__main__":
    sys.exit(main())
