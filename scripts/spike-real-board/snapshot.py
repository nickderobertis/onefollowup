"""The real `followups` board's shape: every issue of project 3 whose repository GitHub reports
as public, with its number, title, body, state, repository, and how many labels and project
memberships it carries, written to `recordings/board-snapshot.json.gz`; and a summary of it on
standard output. Every other item — an issue of a repository that is not public, a pull
request, a draft issue — is dropped before anything is written; only how many were dropped is
kept. The board's whole issue and open counts are read as GitHub's search counts alone.

    python3 snapshot.py           # read the board (about 2 points a page of 100), then summarize
    python3 snapshot.py --summary # summarize the snapshot already recorded
    python3 snapshot.py --rescrub # drop non-public mentions from the recorded snapshot
"""

from __future__ import annotations

import gzip
import json
import statistics
import sys

import ghq
import public

OWNER = "nickderobertis"
NUMBER = 3
SNAPSHOT = ghq.RECORDINGS / "board-snapshot.json.gz"

PAGE = """query($owner:String!,$number:Int!,$after:String){
  actualCost:rateLimit{cost}
  owner:repositoryOwner(login:$owner){... on ProjectV2Owner{projectV2(number:$number){
    public title
    items(first:100,after:$after){totalCount pageInfo{hasNextPage endCursor}
      nodes{type content{... on Issue{number title body state createdAt url
        repository{nameWithOwner visibility}
        labels(first:1){totalCount}
        projectItems(first:1){totalCount}}}}}}}}}"""


def read_board() -> dict:
    visibility = public.Visibility()
    visibility.preload(OWNER)
    items, after, project = [], None, None
    dropped = 0
    while True:
        answer = ghq.graphql(
            PAGE,
            {"owner": OWNER, "number": NUMBER, "after": after},
            purpose="board snapshot page",
        )
        project = answer["data"]["owner"]["projectV2"]
        connection = project["items"]
        for node in connection["nodes"]:
            content = node.get("content") or {}
            repository = (content.get("repository") or {}) if node["type"] == "ISSUE" else {}
            if not (
                repository.get("visibility") == "PUBLIC"
                and visibility.public(repository.get("nameWithOwner"))
            ):
                dropped += 1
                continue
            items.append(
                {
                    "type": "ISSUE",
                    "number": content["number"],
                    "title": content["title"],
                    "body": content["body"],
                    "state": content["state"],
                    "created_at": content["createdAt"],
                    "url": content["url"],
                    "repository": repository["nameWithOwner"],
                    "labels": content["labels"]["totalCount"],
                    "memberships": content["projectItems"]["totalCount"],
                }
            )
        if not connection["pageInfo"]["hasNextPage"]:
            break
        after = connection["pageInfo"]["endCursor"]
    # A public issue's text may mention a repository or item that is not public: each such
    # mention is replaced by "(dropped)" before anything is written.
    gate = public.Filter(visibility, board_project_id=public.board_project_id())
    items = gate.scrub_mentions(items)
    return {
        "public": project["public"],
        "mentions_dropped_as_not_public": gate.mentions_dropped,
        "title": project["title"],
        "total_items": connection["totalCount"],
        "items_dropped_as_not_public": dropped,
        "counts": counts(),
        "items": items,
    }


def counts() -> dict:
    """GitHub's search counts of the board's issues and open issues: counts only, no item."""
    board = f"project:{OWNER}/{NUMBER} is:issue"
    answer = ghq.graphql(
        "query($a:String!,$o:String!){actualCost:rateLimit{cost} "
        "all:search(query:$a,type:ISSUE,first:0){issueCount} "
        "open:search(query:$o,type:ISSUE,first:0){issueCount}}",
        {"a": board, "o": board + " is:open"},
        purpose="board issue counts",
    )
    return {
        "issues": answer["data"]["all"]["issueCount"],
        "open": answer["data"]["open"]["issueCount"],
    }


def percentiles(values: list[int]) -> dict:
    values = sorted(values)
    if not values:
        return {}
    at = lambda q: values[min(len(values) - 1, int(q * len(values)))]  # noqa: E731
    return {
        "n": len(values),
        "min": values[0],
        "p50": at(0.5),
        "p90": at(0.9),
        "p99": at(0.99),
        "max": values[-1],
        "mean": round(statistics.fmean(values), 1),
    }


def summary(board: dict) -> dict:
    issues = board["items"]
    body_bytes = [len((one["body"] or "").encode()) for one in issues]
    histogram: dict[int, int] = {}
    labels: dict[int, int] = {}
    for one in issues:
        histogram[one["memberships"]] = histogram.get(one["memberships"], 0) + 1
        labels[one["labels"]] = labels.get(one["labels"], 0) + 1
    return {
        "board_public": board["public"],
        "items_on_board": board["total_items"],
        "items_dropped_as_not_public": board["items_dropped_as_not_public"],
        "mentions_dropped_as_not_public": board.get("mentions_dropped_as_not_public"),
        "board_issue_counts_from_search": board["counts"],
        "public_issues": len(issues),
        "public_open": sum(one["state"] == "OPEN" for one in issues),
        "repositories": {
            name: sum(one["repository"] == name for one in issues)
            for name in sorted({one["repository"] for one in issues})
        },
        "body_bytes": percentiles(body_bytes),
        "body_bytes_total": sum(body_bytes),
        "labels_per_issue": dict(sorted(labels.items())),
        "project_memberships_per_issue": dict(sorted(histogram.items())),
    }


def load() -> dict:
    with gzip.open(SNAPSHOT, "rt") as held:
        return json.load(held)


def main() -> None:
    if "--rescrub" in sys.argv:
        board = load()
        gate = public.Filter(board_project_id=public.board_project_id())
        board["items"] = gate.scrub_mentions(board["items"])
        board["mentions_dropped_as_not_public"] = (
            board.get("mentions_dropped_as_not_public") or 0
        ) + gate.mentions_dropped
        with gzip.open(SNAPSHOT, "wt") as out:
            json.dump(board, out, sort_keys=True)
    elif "--summary" not in sys.argv:
        board = read_board()
        ghq.RECORDINGS.mkdir(parents=True, exist_ok=True)
        with gzip.open(SNAPSHOT, "wt") as out:
            json.dump(board, out, sort_keys=True)
    result = summary(load())
    (ghq.RECORDINGS / "board-summary.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
