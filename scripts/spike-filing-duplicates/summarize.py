#!/usr/bin/env python3
"""Turn `replay.py --all --json` measurements into the spike report's counts.

  python3 scripts/spike-filing-duplicates/summarize.py MEASUREMENTS.json

Prints, as JSON: each later member's classified cause, the cause counts, and each
strategy's recall (clusters caught, later members caught) with its searches, store
operations and hits per draft, and onetaskgraph's modelled GraphQL points per draft. Also
evaluates `duplicates: {same: [...]}` offline from the ticket records, since an exact field
match needs no search to decide.

Spike tooling, not landed; it runs once over one measurement file (see replay.py).
"""

from __future__ import annotations

import json
import math
import re
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import replay  # noqa: E402  (a sibling script, not an installed module)

# onetaskgraph-github-projects 0.14.20 lib.rs:750-790: one request asks up to 20 searches at
# 3 rows each for 3 points; a search with more rows is walked on 20 shared rows a page.
SEARCH_BATCH = 20
FIRST_ROUND_POINTS = 3
FIRST_PAGE_ROWS = 3
RESUMED_ROWS = 20


def modelled_points(searches: list[dict]) -> int:
    if not searches:
        return 0
    rounds = math.ceil(len(searches) / SEARCH_BATCH)
    beyond = sum(max(0, (s["hits"] or FIRST_PAGE_ROWS) - FIRST_PAGE_ROWS) for s in searches)
    return rounds * FIRST_ROUND_POINTS + math.ceil(beyond / RESUMED_ROWS)


def locations(body: str) -> set[str]:
    fix = replay.section(body, "Suggested fix")
    return set(re.findall(r"`([\w./-]+/[\w.-]+)`", fix))


def main() -> None:
    if len(sys.argv) != 2 or not Path(sys.argv[1]).is_file():
        print("summarize: pass the JSON replay.py --all --json wrote", file=sys.stderr)
        raise SystemExit(2)
    clusters = json.loads(Path(sys.argv[1]).read_text())
    members, _, _ = replay.load_members(replay.GROOM)
    rows = [(c, r) for c in clusters for r in c["later"]]
    causes = {}
    for cluster, row in rows:
        row["cause"] = replay.classify(row)
        causes[row["later"]] = row["cause"]

    def caught(row: dict, strategies: set[str]) -> bool:
        return any(s["found"] for s in row["searches"] if s["strategy"] in strategies)

    def slug_same(row: dict) -> bool:
        return any(e["slug"] and e["slug"] == row["slug"] for e in row["earlier"])

    def location_same(row: dict) -> bool:
        mine = locations(members[row["later"]].body)
        return any(mine and mine == locations(members[e["key"]].body) for e in row["earlier"])

    def location_shared(row: dict) -> bool:
        mine = locations(members[row["later"]].body)
        return any(mine & locations(members[e["key"]].body) for e in row["earlier"])

    strategies = {
        "prepare: draft title, as today": {"prepare"},
        "agent: recorded transcript queries": {"agent"},
        "template: invariant, root-cause sentence, file": {"template"},
        "filing today: prepare + agent + template": {"prepare", "agent", "template"},
        "terms: rarest words, one per query": {"terms"},
    }
    report = {"later_members": len(rows), "clusters": len(clusters)}
    report["causes"] = dict(Counter(causes.values()).most_common())
    report["causes_merged_19"] = dict(
        Counter(causes[r["later"]] for c, r in rows if c["merged"]).most_common()
    )
    findable = [(c, r) for c, r in rows if not r["timing"]]
    report["findable_later_members"] = len(findable)
    result = {}
    for name, kinds in strategies.items():
        asked = [(c, r) for c, r in rows if any(s["strategy"] in kinds for s in r["searches"])]
        hit_rows = [(c, r) for c, r in asked if caught(r, kinds)]
        searches = [s for _, r in asked for s in r["searches"] if s["strategy"] in kinds]
        result[name] = {
            "later_members_asked": len(asked),
            "later_members_caught": len(hit_rows),
            "clusters_caught": len({c["survivor"] for c, _ in hit_rows}),
            "caught": sorted(r["later"] for _, r in hit_rows),
            "searches_per_draft": round(len(searches) / max(1, len(asked)), 1),
            "store_operations_per_draft": 1,
            "hits_per_draft": round(sum(s["hits"] or 0 for s in searches) / max(1, len(asked)), 1),
            "modelled_points_per_draft": round(
                sum(modelled_points([s for s in r["searches"] if s["strategy"] in kinds]) for _, r in asked)
                / max(1, len(asked)),
                1,
            ),
        }
    terms = result["terms: rarest words, one per query"]
    result["any-term: the same words OR-ed in one query"] = {
        **terms,
        "searches_per_draft": 1,
        "note": "recall is the union of the one-per-query terms (GitHub ORs them, checked live); "
        "the store cannot send it: its text is one quoted phrase",
    }
    for name, test in (
        ("same: [root_cause slug]", slug_same),
        ("same: [contributing locations], all equal", location_same),
        ("contributing location shared (not `same` semantics)", location_shared),
    ):
        hit_rows = [(c, r) for c, r in rows if test(r)]
        result[name] = {
            "later_members_caught": len(hit_rows),
            "clusters_caught": len({c["survivor"] for c, _ in hit_rows}),
            "caught": sorted(r["later"] for _, r in hit_rows),
            "store_operations_per_draft": 1,
            "searches_per_draft": 2,
            "note": "decided offline from the ticket records; prepare asks a metadata and a "
            "content search per field value, in its one find",
        }
    prepare = result["prepare: draft title, as today"]
    result["prepare's candidates rendered into the task"] = {
        **{k: prepare[k] for k in ("later_members_caught", "clusters_caught", "caught")},
        "store_operations_per_draft": 0,
        "searches_per_draft": 0,
        "note": "no further search: prepare already asked; recall is what prepare found",
    }
    report["strategies"] = result
    report["per_member"] = [
        {
            "survivor": c["survivor"],
            "merged": c["merged"],
            "later": r["later"],
            "era": r["era"],
            "earlier": [e["key"] for e in r["earlier"]],
            "cause": r["cause"],
            "found_by": sorted({s["strategy"] for s in r["searches"] if s["found"]}),
            "account_points_moved": r["account_points_moved"],
            "modelled_points": modelled_points(r["searches"]),
        }
        for c, r in rows
    ]
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
