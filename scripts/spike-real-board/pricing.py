"""Dry-run pricing: what GitHub says a document costs, from `rateLimit(dryRun:true)`, without
evaluating it (a document selecting a dry run anywhere is not evaluated at all)."""

from __future__ import annotations

import forwarder
import ghq

DRY = "dry:rateLimit(dryRun:true){cost nodeCount}"


def with_selection(document: str, selection: str) -> str:
    """`document` with `selection` first in its operation's top-level selection."""
    held = forwarder.inject(document)
    return held.replace(forwarder.INJECTED, selection, 1)


def dry_price(document: str, variables: dict, *, purpose: str) -> dict:
    """`{cost, nodeCount}` GitHub prices `document` with `variables` at."""
    answer = ghq.graphql(with_selection(document, DRY), variables, purpose=purpose, dry=True)
    return answer["data"]["dry"]


def injection_check(document: str, variables: dict, *, name: str) -> dict:
    """The dry price of `document` as onetaskgraph sends it and as the forwarder sends it."""
    plain = dry_price(document, variables, purpose=f"injection check: {name} as sent")
    injected = dry_price(
        forwarder.inject(document), variables, purpose=f"injection check: {name} with actualCost"
    )
    return {"document": name, "as_sent": plain, "with_actual_cost": injected, "same": plain == injected}
