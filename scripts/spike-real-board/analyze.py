"""Summarize one measurement's recordings: `python3 analyze.py recordings/RUN`.

Writes `RUN/summary.json` and prints it: GitHub's reported cost against the fall of
`x-ratelimit-remaining` per document and per draft; the searches, pages and rows behind each
`SEARCH_BATCH` round; what prepare's own progress lines counted; and every governor hold.
"""

from __future__ import annotations

import gzip
import json
import re
import sys
from collections import defaultdict
from pathlib import Path


def load_jsonl(path: Path) -> list[dict]:
    if not path.exists():
        return []
    opener = gzip.open if path.suffix == ".gz" else open
    lines = []
    with opener(path, "rt") as held:
        try:
            for line in held:
                if line.strip():
                    lines.append(json.loads(line))
        except (EOFError, json.JSONDecodeError):
            # A forwarder stopped by a signal leaves its gzip stream unterminated: what it
            # flushed is kept.
            pass
    return lines


def searches_of(requests: list[dict], answers: dict[int, dict]) -> dict:
    """Every search the batched find asked, with its pages: `text -> [page...]`."""
    walks: dict[str, list[dict]] = defaultdict(list)
    rounds = []
    for request in requests:
        if request.get("document") != "SEARCH_BATCH" or request.get("refused"):
            continue
        variables = request["variables"]
        data = (answers.get(request["seq"]) or {}).get("data") or {}
        slots = []
        for slot in range(20):
            if variables.get(f"ask{slot}") is not True:
                continue
            connection = data.get(f"s{slot}") or {}
            nodes = connection.get("nodes") or []
            page = {
                "seq": request["seq"],
                "first": variables.get(f"first{slot}"),
                "resumed": variables.get(f"after{slot}") is not None,
                "rows": len(nodes),
                "more": (connection.get("pageInfo") or {}).get("hasNextPage"),
            }
            walks[variables[f"search{slot}"]].append(page)
            slots.append(page)
        origins = sum(variables.get(f"origin{slot}") is True for slot in range(5))
        rounds.append(
            {
                "seq": request["seq"],
                "cost": request.get("cost"),
                "fall": request.get("fall"),
                "slots": len(slots),
                "first_pages": sum(not one["resumed"] for one in slots),
                "resumed_pages": sum(one["resumed"] for one in slots),
                "rows_asked": sum(one["first"] or 0 for one in slots),
                "rows": request.get("rows"),
                "origins": origins,
                "response_bytes": request.get("response_bytes"),
                "seconds": request.get("seconds"),
            }
        )
    return {"walks": walks, "rounds": rounds}


def targets_found(prepared: dict, walks: dict, answers: dict, requests: list[dict]) -> list[dict]:
    """For each draft of `drafts.json` prepared, whether its ticket was among the hits its
    queries found, and by how many of them."""
    drafts = json.loads((Path(__file__).resolve().parent / "drafts.json").read_text())
    by_title = {draft["title"]: draft for draft in drafts}
    numbers: dict[str, set] = defaultdict(set)
    for request in requests:
        if request.get("document") not in ("SEARCH_BATCH", "SEARCH_ISSUES") or request.get("refused"):
            continue
        data = (answers.get(request["seq"]) or {}).get("data") or {}
        variables = request["variables"]
        for key, value in data.items():
            if not isinstance(value, dict):
                continue
            text = variables.get("search" + key[1:]) if key.startswith("s") and key != "search" else variables.get("search")
            for node in value.get("nodes") or []:
                if node and "number" in node:
                    numbers[text].add((node["repository"]["nameWithOwner"], node["number"]))
    found = []
    for search in prepared.get("duplicate_searches") or []:
        texts = [query["text"] for query in search["queries"]]
        draft = by_title.get(texts[0]) if texts else None
        if draft is None:
            continue
        target = (draft["ticket"]["repository"], draft["ticket"]["number"])
        finding = [
            query["text"] for query in search["queries"]
            if any(target in hits for text, hits in numbers.items() if text and text.endswith('"' + query["text"] + '"'))
        ]
        found.append(
            {
                "ticket": f"{target[0]}#{target[1]}",
                "ticket_open": draft["ticket_open"],
                "found": bool(finding),
                "by_queries": finding,
            }
        )
    return found


def main() -> None:
    run = Path(sys.argv[1])
    requests = load_jsonl(run / "requests.jsonl")
    answers = {one["seq"]: one["answer"] for one in load_jsonl(run / "responses.jsonl.gz")}
    graphql = [one for one in requests if one.get("document") != "REST /rate_limit"]
    sent = [one for one in graphql if not one.get("refused")]
    meta = json.loads((run / "run.json").read_text()) if (run / "run.json").exists() else {}
    drafts = meta.get("drafts") or 1

    by_document: dict[str, dict] = {}
    for one in sent:
        held = by_document.setdefault(
            one["document"],
            {"requests": 0, "cost": 0, "fall": 0, "fall_unknown": 0, "rows": 0,
             "request_bytes": 0, "response_bytes": 0, "seconds": 0.0, "costs": []},
        )
        held["requests"] += 1
        held["cost"] += one.get("cost") or 0
        held["costs"].append(one.get("cost"))
        if one.get("fall") is None:
            held["fall_unknown"] += 1
        else:
            held["fall"] += one["fall"]
        held["rows"] += one.get("rows") or 0
        held["request_bytes"] += one.get("request_bytes") or 0
        held["response_bytes"] += one.get("response_bytes") or 0
        held["seconds"] = round(held["seconds"] + (one.get("seconds") or 0), 3)
    for held in by_document.values():
        held["costs"] = sorted(set(held["costs"]), key=lambda c: (c is None, c))

    # Prepare's own requests apart from the spike's (`forwarder:`), which are reported beside.
    own = sum(one.get("cost") or 0 for one in sent if not one["document"].startswith("forwarder:"))
    spike_own = sum(one.get("cost") or 0 for one in sent if one["document"].startswith("forwarder:"))
    fall = sum(one.get("fall") or 0 for one in sent)
    stderr = (run / "prepare.stderr").read_text() if (run / "prepare.stderr").exists() else ""
    finished = re.findall(r"^onefollowup prepare: (.+?): finished: (\d+) requests, (\d+) points$", stderr, re.M)
    holds = [line for line in stderr.splitlines() if "holding requests" in line]
    found = searches_of(requests, answers)
    walks = found["walks"]
    pages_per_search = defaultdict(int)
    for pages in walks.values():
        pages_per_search[len(pages)] += 1
    hits = {text: sum(page["rows"] for page in pages) for text, pages in walks.items()}

    prepared = {}
    if (run / "prepare.stdout.json").exists():
        try:
            prepared = json.loads((run / "prepare.stdout.json").read_text())
        except json.JSONDecodeError:
            prepared = {}
    per_draft_queries = [len(one.get("queries") or []) for one in prepared.get("duplicate_searches") or []]

    targets = targets_found(prepared, walks, answers, requests)

    summary = {
        "run": meta,
        "requests": len(sent),
        "refused": len(graphql) - len(sent),
        "rate_limit_reads": len(requests) - len(graphql),
        "reported_points": own,
        "spike_reported_points_in_interval": spike_own,
        "shared_fall": fall,
        "falls_unknown": sum(one.get("fall") is None for one in sent),
        "own_share_of_fall": round(own / fall, 4) if fall else None,
        "reported_points_per_draft": round(own / drafts, 2),
        "shared_fall_per_draft": round(fall / drafts, 2),
        "seconds_in_requests": round(sum(one.get("seconds") or 0 for one in sent), 1),
        "by_document": by_document,
        "prepare_progress_finished": [
            {"phase": phase, "requests": int(r), "points": int(p)} for phase, r, p in finished
        ],
        "searches": len(walks),
        "queries_per_draft": per_draft_queries,
        "pages_per_search": dict(sorted(pages_per_search.items())),
        "searches_ending_at_21": sum(1 for count in hits.values() if count >= 21),
        "hits_per_search": dict(sorted(hits.items(), key=lambda kv: -kv[1])),
        "rounds": found["rounds"],
        "governor_holds": holds,
        "targets_found": targets,
    }
    (run / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=False) + "\n")
    brief = {key: summary[key] for key in summary if key not in ("rounds", "hits_per_search")}
    print(json.dumps(brief, indent=2))


if __name__ == "__main__":
    main()
