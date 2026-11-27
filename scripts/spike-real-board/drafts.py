"""The synthetic drafts: each a public `followups` ticket's title and opening paragraph,
reworded by hand in `drafts.json` so it still names that ticket's distinctive terms and files.

    python3 drafts.py --check                          # validate against the board snapshot
    python3 drafts.py --record WORLD --binary BIN --batch NAME [--count N]

`--check` holds each draft to the recorded snapshot (`snapshot.py`): its ticket is on the
board, in a public repository, open exactly when `ticket_open` says, and every one of its
`distinctive` terms appears both in the ticket and in the draft; and exactly 3 drafts have an
open ticket. `--record` runs `onefollowup draft` for each in WORLD, whose environment the
caller gives, and prints the draft ids. Nothing here reads any drafts source but WORLD's own.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DRAFTS = json.loads((HERE / "drafts.json").read_text())


def check() -> list[str]:
    import snapshot

    board = {
        (one["repository"], one["number"]): one
        for one in snapshot.load()["items"]
        if one["type"] == "ISSUE" and not one.get("private")
    }
    problems = []
    for draft in DRAFTS:
        key = (draft["ticket"]["repository"], draft["ticket"]["number"])
        ticket = board.get(key)
        if ticket is None:
            problems.append(f"{key}: not a public ticket on the board")
            continue
        if (ticket["state"] == "OPEN") != draft["ticket_open"]:
            problems.append(f"{key}: is {ticket['state']}, the draft says open={draft['ticket_open']}")
        theirs = (ticket["title"] + "\n" + (ticket["body"] or "")).lower()
        ours = (draft["title"] + "\n" + draft["body"] + "\n" + " ".join(draft["paths"])).lower()
        for term in draft["distinctive"]:
            if term.lower() not in theirs:
                problems.append(f"{key}: the ticket does not name {term!r}")
            if term.lower() not in ours:
                problems.append(f"{key}: the draft does not name {term!r}")
    if sum(draft["ticket_open"] for draft in DRAFTS) != 3:
        problems.append("exactly 3 drafts must have a ticket open today")
    return problems


def record(world: Path, binary: str, batch: str, count: int) -> list[str]:
    ids = []
    for draft in DRAFTS[:count]:
        args = [
            binary, "draft", "--batch", batch, "--title", draft["title"],
            "--repository", "github.com/" + draft["ticket"]["repository"],
            "--author", "node:spike-real-board-cost", "--stamp", "session=spike",
        ]
        for path in draft["paths"]:
            args += ["--path", path]
        done = subprocess.run(
            args, cwd=world, input=draft["body"] + "\n", capture_output=True, text=True, env=dict(os.environ)
        )
        if done.returncode != 0:
            sys.exit(f"onefollowup draft failed: {done.stderr}")
        ids.append(json.loads(done.stdout)["draft"])
    return ids


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--record", type=Path)
    parser.add_argument("--binary")
    parser.add_argument("--batch")
    parser.add_argument("--count", type=int, default=len(DRAFTS))
    args = parser.parse_args()
    if args.check:
        problems = check()
        if problems:
            sys.exit("drafts.json does not hold to the board snapshot:\n" + "\n".join(problems))
    if args.record:
        print("\n".join(record(args.record, args.binary, args.batch, args.count)))


if __name__ == "__main__":
    main()
