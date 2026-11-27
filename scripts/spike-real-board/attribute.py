"""Offline figures from the recordings, no GitHub request: `python3 attribute.py RUN`.

Writes `recordings/attribution.json` and prints it:

- **spend**: the ledger split into charges GitHub reported and dry runs, whose own charge no
  answer reports and which the ledger counts at an assumed 1 point each;
- **per_draft**: each draft's share of RUN's GitHub-reported cost and of the shared fall, by
  the allocation rule below;
- **projections**: today's design and candidates 1, 3 and 1+3 at 50 drafts, under the stated
  query-overlap and distinct-hit assumptions, against both budgets;
- **run_13331**: what the measured prices bound about the live run's 13,331 counted points.

**Allocation of a request shared by drafts.** A `SEARCH_BATCH` round's cost and fall are split
over its asked slots in proportion to the rows each slot asked (`first<n>`): a slot's nested
connections, which carry the price, scale with its rows. A slot's share goes to the drafts whose
queries include its text, equally when several do. A `SEARCH_ISSUES` request goes to the drafts
asking its text. An `ISSUE_DETAILS` request is split equally over its distinct item ids, and an
item's share equally over the drafts whose queries' answers held it.
"""

from __future__ import annotations

import json
import math
import sys
from collections import defaultdict
from pathlib import Path

import analyze
import ghq

DRAFTS_50 = 50
BOARD_ISSUES = 2138
DETAIL = 24
SLOTS = 20
POINTS_BUDGET = 10  # prepare-graphql-points-per-draft, points a draft
SECONDS_BUDGET = 300  # prepare-seconds-per-batch, seconds a 50-draft batch
# Measured on the real board (candidate-latency.json, summary.json).
SCALAR_ROUND_SECONDS = 6.921
# Measured for candidate 2's detail shape only: Status by name, which omits the other board
# field values, stateReason, the relationship-event count and some comment fields.
BY_NAME_DETAIL_SECONDS = 1.719
# Dry-run price of 20 search slots of 21 rows each carrying Status by name.
STATUS_BY_NAME_ROUND_POINTS = 4


def spend() -> dict:
    reported = assumed = 0.0
    reported_requests = dry_runs = 0
    for line in ghq.LEDGER.read_text().splitlines():
        entry = json.loads(line)
        if entry.get("cost_reported"):
            reported += float(entry["charged"])
            reported_requests += 1
        else:
            assumed += float(entry["charged"])
            dry_runs += 1
    return {
        "github_reported_points": reported,
        "github_reported_requests": reported_requests,
        "dry_runs": dry_runs,
        "dry_run_points_assumed_at_1_each": assumed,
        "ledger_total_counted": reported + assumed,
    }


def slot_text(variables: dict, slot: int) -> str:
    return variables[f"search{slot}"]


def per_draft(run: Path) -> dict:
    requests = analyze.load_jsonl(run / "requests.jsonl")
    answers = {one["seq"]: one["answer"] for one in analyze.load_jsonl(run / "responses.jsonl.gz")}
    prepared = json.loads((run / "prepare.stdout.json").read_text())
    full = lambda text: f'project:nickderobertis/3 is:issue in:title,body "{text}"'  # noqa: E731
    askers: dict[str, list[str]] = defaultdict(list)
    for search in prepared["duplicate_searches"]:
        for query in search["queries"]:
            askers[full(query["text"])].append(search["input"])
    holders: dict[str, set[str]] = defaultdict(set)  # item id -> drafts whose answers held it
    cost = defaultdict(float)
    fall = defaultdict(float)

    def give(drafts: list[str], share_cost: float, share_fall: float) -> None:
        for draft in drafts:
            cost[draft] += share_cost / len(drafts)
            fall[draft] += share_fall / len(drafts)

    for request in requests:
        name = request.get("document")
        if request.get("refused") or name == "REST /rate_limit":
            continue
        c, f = float(request.get("cost") or 0), float(request.get("fall") or 0)
        variables = request["variables"]
        data = (answers.get(request["seq"]) or {}).get("data") or {}
        if name == "SEARCH_BATCH":
            slots = [s for s in range(SLOTS) if variables.get(f"ask{s}") is True]
            rows = {s: variables[f"first{s}"] for s in slots}
            total = sum(rows.values())
            for s in slots:
                drafts = askers[slot_text(variables, s)]
                give(drafts, c * rows[s] / total, f * rows[s] / total)
                for node in (data.get(f"s{s}") or {}).get("nodes") or []:
                    if node and node.get("id"):
                        holders[node["id"]].update(drafts)
        elif name == "SEARCH_ISSUES":
            drafts = askers[variables["search"]]
            give(drafts, c, f)
            for node in (data.get("search") or {}).get("nodes") or []:
                if node and node.get("id"):
                    holders[node["id"]].update(drafts)
    for request in requests:
        if request.get("document") != "ISSUE_DETAILS" or request.get("refused"):
            continue
        c, f = float(request.get("cost") or 0), float(request.get("fall") or 0)
        ids = list(dict.fromkeys(v for k, v in request["variables"].items() if k.startswith("id") and v))
        for item in ids:
            give(sorted(holders[item]) or ["(unattributed)"], c / len(ids), f / len(ids))
    drafts = sorted(set(cost) | set(fall))
    rows = [
        {
            "draft": draft,
            "own_points": round(cost[draft], 2),
            "shared_fall": round(fall[draft], 2),
            "own_share": round(cost[draft] / fall[draft], 3) if fall[draft] else None,
        }
        for draft in drafts
    ]
    return {
        "run": run.name,
        "drafts": rows,
        "total_own_points": round(sum(cost.values()), 2),
        "total_shared_fall": round(sum(fall.values()), 2),
        "items_held_by_several_drafts": sum(len(held) > 1 for held in holders.values()),
        "items": len(holders),
    }


def projections(run: Path) -> dict:
    summary = json.loads((run / "summary.json").read_text())
    by = summary["by_document"]
    drafts = summary["run"]["drafts"]
    scale = DRAFTS_50 / drafts
    searches = sum(summary["queries_per_draft"])  # searches asked, repeats included
    distinct_texts = summary["searches"]
    search_points = by["SEARCH_BATCH"]["cost"] + by.get("SEARCH_ISSUES", {}).get("cost", 0)
    search_seconds = by["SEARCH_BATCH"]["seconds"] + by.get("SEARCH_ISSUES", {}).get("seconds", 0)
    detail_seconds = by["ISSUE_DETAILS"]["seconds"] / by["ISSUE_DETAILS"]["requests"]
    distinct_hits = json.loads((run.parent / "ten-drafts-ungoverned" / "candidate-prices.json").read_text())["distinct_hits_read"]
    hits_per_draft = distinct_hits / drafts
    searches_50 = searches * scale
    # Distinct hits at 50 drafts: the measured 64.5 a draft with no overlap between drafts, at
    # most the whole board; and the expected count were each draft's hits independent draws.
    hits_cases = {
        "upper: no cross-draft overlap, capped at the board": min(hits_per_draft * DRAFTS_50, BOARD_ISSUES),
        # Each draft's hits as independent draws from the board: the expected distinct count.
        # At 10 drafts this model gives 564 against the 645 measured, so it errs low.
        "random draws: each draft's hits drawn independently from the board": BOARD_ISSUES
        * (1 - (1 - hits_per_draft / BOARD_ISSUES) ** DRAFTS_50),
    }
    measured_overlap = 1 - distinct_texts / searches
    overlaps = {
        "measured 10-draft repeat rate": measured_overlap,
        "assumed 15% of 50-draft texts repeated": 0.15,
        "assumed 30% of 50-draft texts repeated": 0.30,
    }

    def verdict(points: float, seconds: float) -> dict:
        return {
            "points": round(points),
            "points_per_draft": round(points / DRAFTS_50, 2),
            "within_points_budget": points / DRAFTS_50 <= POINTS_BUDGET,
            "seconds": round(seconds),
            "within_seconds_budget": seconds <= SECONDS_BUDGET,
        }

    out: dict = {
        "assumptions": {
            "searches_per_draft": searches / drafts,
            "searches_at_50": searches_50,
            "measured_repeat_rate": round(measured_overlap, 4),
            "today_points_per_search": search_points / searches,
            "today_seconds_per_search": search_seconds / searches,
            "today_detail_seconds_per_batch": detail_seconds,
            "candidate_1_round_seconds": SCALAR_ROUND_SECONDS,
            "candidate_1_detail_seconds_per_batch": "today's recorded ISSUE_DETAILS latency (today_detail_seconds_per_batch): it keeps every field",
            "candidate_2_round_points": STATUS_BY_NAME_ROUND_POINTS,
            "candidate_2_round_seconds": "the scalar round's measured 6.921 s, assuming GitHub's time grows with the searches in a request, not the bytes",
            "candidate_2_detail_seconds_per_batch": BY_NAME_DETAIL_SECONDS,
            "distinct_hits_per_draft": hits_per_draft,
            "distinct_hits_at_50": hits_cases,
            "note": "Today's search cost scales with the searches asked, each walking its own pages; dedup removes a repeated text's pages and leaves the distinct hits, and so the detail reads, unchanged.",
        },
        "cases": {},
    }
    for hits_name, hits in hits_cases.items():
        batches = math.ceil(hits / DETAIL)
        for overlap_name, overlap in {"none": 0.0, **overlaps}.items():
            asked = searches_50 * (1 - overlap)
            today_search_points = search_points / searches * asked
            today_search_seconds = search_seconds / searches * asked
            rounds = math.ceil(asked / SLOTS)
            key = f"{hits_name} | overlap {overlap_name} ({overlap:.1%})"
            cases = {}
            # Candidates 1 and 1 + 3 keep every field today's detail read returns, so their
            # detail batches are timed at today's recorded ISSUE_DETAILS latency. Only the
            # reduced-answer shapes (candidate 2, and candidate 1 with candidate 2's detail
            # read) use the by-name batch's latency, measured on a shape omitting fields.
            full_detail = batches * detail_seconds
            by_name_detail = batches * BY_NAME_DETAIL_SECONDS
            if overlap == 0.0:
                cases["today"] = verdict(today_search_points + batches, today_search_seconds + full_detail)
                cases["candidate 1"] = verdict(rounds + batches, rounds * SCALAR_ROUND_SECONDS + full_detail)
                cases["candidate 2 (reduced answer: Status by name in search slots and detail batches)"] = verdict(
                    rounds * STATUS_BY_NAME_ROUND_POINTS + batches,
                    rounds * SCALAR_ROUND_SECONDS + by_name_detail,
                )
                cases["candidate 1 search with candidate 2's detail read (reduced answer)"] = verdict(
                    rounds + batches, rounds * SCALAR_ROUND_SECONDS + by_name_detail
                )
            else:
                cases["candidate 3 (today's slots, one search per distinct text)"] = verdict(
                    today_search_points + batches, today_search_seconds + full_detail
                )
                cases["candidate 1 + 3"] = verdict(
                    rounds + batches, rounds * SCALAR_ROUND_SECONDS + full_detail
                )
            out["cases"][key] = {"detail_batches": batches, "searches_asked": round(asked), **cases}
    return out


def run_13331() -> dict:
    """What the measured prices bound about the live run whose progress counted 13,331 points
    after 380 requests (its commands/prepare.log), which recorded no response's own cost."""
    governed = json.loads((ghq.RECORDINGS / "ten-drafts-governed" / "summary.json").read_text())
    graphql = governed["requests"]
    counted = sum(phase["requests"] for phase in governed["prepare_progress_finished"])
    reads = counted - graphql
    reported = governed["reported_points"]
    largest = max(max(c for c in held["costs"] if c is not None) for held in governed["by_document"].values())
    counted_requests = 380
    measured_mix = reported / counted  # reported points per request the progress lines count
    return {
        "counted_points": 13331,
        "counted_requests": counted_requests,
        "measured_counted_requests_in_governed_run": counted,
        "of_which_rate_limit_reads": reads,
        "measured_points_per_counted_request": round(measured_mix, 3),
        "largest_price_of_a_recorded_document": largest,
        "own_at_measured_mix": round(measured_mix * counted_requests),
        "own_upper_bound_every_request_at_largest_price": counted_requests * largest,
        "own_share_at_measured_mix": round(measured_mix * counted_requests / 13331, 4),
        "own_share_upper_bound": round(counted_requests * largest / 13331, 4),
    }


def main() -> None:
    run = Path(sys.argv[1]) if len(sys.argv) > 1 else ghq.RECORDINGS / "ten-drafts-governed"
    result = {
        "spend": spend(),
        "per_draft": per_draft(run),
        "projections": projections(run),
        "run_13331": run_13331(),
    }
    (ghq.RECORDINGS / "attribution.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
