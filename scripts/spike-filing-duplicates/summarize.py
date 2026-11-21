#!/usr/bin/env python3
"""Turn `replay.py --all --json` measurements into the spike report's counts.

  python3 scripts/spike-filing-duplicates/summarize.py MEASUREMENTS.json

Prints, as JSON: each later member's classified cause, the cause counts, and each
strategy's recall under three measurements: `current_snapshot` (any earlier member the
replay of today's board returned), `filing_time` (only earlier members that were on the
board, from another batch, and created more than the index-lag window before the later
member's search) and `filing_time_with_index_lag_uncertain` (also admitting those inside the
window). Then each strategy's searches, store operations, hits and onetaskgraph's modelled
GraphQL points per draft. Also
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


def availability(row: dict, earlier: dict, members: dict) -> str:
    """Whether `earlier` was there for `row`'s filing to find, at the time it searched.

    `unavailable`: not on the destination board, filed by the same batch, or created after
    the later member's search (`verified_at`, else its creation). `uncertain`: created
    within `replay.INDEX_LAG` before that search, so possibly not yet in GitHub's index.
    """
    if not members[earlier["key"]].record or earlier["batch"] == row["batch"]:
        return "unavailable"
    searched = replay.parse_time(row["verified_at"] or row["created"])
    created = replay.parse_time(earlier["created"])
    if created > searched:
        return "unavailable"
    if created > searched - replay.INDEX_LAG:
        return "uncertain"
    return "available"


def recall(rows: list, hit, counted: dict) -> dict:
    """Recall of `hit(row, earlier_keys)` over the earlier members `counted` admits per row.

    A later member is findable when at least one earlier member is admitted; a cluster is
    caught only when every findable later member in it is caught, and is left out of the
    denominator when none is findable.
    """
    findable = [(c, r) for c, r in rows if counted[r["later"]]]
    caught = {r["later"] for c, r in findable if hit(r, counted[r["later"]])}

    def clusters(merged_only: bool) -> tuple[int, int]:
        pool = {}
        for c, r in findable:
            if c["merged"] or not merged_only:
                pool.setdefault(c["survivor"], []).append(r["later"])
        return sum(all(k in caught for k in ks) for ks in pool.values()), len(pool)

    merged_rows = [r for c, r in findable if c["merged"]]
    return {
        "clusters_caught_merged": "%d/%d" % clusters(True),
        "later_caught_merged": f"{sum(r['later'] in caught for r in merged_rows)}/{len(merged_rows)}",
        "clusters_caught_all": "%d/%d" % clusters(False),
        "later_caught_all": f"{len(caught)}/{len(findable)}",
        "caught": sorted(caught),
    }


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

    state = {r["later"]: {e["key"]: availability(r, e, members) for e in r["earlier"]} for _, r in rows}
    measurements = {
        "current_snapshot": {r: set(keys) for r, keys in state.items()},
        "filing_time": {r: {k for k, v in keys.items() if v == "available"} for r, keys in state.items()},
        "filing_time_with_index_lag_uncertain": {
            r: {k for k, v in keys.items() if v != "unavailable"} for r, keys in state.items()
        },
    }

    def by_search(kinds: set[str]):
        return lambda row, keys: any(
            set(s["found"]) & keys for s in row["searches"] if s["strategy"] in kinds or s.get("source") in kinds
        )

    def by_locations(equal: bool):
        def test(row, keys):
            mine = locations(members[row["later"]].body)
            theirs = [locations(members[k].body) for k in keys]
            return any(mine and (mine == t if equal else mine & t) for t in theirs)
        return test

    def slug_same(row, keys):
        return any(row["slug"] and members[k].record.get("root_cause") == row["slug"] for k in keys)

    searched = {
        "prepare: draft title, as today": {"prepare", "prepare-recorded"},
        "agent: recorded queries": {"agent", "agent-recorded"},
        "template: invariant, root-cause sentence, file": {"template"},
        "filing today: prepare + agent + template": {"prepare", "prepare-recorded", "agent", "agent-recorded", "template"},
        "terms: rarest words, one per query": {"terms"},
        "terms + the template's file question": {"terms", "failing file"},
    }
    tests = {name: by_search(kinds) for name, kinds in searched.items()}
    tests["any-term: the same words OR-ed in one query"] = tests["terms: rarest words, one per query"]
    tests["same: [root_cause slug]"] = slug_same
    tests["same: [contributing locations], all equal"] = by_locations(True)
    tests["contributing location shared (not `same` semantics)"] = by_locations(False)
    tests["prepare's candidates rendered into the task"] = tests["prepare: draft title, as today"]

    cost = {}
    for name, kinds in searched.items():
        def chosen(s, kinds=kinds):
            return s["strategy"] in kinds or s.get("source") in kinds
        asked = [r for _, r in rows if any(chosen(s) and not s.get("recorded") for s in r["searches"])]
        picked = [[s for s in r["searches"] if chosen(s) and not s.get("recorded")] for r in asked]
        cost[name] = {
            "drafts_asked_live": len(asked),
            "searches_per_draft": round(sum(map(len, picked)) / max(1, len(asked)), 1),
            "hits_per_draft_lower_bound": round(sum(s["hits"] or 0 for p in picked for s in p) / max(1, len(asked)), 1),
            "modelled_points_per_draft": round(sum(map(modelled_points, picked)) / max(1, len(asked)), 1),
            "store_operations_per_draft": 1,
        }
    cost["any-term: the same words OR-ed in one query"] = {**cost["terms: rarest words, one per query"], "searches_per_draft": 1}

    report = {"later_members": len(rows), "clusters": len(clusters)}
    report["causes"] = dict(Counter(causes.values()).most_common())
    report["causes_merged_19"] = dict(Counter(causes[r["later"]] for c, r in rows if c["merged"]).most_common())
    report["earlier_member_availability"] = dict(Counter(v for keys in state.values() for v in keys.values()))
    report["unavailable_or_uncertain"] = {
        r: {k: v for k, v in keys.items() if v != "available"} for r, keys in state.items() if any(v != "available" for v in keys.values())
    }
    report["recall"] = {
        measure: {name: recall(rows, test, counted) for name, test in tests.items()}
        for measure, counted in measurements.items()
    }
    report["cost"] = cost
    report["per_member"] = [
        {
            "survivor": c["survivor"],
            "merged": c["merged"],
            "later": r["later"],
            "earlier": state[r["later"]],
            "cause": r["cause"],
            "found_by": sorted({s["strategy"] for s in r["searches"] if s["found"]}),
            "account_points_moved": r["account_points_moved"],
            "modelled_points": modelled_points([s for s in r["searches"] if not s.get("recorded")]),
        }
        for c, r in rows
    ]
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
