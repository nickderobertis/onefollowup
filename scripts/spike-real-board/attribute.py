"""Offline figures from the recordings, no GitHub request: `python3 attribute.py`.

Writes `recordings/attribution.json` and prints it. Every figure is read from a recording;
nothing is attributed that a recording does not show.

- **spend**: the ledger, GitHub-reported charges apart from dry runs (whose own charge no
  answer reports; counted at an assumed 1 point each).
- **runs**: for each measured run, prepare's own spend as GitHub reported it per request,
  beside the fall of `x-ratelimit-remaining` over the run in every window GitHub answered
  from, the spike's own requests in that interval, and the remainder, labelled unexplained.
  Where the run recorded the REST `core` window its rate-limit reads report, prepare's
  progress count is reconstructed from the readings it was given.
- **per_draft**: each draft's share of prepare's own reported cost, by the rule below.
- **projections**: today's find and the candidates at 50 drafts, from measured and
  dry-run-priced figures, against both budgets.
- **live_run**: what the measured prices bound of the 13,331 points the live run counted.

**Allocation of a request shared by drafts.** A `SEARCH_BATCH` round's reported cost is split
over its asked slots in proportion to the rows each slot asked (`first<n>`), a slot's share
going to the drafts whose queries include its text, equally when several do. A `SEARCH_ISSUES`
request goes to the drafts asking its text. An `ISSUE_DETAILS` request is split equally over
its item ids, and an item's share equally over the drafts whose answers held it; an item
dropped from the recordings as not public is shared by all drafts of the run.
"""

from __future__ import annotations

import json
import math
from collections import defaultdict
from pathlib import Path

import analyze
import ghq

RUNS = ("ten-drafts-ungoverned", "ten-drafts-governed", "ten-drafts-governed-2")
PREPARE = ("SEARCH_BATCH", "SEARCH_ISSUES", "ISSUE_DETAILS", "ISSUE_RELATIONS", "ISSUE_DETAIL", "ISSUE")
DRAFTS_50 = 50
DETAIL = 24
SLOTS = 20
POINTS_BUDGET = 10  # prepare-graphql-points-per-draft, points a draft
SECONDS_BUDGET = 300  # prepare-seconds-per-batch, seconds a 50-draft batch
# The live run's own log (commands/prepare.log of the Linear details-blocks follow-ups run):
# 50 drafts, 365 searches, 13,331 points counted after its find phase's 380th request.
LIVE = {"drafts": 50, "searches": 365, "counted_points": 13331, "counted_requests": 380}


def spend() -> dict:
    reported = assumed = 0.0
    reported_requests = dry_runs = 0
    for line in ghq.LEDGER.read_text().splitlines():
        entry = json.loads(line)
        if entry.get("cost_reported"):
            reported += float(entry["charged"])
            reported_requests += 1 if not entry.get("carried") else 0
        else:
            assumed += float(entry["charged"])
            dry_runs += 1 if not entry.get("carried") else 0
    return {
        "github_reported_points": reported,
        "dry_run_points_assumed_at_1_each": assumed,
        "total_counted_against_the_1500_cap": reported + assumed,
        "this_dispatch_reported_requests": reported_requests,
        "this_dispatch_dry_runs": dry_runs,
        "note": "the first two ledger lines carry the earlier dispatch's totals (243 reported, 25 dry runs); its recordings were discarded",
    }


def requests_of(run: str) -> list[dict]:
    return sorted(analyze.load_jsonl(ghq.RECORDINGS / run / "requests.jsonl"), key=lambda e: e["seq"])


def windows(requests: list[dict]) -> dict:
    """Each GraphQL window's first and last reading over the run, and its fall."""
    held: dict[str, list[int]] = defaultdict(list)
    for entry in requests:
        if entry.get("resource") == "graphql" and entry.get("remaining") is not None:
            held[entry["reset"]].append(entry["remaining"])
    return {
        reset: {"readings": len(values), "first": values[0], "last": values[-1], "fall": values[0] - values[-1]}
        for reset, values in held.items()
    }


def run_figures(run: str) -> dict:
    requests = requests_of(run)
    prepare = [e for e in requests if e["document"] in PREPARE]
    spike = [e for e in requests if e["document"].startswith("forwarder:")]
    own = sum(e.get("cost") or 0 for e in prepare)
    spike_reported = sum(e.get("cost") or 0 for e in spike if not e.get("dry_run"))
    spike_dry = sum(1 for e in spike if e.get("dry_run"))
    by_window = windows(requests)
    fall = sum(w["fall"] for w in by_window.values())
    own_by_window: dict[str, float] = defaultdict(float)
    for e in prepare:
        own_by_window[e["reset"]] += e.get("cost") or 0
    for reset, held in by_window.items():
        held["prepare_own_reported"] = own_by_window.get(reset, 0)
        held["unexplained"] = held["fall"] - held["prepare_own_reported"]
    # Per request: the fall since the previous reading of its window, against its own cost.
    paired = [(e.get("cost") or 0, e.get("fall")) for e in prepare if e.get("fall") is not None]
    meta = json.loads((ghq.RECORDINGS / run / "run.json").read_text())
    summary = json.loads((ghq.RECORDINGS / run / "summary.json").read_text())
    counted = summary["prepare_progress_finished"]
    figures = {
        "governor_enabled": meta["governor_enabled"],
        "seconds": meta["seconds"],
        "prepare_requests": len(prepare),
        "prepare_own_reported_points": own,
        "prepare_own_reported_per_draft": own / meta["drafts"],
        "shared_fall_all_windows": fall,
        "shared_fall_per_draft": fall / meta["drafts"],
        "spike_requests_in_interval_reported_points": spike_reported,
        "spike_dry_runs_in_interval_cost_not_reported": spike_dry,
        "unexplained": fall - own - spike_reported,
        "unexplained_per_draft": (fall - own - spike_reported) / meta["drafts"],
        "windows": by_window,
        "per_request": {
            "requests_with_a_known_fall": len(paired),
            "fall_equals_own_cost": sum(1 for c, f in paired if f == c),
            "fall_exceeds_own_cost": sum(1 for c, f in paired if f > c),
            "fall_below_own_cost": sum(1 for c, f in paired if f < c),
            "largest_fall_beside_a_request": max((f for _, f in paired), default=None),
        },
        "prepare_progress_counted": counted,
        "prepare_progress_counted_points": sum(p["points"] for p in counted),
    }
    rest = [e for e in requests if e["document"] == "REST /rate_limit" and e.get("header_window")]
    if rest:
        figures["progress_reconstruction"] = reconstruct(requests)
    return figures


def reconstruct(requests: list[dict]) -> dict:
    """Prepare's progress count from the readings it was given: a GraphQL response counts its
    window's fall since that window's last reading; a rate-limit read counts the fall of the
    window its own headers report (`core`), and then every window its body reports is seen
    (store.rs `Progress::answered`, `Progress::seen`; quota.rs `rate_limits`). The spike's own
    rate-limit reads, before and after prepare, are readings only."""
    last: dict[tuple[str, str], int] = {}
    phase = "duplicate find on followups"
    out = {
        "duplicate find on followups": {"requests": 0, "points": 0, "core_points": 0},
        "kept-hit detail read": {"requests": 0, "points": 0, "core_points": 0},
    }
    rest = [e for e in requests if e["document"] == "REST /rate_limit"]
    ours = {rest[0]["seq"], rest[-1]["seq"]}
    for e in requests:
        if e["document"] == "ISSUE_DETAILS":
            phase = "kept-hit detail read"
        if e["document"].startswith("forwarder:"):
            last[("graphql", e["reset"])] = e["remaining"]
            continue
        if e["document"] == "REST /rate_limit":
            header = e["header_window"]
            key, remaining = (header["resource"], header["reset"]), int(header["remaining"])
            if e["seq"] not in ours:
                fall = max(0, last[key] - remaining) if key in last else 0
                out[phase]["requests"] += 1
                out[phase]["points"] += fall
                out[phase]["core_points"] += fall
            for name, window in (e.get("windows") or {}).items():
                last[(name, str(window["reset"]))] = window["remaining"]
            last[key] = remaining
            continue
        key = ("graphql", e["reset"])
        if key in last:
            out[phase]["points"] += max(0, last[key] - e["remaining"])
        out[phase]["requests"] += 1
        last[key] = e["remaining"]
    return out


def per_draft(run: str) -> dict:
    directory = ghq.RECORDINGS / run
    requests = requests_of(run)
    answers = {one["seq"]: one["answer"] for one in analyze.load_jsonl(directory / "responses.jsonl.gz")}
    prepared = json.loads((directory / "prepare.stdout.json").read_text())

    def full(text: str) -> str:
        return f'project:nickderobertis/3 is:issue in:title,body "{text}"'

    askers: dict[str, list[str]] = defaultdict(list)
    everyone = [search["input"] for search in prepared["duplicate_searches"]]
    for search in prepared["duplicate_searches"]:
        for query in search["queries"]:
            askers[full(query["text"])].append(search["input"])
    holders: dict[str, set[str]] = defaultdict(set)
    cost: dict[str, float] = defaultdict(float)

    def give(drafts: list[str], share: float) -> None:
        for draft in drafts:
            cost[draft] += share / len(drafts)

    for request in requests:
        name, variables = request["document"], request.get("variables") or {}
        data = (answers.get(request["seq"]) or {}).get("data") or {}
        c = float(request.get("cost") or 0)
        if name == "SEARCH_BATCH":
            slots = [s for s in range(SLOTS) if variables.get(f"ask{s}") is True]
            total = sum(variables[f"first{s}"] for s in slots)
            for s in slots:
                drafts = askers[variables[f"search{s}"]]
                give(drafts, c * variables[f"first{s}"] / total)
                for node in (data.get(f"s{s}") or {}).get("nodes") or []:
                    if node and node.get("id"):
                        holders[node["id"]].update(drafts)
        elif name == "SEARCH_ISSUES":
            drafts = askers[variables["search"]]
            give(drafts, c)
            for node in (data.get("search") or {}).get("nodes") or []:
                if node and node.get("id"):
                    holders[node["id"]].update(drafts)
    for request in requests:
        if request["document"] != "ISSUE_DETAILS":
            continue
        ids = [v for k, v in request["variables"].items() if k.startswith("id") and v]
        for item in ids:
            give(sorted(holders.get(item, ())) or everyone, float(request.get("cost") or 0) / len(ids))
    return {
        "run": run,
        "drafts": [{"draft": d, "own_points": round(cost[d], 2)} for d in sorted(cost)],
        "total_own_points": round(sum(cost.values()), 2),
    }


def projections() -> dict:
    run = ghq.RECORDINGS / "ten-drafts-ungoverned"
    summary = json.loads((run / "summary.json").read_text())
    priced = json.loads((run / "candidate-prices.json").read_text())
    latency = json.loads((run / "candidate-latency.json").read_text())
    by = summary["by_document"]
    drafts = summary["run"]["drafts"]
    searches = priced["searches_recorded"]
    distinct = priced["distinct_query_texts"]
    hits_per_draft = priced["distinct_hits_read"] / drafts
    board = json.loads((ghq.RECORDINGS / "board-summary.json").read_text())
    board_issues = board["board_issue_counts_from_search"]["issues"]
    search_points = by["SEARCH_BATCH"]["cost"] + by["SEARCH_ISSUES"]["cost"]
    search_seconds = by["SEARCH_BATCH"]["seconds"] + by["SEARCH_ISSUES"]["seconds"]
    detail_seconds = by["ISSUE_DETAILS"]["seconds"] / by["ISSUE_DETAILS"]["requests"]
    prices = priced["prices"]
    scalar_round = prices["candidate 1: 20 scalar search slots x 21 rows"]["cost"]
    by_name_round = prices["candidate 2: 20 search slots x 21 rows, each with Status by name"]["cost"]
    by_name_detail = prices["candidate 2: detail batch of 24 with Status by name, comments"]["cost"]
    today_detail = prices["today: ISSUE_DETAILS (24 items, comments)"]["cost"]
    scalar_seconds = latency["candidate 1: 20 scalar search slots x 21 rows"]["seconds"]
    by_name_detail_seconds = latency["candidate 2: detail batch of 24 with Status by name, comments"]["seconds"]
    searches_50 = LIVE["searches"]
    repeat = 1 - distinct / searches
    # Distinct hits at 50 drafts: the measured hits a draft, each draft's drawn independently
    # from the board (at 10 drafts this gives 2,138 x (1 - (1 - 64.3/2,138)^10) = 563, under
    # the 643 measured, so it errs low); and no overlap at all, capped at the board.
    hits_low = board_issues * (1 - (1 - hits_per_draft / board_issues) ** DRAFTS_50)
    hits_high = min(hits_per_draft * DRAFTS_50, board_issues)

    def verdict(points: float, seconds: float) -> dict:
        return {
            "points": round(points),
            "points_per_draft": round(points / DRAFTS_50, 2),
            "within_10_points_a_draft": points / DRAFTS_50 <= POINTS_BUDGET,
            "seconds": round(seconds),
            "within_300_seconds": seconds <= SECONDS_BUDGET,
        }

    cases = {}
    for label, hits in (("hits drawn independently", hits_low), ("no overlap, capped at the board", hits_high)):
        batches = math.ceil(hits / DETAIL)
        rounds_scalar = math.ceil(searches_50 / SLOTS)
        rounds_dedup = math.ceil(searches_50 * (1 - repeat) / SLOTS)
        today_search = search_points / searches * searches_50
        today_search_s = search_seconds / searches * searches_50
        cases[label] = {
            "distinct_hits": round(hits),
            "detail_batches": batches,
            "today": verdict(today_search + batches * today_detail, today_search_s + batches * detail_seconds),
            "candidate 1: scalar slots of 21, then detail batches": verdict(
                rounds_scalar * scalar_round + batches * today_detail,
                rounds_scalar * scalar_seconds + batches * detail_seconds,
            ),
            "candidate 2: Status by name in slots and detail batches (answers less)": verdict(
                rounds_scalar * by_name_round + batches * by_name_detail,
                rounds_scalar * scalar_seconds + batches * by_name_detail_seconds,
            ),
            "candidate 3: today's slots, one search per distinct text": verdict(
                today_search * (1 - repeat) + batches * today_detail,
                today_search_s * (1 - repeat) + batches * detail_seconds,
            ),
            "candidates 1 + 3": verdict(
                rounds_dedup * scalar_round + batches * today_detail,
                rounds_dedup * scalar_seconds + batches * detail_seconds,
            ),
        }
    return {
        "basis": {
            "searches_at_50_drafts": f"{searches_50}, the live 50-draft run's own count",
            "today_points_per_search": round(search_points / searches, 3),
            "today_seconds_per_search": round(search_seconds / searches, 3),
            "measured_repeat_rate_of_query_texts": round(repeat, 4),
            "distinct_hits_per_draft": hits_per_draft,
            "detail_seconds_per_batch_today": round(detail_seconds, 3),
            "scalar_round_points_dry_run": scalar_round,
            "scalar_round_seconds_measured": scalar_seconds,
            "status_by_name_round_points_dry_run": by_name_round,
            "status_by_name_detail_points_dry_run": by_name_detail,
            "status_by_name_detail_seconds_measured": by_name_detail_seconds,
            "candidate 2's seconds": "its slots timed at the scalar round's measured seconds (not measured with Status)",
        },
        "cases": cases,
    }


def live_run() -> dict:
    run = ghq.RECORDINGS / "ten-drafts-ungoverned"
    bound = json.loads((run / "candidate-prices.json").read_text())["prices"]
    widest = bound["bound: SEARCH_BATCH, 20 first pages and 5 origin slots of 100 items"]["cost"]
    first = bound["today: SEARCH_BATCH first round (20 slots x 3 rows)"]["cost"]
    requests = LIVE["counted_requests"]
    return {
        **LIVE,
        "prepare_own_at_most_if_rounds_as_measured": requests * first,
        "prepare_own_at_most_at_the_priced_bound": requests * widest,
        "unexplained_at_least": LIVE["counted_points"] - requests * widest,
        "unexplained_at_least_if_rounds_as_measured": LIVE["counted_points"] - requests * first,
        "note": (
            "No response of the live run recorded its own cost. Its 380 counted requests include "
            "its governor's rate-limit reads, which cost nothing, so these bounds are generous. "
            "Every measured round was priced 1 or 3 and no measured round asked an origin slot; "
            "the bound prices one asking all 20 first pages and all 5 origin slots at 100 items."
        ),
    }


def main() -> None:
    result = {
        "spend": spend(),
        "runs": {run: run_figures(run) for run in RUNS},
        "per_draft": per_draft("ten-drafts-ungoverned"),
        "projections": projections(),
        "live_run": live_run(),
    }
    (ghq.RECORDINGS / "attribution.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
