#!/usr/bin/env python3
"""Spike: turn the 2026-11-20 grooming pass's own quota readings into the trace shape.

The pass (`scratch/groom/apply.py` in ai-orchestrator) printed `quota graphql=G core=C`
before every tenth write and logged each write's completion time to `applied.jsonl`. The
logs carry no timestamp, limit or reset of their own, so this script infers them:

- `t`: a reading taken before write 10k of a run is stamped at write 10k-1's completion
  plus the pass's 3 s sleep; a run's first reading at its first write's completion less the
  median write duration plus 3 s. Marked in `inferred`.
- `limit`: 5000, the limit every same-day reading of this pool reports.
- `reset`: the midpoint of the readings a reset fell between (remaining jumps up), and one
  hour after it; the window before the first reset is that reset. Marked in `inferred`.
- `own_writes`: the writes this pass made since the previous reading (its own spend, in
  writes; the points per write are not logged, so `own` is null).

A reading that cannot belong to one pool (a resource would have to reset twice within half
an hour) is kept with `excluded` naming why.

Usage: python3 convert_apply_logs.py GROOM_DIR > trace.jsonl
"""

from __future__ import annotations

import json
import statistics
import sys
from pathlib import Path

LIMIT = 5000
WINDOW = 3600
RESOURCES = ("graphql", "core")


def parse_log(path: Path) -> tuple[list[tuple[int, dict[str, int]]], int]:
    readings: list[tuple[int, dict[str, int]]] = []
    writes = 0
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("quota "):
            fields = dict(part.split("=", 1) for part in line.split()[1:])
            readings.append((writes, {r: int(fields[r]) for r in RESOURCES}))
        elif line.startswith("ok "):
            writes += 1
        elif line.startswith("ERR"):
            sys.exit(f"error: {path} records a failed write; this converter assumes none")
    return readings, writes


def main() -> int:
    if len(sys.argv) != 2:
        sys.exit("usage: convert_apply_logs.py GROOM_DIR")
    groom = Path(sys.argv[1])
    applied = [json.loads(l) for l in (groom / "applied.jsonl").read_text().splitlines()]
    runs = [parse_log(groom / f"apply-{i}.log") for i in (1, 2)]
    total = sum(n for _, n in runs)
    # applied.jsonl's leading entries belong to an earlier limited run with no log.
    applied = applied[len(applied) - total :]
    gaps = sorted(b["at"] - a["at"] for a, b in zip(applied, applied[1:]))
    median_gap = statistics.median(gaps)

    rows = []
    offset = 0
    for readings, writes in runs:
        prev_w = 0
        for w, rem in readings:
            i = offset + w
            t = applied[i - 1]["at"] + 3 if w > 0 else applied[i]["at"] - median_gap + 3
            rows.append({"t": round(t, 1), "remaining": rem, "own_writes": w - prev_w})
            prev_w = w
        offset += writes
        # the next run's first reading follows this run's last writes
        rows[-1]["tail_writes"] = writes - prev_w
    for a, b in zip(rows, rows[1:]):
        if b["own_writes"] == 0 and "tail_writes" in a:
            b["own_writes"] = a.pop("tail_writes")
    rows[-1].pop("tail_writes", None)

    # Remaining only rises at a reset, at most once an hour per resource. A reading that
    # would need a second reset within half a window of the last one is another pool's.
    excluded: dict[int, str] = {}
    resets: dict[str, list[float]] = {r: [] for r in RESOURCES}
    last = 0
    for k in range(1, len(rows)):
        rises = {r: rows[k]["remaining"][r] > rows[last]["remaining"][r] for r in RESOURCES}
        if any(up and resets[r] and rows[k]["t"] - resets[r][-1] < WINDOW / 2
               for r, up in rises.items()):
            excluded[k] = (
                "a resource rises within half an hour of its last reset, which one pool's "
                "hourly window never allows: this reading came from another gh account"
            )
            continue
        for r, up in rises.items():
            if up:
                resets[r].append(round((rows[last]["t"] + rows[k]["t"]) / 2, 1))
        last = k

    out = sys.stdout
    for k, row in enumerate(rows):
        rec = {"t": row["t"], "source": "apply-log", "own": {r: None for r in RESOURCES},
               "own_writes": row["own_writes"], "inferred": ["t", "limit", "used", "reset"]}
        for r in RESOURCES:
            later = [x for x in resets[r] if x > row["t"]]
            reset = later[0] if later else (resets[r][-1] + WINDOW if resets[r] else None)
            rem = row["remaining"][r]
            rec[r] = {"limit": LIMIT, "remaining": rem, "used": LIMIT - rem,
                      "reset": None if reset is None else int(reset)}
        if k in excluded:
            rec["excluded"] = excluded[k]
        out.write(json.dumps(rec, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
