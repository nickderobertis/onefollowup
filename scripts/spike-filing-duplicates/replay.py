#!/usr/bin/env python3
"""Replay one duplicate cluster's filing-time searches against a destination, read-only.

Spike harness for `spike-filing-duplicates` (follow-ups-grooming-verb-2026-11-21); it is
not landed. A cluster is one root cause the 2026-11-20 grooming pass found filed more than
once. For each later member it asks the destination, in one `onetaskgraph task find`
through the host's credential wrapper, the searches its filing made or would have made:

  prepare   the draft titles, as `prepare` searches them (text in title and body)
  agent     the queries the verifying agent actually asked, where its transcript survives
  template  what the task template's guidance yields from the ticket's own text: the
            invariant (its title), the root cause's first sentence, the failing file
  terms     the ticket's rarest words, one per query

and prints, per search, whether an earlier member came back.

  python3 scripts/spike-filing-duplicates/replay.py --cluster onevcs#447 --destination followups

`--all --json PATH` replays every cluster and writes the measurements the report is built
from. `--dry-run` prints the searches without asking the store. Before every find the
GraphQL allowance is read and the run stops when it is under `--floor`.

No committed test drives this: it runs once, against the live board, to produce one report,
and its output is quoted there; a test would guard nothing after the spike.
"""

from __future__ import annotations

import argparse
import glob
import json
import math
import os
import re
import subprocess
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path

HOST_ROOT = Path("/home/whoop/ai-orchestrator")
GROOM = HOST_ROOT / "scratch/groom"
# onefollowup replaced the host's own follow-up code in ai-orchestrator 7ebe3bd2.
ONEFOLLOWUP_ADOPTED = datetime(2026, 11, 15, 23, 51, 19, tzinfo=timezone.utc)
# A member created this close before a later member's search may not be in GitHub's index.
INDEX_LAG = timedelta(minutes=15)
TERMS_PER_TICKET = 5
# A word in more earlier tickets than this is no distinguishing term, and every hit it
# returns is a row the store pages through and points the search spends.
TERM_MAX_DOCUMENTS = 3
# Other work on this host draws on the same GraphQL allowance; measured at 8 points a minute
# (4609 -> 4601 over 60 s at 08:41 on 2026-11-21, nothing of this spike running).
BACKGROUND_POINTS_PER_SECOND = 8 / 60
STOPWORDS = set(
    """a an and are as at be been but by can cannot does each every for from has have how
    in into is it its itself last more must never no not of on once one only or other
    over same so still than that the their them then there these they this through to
    under until was what when where whether which while who whose will with without
    would after before because both instead also any being do done even first itself
    run runs ticket tickets should against back down about across within upon""".split()
)


def fail(message: str, action: str) -> None:
    print(f"replay: {message}; {action}", file=sys.stderr)
    raise SystemExit(2)


def parse_time(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def metadata(body: str) -> dict:
    found = re.search(r"<!-- onetaskgraph.metadata\n(.*?)\n-->", body, re.S)
    if not found:
        return {}
    try:
        return json.loads(found.group(1))
    except json.JSONDecodeError:
        return {}


def prose(body: str) -> str:
    """The body as onetaskgraph's in-process filter reads it: without the metadata slot."""
    return re.sub(r"<!-- onetaskgraph.metadata\n.*?\n-->", "", body, flags=re.S)


def section(body: str, heading: str) -> str:
    found = re.search(rf"^## {heading}\s*\n(.*?)(?=^<a id=|^## |\Z)", body, re.S | re.M)
    return found.group(1).strip() if found else ""


@dataclass
class Member:
    key: str
    id: str
    title: str
    body: str
    created: datetime
    record: dict
    origin: str

    @property
    def batch(self) -> str:
        return self.record.get("created_by_run", "")

    @property
    def bare_title(self) -> str:
        repo = self.key.split("#")[0]
        return re.sub(rf"^{re.escape(repo)}:\s*", "", self.title).strip()


@dataclass
class Search:
    strategy: str
    text: str
    source: str
    hits: list[str] = field(default_factory=list)
    complete: bool = True
    # Whether onetaskgraph's in-process rule (the whole text, case-insensitively, inside the
    # title or prose) would keep each earlier member; GitHub's token index decides first.
    substring_in: list[str] = field(default_factory=list)
    # Answered at filing time and kept in the batch's state, so it is read rather than asked.
    recorded: bool = False


def gh_issue(key: str) -> dict:
    repo, number = key.split("#")
    done = subprocess.run(
        ["gh", "api", f"repos/nickderobertis/{repo}/issues/{number}"],
        capture_output=True,
        text=True,
    )
    if done.returncode != 0:
        fail(f"gh api could not read {key}: {done.stderr.strip()}", "check `gh auth status`")
    issue = json.loads(done.stdout)
    return {
        "issue_id": issue["node_id"],
        "title": issue["title"],
        "body": issue.get("body") or "",
        "created": issue["created_at"],
    }


def load_members(groom: Path) -> tuple[dict[str, Member], list[dict], list[dict]]:
    try:
        tickets = json.loads((groom / "tickets.json").read_text())
        confirmed = json.loads((groom / "dups.json").read_text())["confirmed"]
        extra = json.loads((groom / "dups-extra.json").read_text())["confirmed"]
    except (OSError, KeyError, json.JSONDecodeError) as error:
        fail(f"the grooming records under {groom} are unreadable ({error})", "pass --groom")
    known = {f"{t['repo']}#{t['number']}": t for t in tickets}
    members: dict[str, Member] = {}
    for cluster in confirmed + extra:
        for key in cluster["members"]:
            raw = known.get(key) or gh_issue(key)
            meta = metadata(raw["body"])
            members[key] = Member(
                key=key,
                id=f"followups:{raw['issue_id']}",
                title=raw["title"],
                body=raw["body"],
                created=parse_time(raw["created"]),
                record=meta.get("orchestrator.follow-up", {}),
                origin=meta.get("onetaskgraph.origin", ""),
            )
    return members, confirmed, extra


def merged_survivors(groom: Path) -> set[str]:
    """The clusters the pass merged, as opposed to confirmed ones it closed as fixed."""
    actions = json.loads((groom / "actions.json").read_text())
    return {a["key"] for a in actions if a["kind"] == "survivor"}


class DraftTitles:
    """Each draft's own title, from whatever local record of it survives."""

    def __init__(self, host: Path):
        self.titles: dict[str, tuple[str, str]] = {}
        for path in glob.glob(str(host / ".onefollowup/state/*.json")):
            try:
                state = json.loads(Path(path).read_text())
            except (OSError, json.JSONDecodeError):
                continue
            for entry in state.get("inputs", []):
                if entry.get("title"):
                    self.titles[entry["id"]] = (entry["title"], "batch state")
        for root in (host / ".follow-ups", host / ".onefollowup"):
            for path in root.rglob("*"):
                if path.suffix == ".md" and "drafts" in path.parts:
                    head = path.read_text(errors="replace")[:2000]
                    found = re.search(r"^title:\s*(.+)$", head, re.M)
                    if found:
                        self.titles.setdefault(path.stem, (found.group(1).strip("'\" "), "draft file"))
                elif path.suffix == ".json" and re.match(r"\d{8}T\d{6}Z-", path.name):
                    self._json(path)

    def _json(self, path: Path) -> None:
        try:
            data = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError, UnicodeDecodeError):
            return
        for entry in data.get("items", []) if isinstance(data, dict) else []:
            item = entry.get("item") or {}
            if isinstance(item, dict) and item.get("title") and entry.get("id"):
                self.titles.setdefault(entry["id"], (item["title"], "draft read"))

    def of(self, draft: str) -> tuple[str, str]:
        if draft in self.titles:
            return self.titles[draft]
        stem = draft.rsplit("/", 1)[-1]
        if stem in self.titles:
            return self.titles[stem]
        # The id carries the title's first 48 characters, slugged; the last word may be cut.
        # A phrase of its whole words is a prefix of the title's, so it finds at least what
        # the whole title would.
        slug = re.sub(r"^\d{8}T\d{6}Z-", "", stem)
        words = slug.split("-")
        if len(slug) >= 48 and len(words) > 1:
            words = words[:-1]
        return " ".join(w for w in words if w), "reconstructed from the draft id"


def agent_queries(host: Path, batch: str, slug: str) -> list[str]:
    """The `ticket search` questions the verifying agent asked for `slug`, if recorded."""
    asked: list[str] = []
    for sessions in glob.glob(str(host / "runs" / f"{glob.escape(batch)}*follow-ups*/oneharness-sessions.jsonl")):
        for line in Path(sessions).read_text().splitlines():
            try:
                history = Path(json.loads(line)["history_file"])
            except (KeyError, json.JSONDecodeError):
                continue
            if not history.exists():
                continue
            for event in history.read_text().splitlines():
                try:
                    command = json.loads(event).get("event", {}).get("input") or {}
                except json.JSONDecodeError:
                    continue
                text = command.get("command", "") if isinstance(command, dict) else ""
                if "ticket search" not in text and "'ticket','search'" not in text:
                    continue
                for pair in re.finditer(r"\('([^']+)','([^']+)'\)", text):
                    if pair.group(1) == slug:
                        asked.append(pair.group(2))
                for flag in re.finditer(rf"--ticket {re.escape(slug)} '([^']+)'", text):
                    asked.append(flag.group(1))
    return list(dict.fromkeys(asked))


WORD = re.compile(r"[A-Za-z][A-Za-z0-9_]*(?:[.-][A-Za-z0-9_]+)*")


def words(text: str) -> list[str]:
    return [w.lower() for w in WORD.findall(text)]


def rarest_terms(later: Member, corpus: list[Member], count: int) -> list[str]:
    """`later`'s rarest words over the board as it stood when it was filed.

    Document frequency is counted over the tickets created before `later`, its own cluster
    left out so the ranking uses nothing the filer could not have known.
    """
    documents = [set(words(m.title + " " + prose(m.body))) for m in corpus]
    own = words(later.bare_title + " " + section(later.body, "Root cause"))
    seen: dict[str, float] = {}
    for term in own:
        if term in STOPWORDS or len(term) < 4 or term in seen:
            continue
        df = sum(term in doc for doc in documents)
        if df <= TERM_MAX_DOCUMENTS:
            seen[term] = math.log((len(documents) + 1) / (df + 1))
    return [t for t, _ in sorted(seen.items(), key=lambda kv: -kv[1])][:count]


def template_queries(later: Member) -> list[tuple[str, str]]:
    root = section(later.body, "Root cause")
    first = re.split(r"(?<=[.!?])\s", root.replace("\n", " "), maxsplit=1)[0].strip().rstrip(".")
    queries = [(later.bare_title, "invariant: the title"), (first, "invariant: root cause sentence")]
    fix = section(later.body, "Suggested fix") + " " + section(later.body, "Root cause")
    path = re.search(r"`([\w./-]+\.(?:rs|py|sh|mjs|ts|tsx|js|toml|ya?ml|json|j2|md))`", fix)
    if path:
        queries.append((path.group(1).rsplit("/", 1)[-1], "failing file"))
    return [(q, why) for q, why in queries if q]


def build_searches(later: Member, earlier: list[Member], members: dict[str, Member], titles: DraftTitles, host: Path) -> list[Search]:
    searches: list[Search] = []
    for draft in later.record.get("drafts", []):
        title, source = titles.of(draft)
        searches.append(Search("prepare", title, source))
    slug = later.record.get("root_cause", "")
    state = batch_state(host, later.batch)
    for query in agent_queries(host, later.batch, slug):
        searches.append(Search("agent", query, "verifying agent's transcript"))
    if state is not None:
        searches.extend(recorded_searches(state, later, slug))
    for query, why in template_queries(later):
        searches.append(Search("template", query, why))
    corpus = [m for m in members.values() if m.created < later.created and m not in earlier]
    corpus += [m for m in TICKET_CORPUS if m.created < later.created]
    for term in rarest_terms(later, corpus, TERMS_PER_TICKET):
        searches.append(Search("terms", term, "rarest word"))
    unique: dict[tuple[str, str], Search] = {}
    for search in searches:
        unique.setdefault((search.strategy, search.text.lower()), search)
    for search in unique.values():
        if search.recorded:
            continue
        needle = search.text.lower()
        search.substring_in = [
            m.key for m in earlier if needle in m.title.lower() or needle in prose(m.body).lower()
        ]
    return list(unique.values())


TICKET_CORPUS: list[Member] = []


def batch_state(host: Path, batch: str) -> dict | None:
    path = host / ".onefollowup/state" / f"{batch}.json"
    try:
        return json.loads(path.read_text()) if batch and path.is_file() else None
    except (OSError, json.JSONDecodeError):
        return None


def recorded_searches(state: dict, later: Member, slug: str) -> list[Search]:
    """What prepare and the agent's `ticket search` found when this batch ran.

    Each read keeps the keys of the searches that found it: `input:<n>` for prepare's
    search of the n-th draft's title, `search:<ticket>:<n>` for the agent's. One ticket's
    calls share their keys, so the agent's questions are answered here as one union.
    """
    drafts = later.record.get("drafts", [])
    inputs = [f"input:{n}" for n, entry in enumerate(state.get("inputs", [])) if entry.get("id") in drafts]
    reads = state.get("reads", [])

    def found(test) -> list[str]:
        return [r["item"]["id"] for r in reads if any(test(key) for key in r.get("found_by", []))]

    prepare = Search("prepare-recorded", " | ".join(inputs), "batch state at filing", recorded=True)
    prepare.hits = found(lambda key: key in inputs)
    agent = Search("agent-recorded", f"search:{slug}:*", "batch state at filing", recorded=True)
    agent.hits = found(lambda key: key.startswith(f"search:{slug}:"))
    return [prepare, agent]


def graphql_remaining(host: Path) -> int:
    done = subprocess.run(
        [str(host / "scripts/plan-store.sh"), "bash", "-c",
         'GH_TOKEN="$GH_PROJECTS_TOKEN" gh api rate_limit --jq .resources.graphql.remaining'],
        cwd=host, capture_output=True, text=True,
    )
    if done.returncode != 0 or not done.stdout.strip().isdigit():
        fail(f"could not read GET /rate_limit: {done.stderr.strip()}", "check the board credential")
    return int(done.stdout.strip())


def find(host: Path, destination: str, searches: list[Search], scratch: Path) -> None:
    request = {
        "schema_version": 1,
        "queries": [{"key": f"q{i}", "text": s.text, "in": "both"} for i, s in enumerate(searches)],
    }
    path = scratch / f"find.{os.getpid()}.json"
    path.write_text(json.dumps(request))
    done = subprocess.run(
        [str(host / "scripts/plan-store.sh"), str(host / ".venv/bin/onetaskgraph"), "task", "find",
         destination, "--queries", str(path), "--json"],
        cwd=host, capture_output=True, text=True,
    )
    path.unlink()
    if done.returncode not in (0, 4):
        fail(f"task find refused: {done.stderr.strip()[-800:]}", "read the refusal, then rerun")
    try:
        answers = {a["key"]: a for a in json.loads(done.stdout)["answers"]}
    except (json.JSONDecodeError, KeyError) as error:
        fail(f"task find answered no FoundTasks ({error})", "rerun with --dry-run to see the request")
    for i, search in enumerate(searches):
        answer = answers.get(f"q{i}", {})
        search.hits = [item["id"] for item in answer.get("items", [])]
        search.complete = bool(answer.get("complete", False))


def timing(later: Member, earlier: list[Member]) -> str | None:
    """A cause no search could have answered: the earlier copy was not there to find."""
    if all(not m.record for m in earlier):
        return "earlier member is not on the destination board"
    if any(m.batch == later.batch for m in earlier):
        return "same batch filed both"
    if any(m.origin and m.origin == later.origin for m in earlier):
        return "re-copy of one ticket"
    searched = parse_time(later.record["verified_at"]) if later.record.get("verified_at") else later.created
    if all(m.created > searched for m in earlier):
        return "concurrent batch: earlier member filed after this search"
    if all(m.created > searched - INDEX_LAG for m in earlier):
        return "earlier member inside the index-lag window"
    return None


def filing_phrase(search: dict) -> bool:
    """A search the filing made, or the template asks before its last question (the file)."""
    if search["strategy"] in ("prepare", "prepare-recorded", "agent", "agent-recorded"):
        return True
    return search["strategy"] == "template" and search["source"] != "failing file"


def classify(row: dict) -> str:
    if row["timing"]:
        return row["timing"]
    found = {s["strategy"] for s in row["searches"] if s["found"]}
    if found & {"agent", "agent-recorded"}:
        return "found by the agent's own search, judged distinct"
    if found & {"prepare", "prepare-recorded"}:
        return "found by prepare, never shown to the agent"
    asked = [s for s in row["searches"] if filing_phrase(s) and not s.get("recorded")]
    if any(s["substring_in"] and not s["found"] for s in asked):
        return "whole-word rule: text inside the earlier member, not returned by GitHub"
    return "phrase mismatch: no filing-time phrase occurs in the earlier member"


def replay(cluster: dict, members: dict[str, Member], titles: DraftTitles, args, merged: set[str]) -> dict:
    ordered = sorted((members[k] for k in cluster["members"]), key=lambda m: m.created)
    rows = []
    for at, later in enumerate(ordered[1:], start=1):
        earlier = ordered[:at]
        searches = build_searches(later, earlier, members, titles, args.host)
        if not args.dry_run:
            remaining = graphql_remaining(args.host)
            if args.spent >= args.budget:
                fail(f"this run has moved the allowance by {args.spent}, its budget {args.budget}", "raise nothing; report what was measured")
            if remaining < args.floor:
                fail(f"GraphQL remaining {remaining} is under the floor {args.floor}", "wait for the reset, then rerun")
            started = time.monotonic()
            find(args.host, args.destination, [s for s in searches if not s.recorded], args.scratch)
            elapsed = time.monotonic() - started
            moved = remaining - graphql_remaining(args.host)
            args.spent += moved
        else:
            moved = elapsed = None
        wanted = {m.id: m.key for m in earlier}
        row = {
            "later": later.key,
            "title": later.title,
            "batch": later.batch,
            "created": later.created.isoformat(),
            "verified_at": later.record.get("verified_at"),
            "era": "onefollowup" if later.created >= ONEFOLLOWUP_ADOPTED else "host follow-up flow",
            "slug": later.record.get("root_cause"),
            "earlier": [{"key": m.key, "created": m.created.isoformat(), "batch": m.batch, "slug": m.record.get("root_cause")} for m in earlier],
            "account_points_moved": moved,
            "find_seconds": elapsed,
            "searches": [
                {
                    "strategy": s.strategy,
                    "text": s.text,
                    "source": s.source,
                    "hits": len(s.hits),
                    "complete": s.complete,
                    "found": sorted(wanted[h] for h in s.hits if h in wanted),
                    "substring_in": s.substring_in,
                    "recorded": s.recorded,
                }
                for s in searches
            ],
        }
        row["timing"] = timing(later, earlier)
        row["cause"] = classify(row) if not args.dry_run else None
        rows.append(row)
    return {"survivor": cluster["survivor"], "merged": cluster["survivor"] in merged, "members": cluster["members"], "later": rows}


def print_cluster(result: dict, dry_run: bool) -> None:
    print(f"{result['survivor']}  ({'merged' if result['merged'] else 'confirmed, closed as fixed'})")
    for row in result["later"]:
        earlier = ", ".join(e["key"] for e in row["earlier"])
        print(f"  later {row['later']} [{row['era']}] vs earlier {earlier}")
        for s in row["searches"]:
            mark = "-" if dry_run else ("FOUND " + ",".join(s["found"]) if s["found"] else f"missed ({s['hits']} hits)")
            print(f"    {s['strategy']:<8} {mark:<28} {s['text'][:90]!r}")
        if not dry_run:
            print(f"    cause: {row['cause']}; account GraphQL moved {row['account_points_moved']}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--cluster", help="any member of the cluster, e.g. onevcs#447")
    target.add_argument("--all", action="store_true", help="every confirmed cluster")
    parser.add_argument("--destination", default="followups", help="the host's configured source")
    parser.add_argument("--host", type=Path, default=HOST_ROOT, help="the orchestrator checkout")
    parser.add_argument("--groom", type=Path, default=GROOM, help="the grooming pass's records")
    parser.add_argument("--floor", type=int, default=3000, help="stop below this GraphQL remaining")
    parser.add_argument("--budget", type=int, default=260, help="stop once the allowance has moved this much")
    parser.add_argument("--json", type=Path, help="write the measurements here")
    parser.add_argument("--dry-run", action="store_true", help="print the searches, ask nothing")
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", args.destination):
        fail(f"{args.destination!r} is no source name", "pass a source onetaskgraph.yaml declares")
    if not (args.host / "scripts/plan-store.sh").is_file():
        fail(f"{args.host} has no scripts/plan-store.sh", "pass --host the orchestrator checkout")
    args.spent = 0
    args.scratch = Path(os.environ.get("ONEPIPELINE_NODE_SCRATCH_DIR") or "/tmp")

    members, confirmed, extra = load_members(args.groom)
    merged = merged_survivors(args.groom)
    tickets = json.loads((args.groom / "tickets.json").read_text())
    TICKET_CORPUS.extend(
        Member(f"{t['repo']}#{t['number']}", "", t["title"], t["body"], parse_time(t["created"]), {}, "")
        for t in tickets
        if f"{t['repo']}#{t['number']}" not in members
    )
    clusters = confirmed + extra
    if args.cluster:
        clusters = [c for c in clusters if args.cluster in c["members"]]
        if not clusters:
            fail(f"{args.cluster} is in no confirmed cluster", "name a member as repo#number")
    titles = DraftTitles(args.host)
    results = []
    for cluster in clusters:
        result = replay(cluster, members, titles, args, merged)
        print_cluster(result, args.dry_run)
        results.append(result)
        # Written after every cluster, so a run the budget stops keeps what it measured.
        if args.json:
            args.json.write_text(json.dumps(results, indent=1))


if __name__ == "__main__":
    main()
