"""Copy what prepare wrote into a measurement's recordings, public content only.

    python3 outputs.py KEPT STDOUT_IN STDOUT_OUT STDERR_IN STDERR_OUT

KEPT is the forwarder's file of the node ids it kept (every one an item whose repository
GitHub reports as public). Prepare's JSON answer is passed through `public.Filter`, which
drops every candidate whose repository is not public; its progress lines have every node id
not kept, and every issue URL of a repository that is not public, replaced by `"(dropped)"`.
Prints how many items it dropped.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import public


def main() -> None:
    kept, stdout_in, stdout_out, stderr_in, stderr_out = map(Path, sys.argv[1:6])
    gate = public.Filter()
    if kept.exists():
        gate.kept_ids = {line.strip() for line in kept.read_text().splitlines() if line.strip()}
    text = stdout_in.read_text() if stdout_in.exists() else ""
    try:
        answer = json.loads(text) if text.strip() else None
    except json.JSONDecodeError:
        answer = None
    if answer is not None:
        scrubbed = gate.scrub(answer)
        scrubbed = json.loads(gate.scrub_text(json.dumps(scrubbed)))
        stdout_out.write_text(json.dumps(scrubbed, indent=2) + "\n")
    else:
        stdout_out.write_text(gate.scrub_text(text))
    stderr = stderr_in.read_text() if stderr_in.exists() else ""
    stderr_out.write_text(gate.scrub_text(stderr))
    print(json.dumps({"items_dropped_as_not_public": gate.dropped}))


if __name__ == "__main__":
    main()
