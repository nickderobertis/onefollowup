#!/usr/bin/env python3
"""Spike sampler: read GitHub's GET /rate_limit on a fixed interval for a bounded time.

GET /rate_limit costs no primary quota (GitHub REST docs, "Get rate limit status for the
authenticated user"). This script makes no other request. Each reading is one JSON line:

  {"t": <unix seconds>, "source": "sample", "own": {"graphql": 0, "core": 0},
   "graphql": {"limit", "remaining", "used", "reset"}, "core": {...}}

`own` is this host's own known spend since the previous reading on the resource; the
sampler spends nothing, so it is always 0 here (the trace shape is shared with traces
converted from the follow-up loop's own logs, where it is not).

Usage: python3 sample.py --env-file PATH --out trace.jsonl [--interval 30] [--duration 3600]
Throwaway spike code on the spike's branch, never landed: a committed suite would guard
nothing, so it is proved by the one-off runs its spike report quotes (a hand-built trace
with known answers, and the rule arithmetic worked by hand).
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request

MAX_DURATION = 3600  # the spike's bound: 60 minutes, and no longer
RESOURCES = ("graphql", "core")


def read_token(env_file: str, name: str) -> str:
    with open(env_file, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line.startswith("export "):
                line = line[len("export ") :]
            key, sep, value = line.partition("=")
            if sep and key.strip() == name:
                return value.strip().strip("'\"")
    sys.exit(f"error: {name} not found in {env_file}; set it there and re-run")


def read_rate_limit(token: str) -> dict:
    req = urllib.request.Request(
        "https://api.github.com/rate_limit",
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "quota-burn-spike",
        },
    )
    with urllib.request.urlopen(req, timeout=20) as resp:
        body = json.load(resp)
    out = {}
    for name in RESOURCES:
        r = body["resources"][name]
        out[name] = {k: int(r[k]) for k in ("limit", "remaining", "used", "reset")}
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--env-file", required=True)
    ap.add_argument("--token-var", default="GH_PROJECTS_TOKEN")
    ap.add_argument("--out", required=True)
    ap.add_argument("--interval", type=float, default=30.0)
    ap.add_argument("--duration", type=float, default=MAX_DURATION)
    args = ap.parse_args()
    if not 0 < args.duration <= MAX_DURATION:
        sys.exit(f"error: --duration must be in (0, {MAX_DURATION}] seconds")
    if args.interval < 5:
        sys.exit("error: --interval must be at least 5 seconds")
    token = read_token(args.env_file, args.token_var)
    start = time.monotonic()
    n = 0
    with open(args.out, "a", encoding="utf-8") as out:
        while True:
            due = start + n * args.interval
            if due - start > args.duration:
                break
            time.sleep(max(0.0, due - time.monotonic()))
            t = time.time()
            try:
                reading = read_rate_limit(token)
            except (urllib.error.URLError, OSError, KeyError, ValueError) as exc:
                print(f"warn: reading {n} failed at {t:.0f}: {exc}", file=sys.stderr)
                n += 1
                continue
            rec = {"t": round(t, 1), "source": "sample", "own": {r: 0 for r in RESOURCES}}
            rec.update(reading)
            out.write(json.dumps(rec, sort_keys=True) + "\n")
            out.flush()
            n += 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
