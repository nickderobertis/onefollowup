"""Throwaway spike reader (spike-legacy-formats): the three reads the 2026-11-18 board dump lacks.

Reads, with `gh api graphql`, each comment's creation time (by node id, 100 per request), every
comment of the one item with more than 30, the body edit history of five provenance-lost
tickets, and the issue URL of each ticket a task's `delivers` names in the local plan store. Before every request it reads `gh api rate_limit` and waits for the reset while REST
core or GraphQL is under 60% remaining. Writes `extras.json` beside this file and prints the
GraphQL points the reads spent.

    python3 -I read_extras.py <board dump directory> <plan store root (.plans)>

A section `extras.json` already holds is kept and not read again.
"""

import json
import pathlib
import subprocess
import sys
import time

FLOOR = 0.60
HERE = pathlib.Path(__file__).resolve().parent
HISTORY = [
    ("nickderobertis", "onetaskgraph", 3696),
    ("nickderobertis", "onepipeline", 1319),
    ("nickderobertis", "onepipeline", 820),
    ("nickderobertis", "onepipeline", 648),
    ("nickderobertis", "ai-orchestrator", 2319),
]
LONG_ITEM = ("nickderobertis", "ai-orchestrator", 1559)
KEYS = ("draft_sources", "transcripts", "witnesses")


def held_to_floor():
    while True:
        limits = json.loads(subprocess.check_output(["gh", "api", "rate_limit"]))["resources"]
        low = [
            (name, limits[name])
            for name in ("core", "graphql")
            if limits[name]["remaining"] < FLOOR * limits[name]["limit"]
        ]
        if not low:
            return
        reset = max(one["reset"] for _, one in low)
        wait = max(reset - time.time(), 0) + 5
        print(f"under the floor ({', '.join(name for name, _ in low)}); waiting {wait:.0f}s", file=sys.stderr)
        time.sleep(wait)


def graphql(query):
    held_to_floor()
    out = subprocess.run(
        ["gh", "api", "graphql", "-f", f"query={query}"], capture_output=True, text=True
    )
    if out.returncode != 0:
        sys.exit(f"gh api graphql failed: {out.stderr.strip()}\nnext: read the error; rerun once it is fixed")
    return json.loads(out.stdout)["data"]


def delivers(root):
    """Every `delivers` entry of a task in the local plan store, with its task's status."""
    import yaml

    found = {}

    def walk(value, status):
        if isinstance(value, dict):
            status = value.get("status", status) if isinstance(value.get("status"), str) else status
            for key, inner in value.items():
                if key == "delivers" and isinstance(inner, list):
                    for one in inner:
                        if isinstance(one, str):
                            found.setdefault(one, []).append(status)
                else:
                    walk(inner, status)
        elif isinstance(value, list):
            for inner in value:
                walk(inner, status)

    for path in sorted(pathlib.Path(root).rglob("*")):
        if not path.is_file() or path.suffix not in (".md", ".json"):
            continue
        text = path.read_text(errors="replace")
        if "delivers" not in text:
            continue
        try:
            if path.suffix == ".json":
                walk(json.loads(text), None)
            elif text.startswith("---"):
                walk(yaml.safe_load(text.split("\n---", 1)[0][3:]), None)
        except (ValueError, yaml.YAMLError):
            continue
    return found


def main():
    dump = pathlib.Path(sys.argv[1])
    path = HERE / "extras.json"
    held = json.loads(path.read_text()) if path.exists() else {}
    if "comment_created_at" in held and "history" in held:
        spent = held["graphql_points"]
        created, long_comments, history = held["comment_created_at"], held["long_item"], held["history"]
    else:
        created, long_comments, history, spent = read_board(dump)
    if "delivers" not in held:
        claims = delivers(sys.argv[2])
        ids = sorted({one.split(":", 1)[1] for one in claims if one.startswith("followups:")})
        urls = {}
        for at in range(0, len(ids), 100):
            data = graphql(
                f"query {{ rateLimit {{ cost }} nodes(ids: {json.dumps(ids[at : at + 100])}) {{ ... on Issue {{ id url }} }} }}"
            )
            spent += data["rateLimit"]["cost"]
            urls.update({n["id"]: n["url"] for n in data["nodes"] if n})
        held["delivers"] = [
            {"id": one, "url": urls.get(one.split(":", 1)[1]), "task_statuses": statuses}
            for one, statuses in sorted(claims.items())
        ]
    out = {
        "read_at": held.get("read_at") or time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "graphql_points": spent,
        "comment_created_at": created,
        "long_item": long_comments,
        "history": history,
        "delivers": held["delivers"],
    }
    path.write_text(json.dumps(out, indent=1, sort_keys=True) + "\n")
    print(f"graphql points spent: {spent}; comment times: {len(created)}; delivers: {len(out['delivers'])}")


def read_board(dump):
    ids = []
    for page in sorted(dump.glob("page-*.json")):
        for item in json.loads(page.read_text())["data"]["user"]["projectV2"]["items"]["nodes"]:
            ids += [c["id"] for c in item["content"]["comments"]["nodes"]]
    spent = 0
    created = {}
    for at in range(0, len(ids), 100):
        chunk = json.dumps(ids[at : at + 100])
        data = graphql(
            f"query {{ rateLimit {{ cost }} nodes(ids: {chunk}) {{ ... on IssueComment {{ id createdAt }} }} }}"
        )
        spent += data["rateLimit"]["cost"]
        created.update({n["id"]: n["createdAt"] for n in data["nodes"] if n})
    owner, name, number = LONG_ITEM
    data = graphql(
        f'query {{ rateLimit {{ cost }} repository(owner: "{owner}", name: "{name}") {{ issue(number: {number}) '
        "{ comments(first: 100) { totalCount nodes { id createdAt author { login } body } } } } }"
    )
    spent += data["rateLimit"]["cost"]
    long_comments = data["repository"]["issue"]["comments"]
    for one in long_comments["nodes"]:
        created[one["id"]] = one["createdAt"]
    aliases = " ".join(
        f'h{i}: repository(owner: "{o}", name: "{n}") {{ issue(number: {num}) {{ url '
        "userContentEdits(first: 100) { totalCount nodes { editedAt diff } } } }"
        for i, (o, n, num) in enumerate(HISTORY)
    )
    data = graphql(f"query {{ rateLimit {{ cost }} {aliases} }}")
    spent += data["rateLimit"]["cost"]
    history = []
    for i in range(len(HISTORY)):
        issue = data[f"h{i}"]["issue"]
        revisions = issue["userContentEdits"]["nodes"]
        history.append(
            {
                "url": issue["url"],
                "revisions": issue["userContentEdits"]["totalCount"],
                "with_text": sum(1 for r in revisions if r["diff"]),
                "with_record": sum(1 for r in revisions if r["diff"] and "orchestrator.follow-up" in r["diff"]),
                "schemas": sorted({m for r in revisions if r["diff"] for m in __import__("re").findall(r'"schema": ?(\d+)', r["diff"])}),
                "holding": {
                    key: [r["editedAt"] for r in revisions if r["diff"] and f'"{key}"' in r["diff"]]
                    for key in KEYS
                },
            }
        )
    long_comments = {"url": "https://github.com/%s/%s/issues/%d" % LONG_ITEM, **long_comments}
    return created, long_comments, history, spent


if __name__ == "__main__":
    main()
