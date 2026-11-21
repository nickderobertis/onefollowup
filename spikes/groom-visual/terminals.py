"""Throwaway mock: the terminal views of grooming, as ANSI text for freeze.

Writes <out>/just-groom.ansi, governor.ansi and quota.ansi. Fixture data only.
"""

import pathlib
import sys

B, D, G, Y, C, R = "\x1b[1m", "\x1b[2m", "\x1b[32m", "\x1b[33m", "\x1b[36m", "\x1b[0m"
P = f"{G}${R}"
BATCH = "groom-20261121T093012Z"


def step(t, name, status, text):
    return f"{D}{t}{R}  {C}{name:<8}{R}  {status:<{8 + len(status) - len(strip(status))}}  {text}"


def strip(s):
    for code in (B, D, G, Y, C, R):
        s = s.replace(code, "")
    return s


done = f"{G}done{R}"
shard_times = ["09:44:05", "09:44:52", "09:46:17", "09:57:30", "09:58:02", "10:00:41",
               "10:11:09", "10:12:36", "10:13:58", "10:24:20", "10:25:47", "10:27:03"]

just_groom = [
    f"{P} just groom",
    f"{D}onepipeline: engine requested 0.121.0 (config/onepipeline.version), about to run 0.121.0{R}",
    f"flow {B}follow-up-groom-20261121t093012z{R} {D}·{R} prepare → verify → check → publish → settle",
    "",
    step("09:30:12Z", "prepare", "started", "onefollowup groom prepare"),
    step("09:31:40Z", "prepare", done, f"562 open · {B}120 selected{R} · 439 unchanged · 3 protected · 12 shards"),
    step("09:31:41Z", "verify", "started", "12 verifying dispatches, 3 at a time"),
]
for k, t in enumerate(shard_times, 1):
    just_groom.append(step(f"{t}Z", "verify", f"{G}✓{R}", f"{BATCH}-{k:<3} 10 tickets · 10 verdicts"))
just_groom += [
    step("10:27:03Z", "verify", done, f"{B}12 of 12 shards verified{R} · 120 verdicts"),
    step("10:27:05Z", "check", done, "0 problems · 9 fixed · 3 obsolete · 6 duplicates into 5 survivors · 102 kept (4 unresolved)"),
    step("10:27:06Z", "publish", "started", "23 comments · 18 closes · 120 groom records"),
    f"{Y}onefollowup: governor: content writes at 40 this minute (cap 40); next write in 9s{R}",
    step("10:41:52Z", "publish", done, "23 of 23 comments · 18 of 18 closes · 120 of 120 records receipted"),
    step("10:47:18Z", "settle", done, "23 of 23 changed tickets confirmed · 0 mismatched"),
    "",
    f"flow follow-up-groom-20261121t093012z {G}{B}succeeded{R} in 1h17m06s {D}· GraphQL 3,912 points · core 286{R}",
    f"{B}Report:{R} .onefollowup/groom/{BATCH}/report.md {D}(report.json beside it){R}",
]

governor = [
    f"{P} just groom --publish",
    f"{D}...{R}",
    f"{Y}onefollowup: governor: pausing graphql at 10:37:30Z: remaining 1,412 of 5,000 is under floor 1,000 (20%) + reserve 1,953{R}",
    f"{Y}  (others burning 38/min, peak 62/min in the last 15m, × 21m to reset × 1.5); resets 10:58:30Z; resuming by then, sooner on a fresh reading{R}",
    f"{D}onefollowup: governor: graphql read at 10:44:00Z: remaining 1,163; under floor 1,000 + reserve 1,349; waiting{R}",
    f"{D}onefollowup: governor: graphql read at 10:51:30Z: remaining 1,760; above floor 1,000 + reserve 651; resuming after 14m00s paused{R}",
    f"{D}...{R}",
    "",
    f"{P} onefollowup groom prepare",
    f"{D}onefollowup: governor: not pausing graphql: remaining 4,467 of 5,000 is above floor 1,000 (20%) + reserve 1,120{R}",
    f"{D}  (others' peak 31/min × 24m to reset × 1.5); this call's estimate 40{R}",
]

quota = [
    f"{P} onetaskgraph sources quota followups",
    f"{B}RESOURCE              LIMIT  REMAINING   USED  RESETS (UTC){R}",
    "core                  5,000      3,835  1,165  2026-11-21 10:58:30  (in 31m)",
    "graphql               5,000      4,467    533  2026-11-21 10:52:04  (in 25m)",
    "search                   30         30      0  2026-11-21 10:28:01  (in 1m)",
    "code_search              10         10      0  2026-11-21 10:28:01  (in 1m)",
    "integration_manifest  5,000      5,000      0  2026-11-21 11:27:00  (in 60m)",
]

out = pathlib.Path(sys.argv[1])
for name, lines in (("just-groom", just_groom), ("governor", governor), ("quota", quota)):
    (out / f"{name}.ansi").write_text("\n".join(lines) + "\n")
