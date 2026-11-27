"""Price candidate duplicate-search designs on the real board, and count hits per query.

    python3 price.py candidates RUN   # dry-run prices, written to RUN/candidate-prices.json
    python3 price.py hits RUN         # issueCount per distinct query RUN searched (1 point a 20)
    python3 price.py latency RUN      # one real round of each candidate shape, timed

RUN is a measurement's recordings directory; every query text, page size and hit id priced
is one that run recorded. Dry runs are charged at most 1 each in the ledger (`ghq`).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import analyze
import ghq
import pricing
import public

SLOTS = 20
DETAIL = 24
KEPT = 21

SCALAR_HIT = "__typename ... on Issue{id number title state updatedAt repository{nameWithOwner}}"
STATUS_BY_NAME = (
    'projectItems(first:$boardItems){nodes{id project{id number} '
    'status:fieldValueByName(name:"Status"){... on ProjectV2ItemFieldSingleSelectValue{name optionId}}}}'
)


def search_slots(texts: list[str], first: int, hit: str) -> tuple[str, dict]:
    """One request of `len(texts)` aliased searches, each `first` rows of `hit`."""
    head = ",".join(f"$q{n}:String!" for n in range(len(texts)))
    slots = " ".join(
        f"s{n}:search(query:$q{n},type:ISSUE,first:{first}){{issueCount pageInfo{{hasNextPage endCursor}} nodes{{{hit}}}}}"
        for n in range(len(texts))
    )
    variables = {f"q{n}": text for n, text in enumerate(texts)}
    if "$boardItems" in hit:
        head += ",$boardItems:Int!"
        variables["boardItems"] = 3
    return f"query({head}){{{slots}}}", variables


def detail_batch(ids: list[str], selection: str) -> tuple[str, dict]:
    head = ",".join(f"$id{n}:ID!" for n in range(len(ids)))
    aliases = " ".join(f"i{n}:node(id:$id{n}){{{selection}}}" for n in range(len(ids)))
    variables = {f"id{n}": one for n, one in enumerate(ids)}
    extra = []
    if "$first" in selection:
        extra.append("$first:Int!")
        variables["first"] = 100
    if "$nestedFirst" in selection:
        extra.append("$nestedFirst:Int!")
        variables["nestedFirst"] = 50
    if "$boardItems" in selection:
        extra.append("$boardItems:Int!")
        variables["boardItems"] = 3
    return f"query({','.join([head] + extra)}){{{aliases}}}", variables


# What today's ISSUE_DETAILS reads of each item, less the fragment's board field values, with
# the board's Status read by name: the second candidate's detail read.
DETAIL_STATUS_BY_NAME = (
    "__typename ... on Issue{id number title body url createdAt updatedAt state "
    "repository{nameWithOwner} parent{id} subIssuesSummary{total} commentCount:comments{totalCount} "
    "labels(first:$nestedFirst){nodes{id name color}pageInfo{hasNextPage}} "
    + STATUS_BY_NAME
    + " comments(first:$first){nodes{id body createdAt author{login}}pageInfo{hasNextPage endCursor}}}"
)


def recorded(run: Path):
    requests = analyze.load_jsonl(run / "requests.jsonl")
    texts: list[str] = []
    for request in requests:
        if request.get("document") == "SEARCH_BATCH" and not request.get("refused"):
            variables = request["variables"]
            for slot in range(SLOTS):
                if variables.get(f"ask{slot}") is True and variables.get(f"after{slot}") is None:
                    texts.append(variables[f"search{slot}"])
        if request.get("document") == "SEARCH_ISSUES" and request["variables"].get("after") is None:
            texts.append(request["variables"]["search"])
    ids: list[str] = []
    for request in requests:
        if request.get("document") == "ISSUE_DETAILS":
            for slot in range(DETAIL):
                held = request["variables"].get(f"id{slot}")
                if held and held != public.DROPPED and held not in ids:
                    ids.append(held)
    return requests, texts, ids


def document_of(run: Path, request: dict) -> str:
    return (run / "documents" / f"{request['sha']}.graphql").read_text()


def candidates(run: Path) -> dict:
    requests, texts, ids = recorded(run)
    distinct = list(dict.fromkeys(texts))
    batch = [one for one in requests if one.get("document") == "SEARCH_BATCH"]
    details = [one for one in requests if one.get("document") == "ISSUE_DETAILS"]
    first_round, resumed_round = batch[0], next(
        one for one in batch if one["variables"].get("after0") is not None
    )
    priced = {}

    def price(name: str, document: str, variables: dict) -> None:
        priced[name] = pricing.dry_price(document, variables, purpose=f"candidate price: {name}")

    # Today's documents, as recorded, to hold the dry run to GitHub's reported cost.
    price("today: SEARCH_BATCH first round (20 slots x 3 rows)", document_of(run, first_round), first_round["variables"])
    price("today: SEARCH_BATCH resumed round (20 slots sharing 20 rows)", document_of(run, resumed_round), resumed_round["variables"])
    # A dropped id (an item whose repository is not public) is priced as a kept one: the
    # price depends on how many items a batch reads, not which.
    detail_variables = {
        key: (ids[0] if held == public.DROPPED else held) for key, held in details[0]["variables"].items()
    }
    price("today: ISSUE_DETAILS (24 items, comments)", document_of(run, details[0]), detail_variables)
    # The most a SEARCH_BATCH request can be priced at: every slot's first page and every
    # origin slot asked at 100 board items, a bound on any request of today's find.
    widest = dict(first_round["variables"])
    for slot in range(SLOTS):
        widest[f"ask{slot}"], widest[f"first{slot}"] = True, 3
        widest.setdefault(f"search{slot}", distinct[slot % len(distinct)])
    for slot in range(5):
        widest[f"origin{slot}"], widest[f"itemsFirst{slot}"], widest[f"filter{slot}"] = True, 100, "is:issue"
    price("bound: SEARCH_BATCH, 20 first pages and 5 origin slots of 100 items", document_of(run, first_round), widest)
    # Today's SEARCH_BATCH slots asking all 21 kept hits at once.
    widened = dict(first_round["variables"])
    for slot in range(SLOTS):
        if widened.get(f"ask{slot}") is True:
            widened[f"first{slot}"] = KEPT
    price("today's SEARCH_BATCH, every slot one 21-row page", document_of(run, first_round), widened)
    # Candidate 1: scalar hits, paged to the 21 kept, then detail batches of distinct hits.
    document, variables = search_slots(distinct[:SLOTS], KEPT, SCALAR_HIT)
    price("candidate 1: 20 scalar search slots x 21 rows", document, variables)
    document, variables = search_slots(distinct[:SLOTS], 100, SCALAR_HIT)
    price("candidate 1: 20 scalar search slots x 100 rows", document, variables)
    # Candidate 2: the same, with Status read by name.
    document, variables = search_slots(distinct[:SLOTS], KEPT, SCALAR_HIT[:-1] + " " + STATUS_BY_NAME + "}")
    price("candidate 2: 20 search slots x 21 rows, each with Status by name", document, variables)
    document, variables = detail_batch(ids[:DETAIL], DETAIL_STATUS_BY_NAME)
    price("candidate 2: detail batch of 24 with Status by name, comments", document, variables)
    return {
        "run": run.name,
        "searches_recorded": len(texts),
        "distinct_query_texts": len(distinct),
        "distinct_hits_read": len(ids),
        "prices": priced,
    }


def hits(run: Path) -> dict:
    _, texts, _ = recorded(run)
    distinct = list(dict.fromkeys(texts))
    counted = {}
    for at in range(0, len(distinct), SLOTS):
        chunk = distinct[at : at + SLOTS]
        head = ",".join(f"$q{n}:String!" for n in range(len(chunk)))
        slots = " ".join(f"s{n}:search(query:$q{n},type:ISSUE,first:1){{issueCount}}" for n in range(len(chunk)))
        answer = ghq.graphql(
            f"query({head}){{actualCost:rateLimit{{cost}} {slots}}}",
            {f"q{n}": text for n, text in enumerate(chunk)},
            purpose="hits per query",
        )
        for n, text in enumerate(chunk):
            counted[text] = answer["data"][f"s{n}"]["issueCount"]
    return counted


def latency(run: Path) -> dict:
    """One real request of each candidate shape: cost, bytes, seconds."""
    _, texts, ids = recorded(run)
    distinct = list(dict.fromkeys(texts))
    measured = {}
    for name, (document, variables) in {
        "candidate 1: 20 scalar search slots x 21 rows": search_slots(distinct[:SLOTS], KEPT, SCALAR_HIT),
        "candidate 2: detail batch of 24 with Status by name, comments": detail_batch(ids[:DETAIL], DETAIL_STATUS_BY_NAME),
    }.items():
        document = pricing.with_selection(document, "actualCost:rateLimit{cost}")
        before = len(ghq.LEDGER.read_text().splitlines())
        answer = ghq.graphql(document, variables, purpose=f"latency: {name}")
        entry = json.loads(ghq.LEDGER.read_text().splitlines()[before])
        measured[name] = {
            "cost": entry["charged"],
            "seconds": entry["seconds"],
            "response_bytes": entry["response_bytes"],
            "rows": analyze_rows(answer),
        }
    return measured


def analyze_rows(answer: dict) -> int:
    rows = 0
    for value in (answer.get("data") or {}).values():
        if isinstance(value, dict) and isinstance(value.get("nodes"), list):
            rows += len(value["nodes"])
        elif isinstance(value, dict) and value.get("__typename"):
            rows += 1
    return rows


def main() -> None:
    verb, run = sys.argv[1], Path(sys.argv[2])
    if verb == "candidates":
        result = candidates(run)
        (run / "candidate-prices.json").write_text(json.dumps(result, indent=2) + "\n")
    elif verb == "hits":
        result = hits(run)
        (run / "hits-per-query.json").write_text(json.dumps(result, indent=2) + "\n")
    elif verb == "latency":
        result = latency(run)
        (run / "candidate-latency.json").write_text(json.dumps(result, indent=2) + "\n")
    else:
        sys.exit(f"unknown verb {verb}")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
