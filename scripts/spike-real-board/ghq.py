"""The one way this spike reaches GitHub: read-only GraphQL with the projects token, every
request and its GitHub-reported cost appended to the spend ledger, and a cap on the total.

The token is read from the env file (`SPIKE_ENV_FILE`, default the orchestrator checkout's
`.env`) at the moment a request is sent, kept in this process's memory only, and never
printed, logged or written: recorded requests carry no headers of ours at all.

Every document is refused before it is sent when it holds `mutation`, and when the ledger's
reported total has reached the cap less the margin a single request could add.
"""

from __future__ import annotations

import http.client
import json
import os
import re
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
RECORDINGS = Path(os.environ.get("SPIKE_RECORDINGS", HERE / "recordings"))
LEDGER = RECORDINGS / "ledger.jsonl"
ENV_FILE = Path(os.environ.get("SPIKE_ENV_FILE", Path.home() / "ai-orchestrator" / ".env"))
TOKEN_NAME = "GH_PROJECTS_TOKEN"
CAP = float(os.environ.get("SPIKE_CAP", "1500"))
# The most one request this spike sends is expected to cost; the cap refuses a request once
# the total is within this of it, so a run stops before the cap rather than past it.
MARGIN = float(os.environ.get("SPIKE_MARGIN", "40"))
HOST = "api.github.com"

_lock = threading.Lock()
_local = threading.local()
# Called with every request this module sends, so a recorder can count the spike's own
# requests in the shared allowance's fall beside prepare's.
OBSERVER = None


class Refused(Exception):
    """A request this spike will not send."""


def _token() -> str:
    for line in ENV_FILE.read_text().splitlines():
        if line.startswith(TOKEN_NAME + "="):
            value = line.split("=", 1)[1].strip().strip('"').strip("'")
            if value:
                return value
    raise Refused(f"{TOKEN_NAME} is not set in {ENV_FILE}")


def refuse_writes(document: str) -> None:
    if re.search(r"\bmutation\b", document):
        raise Refused("a document holding `mutation` is never sent: this spike only reads")


def spent() -> float:
    """The ledger's GitHub-reported total, in points."""
    if not LEDGER.exists():
        return 0.0
    total = 0.0
    for line in LEDGER.read_text().splitlines():
        if line.strip():
            total += float(json.loads(line).get("charged", 0) or 0)
    return total


def check_cap() -> None:
    total = spent()
    if total + MARGIN > CAP:
        raise Refused(
            f"the spike has spent {total:.0f} reported points; another request could take it past {CAP:.0f}"
        )


def ledger(entry: dict) -> None:
    RECORDINGS.mkdir(parents=True, exist_ok=True)
    with _lock, LEDGER.open("a") as out:
        out.write(json.dumps(entry, sort_keys=True) + "\n")


def _connection() -> http.client.HTTPSConnection:
    held = getattr(_local, "connection", None)
    if held is None:
        held = http.client.HTTPSConnection(HOST, timeout=120)
        _local.connection = held
    return held


def send(method: str, path: str, body: bytes | None) -> tuple[int, dict, bytes, float]:
    """One request to api.github.com with the token, on this thread's kept connection:
    `(status, lower-cased headers, body, seconds)`."""
    headers = {
        "Authorization": "bearer " + _token(),
        "User-Agent": "onefollowup-real-board-spike",
        "Accept": "application/json",
    }
    if body is not None:
        headers["Content-Type"] = "application/json"
    for attempt in range(2):
        connection = _connection()
        started = time.monotonic()
        try:
            connection.request(method, path, body=body, headers=headers)
            response = connection.getresponse()
            data = response.read()
        except (http.client.HTTPException, OSError):
            connection.close()
            _local.connection = None
            if attempt:
                raise
            continue
        elapsed = time.monotonic() - started
        return (
            response.status,
            {k.lower(): v for k, v in response.getheaders()},
            data,
            elapsed,
        )
    raise RuntimeError("unreachable")


def graphql(
    document: str,
    variables: dict | None = None,
    *,
    purpose: str,
    dry: bool = False,
    tolerate_errors: bool = False,
) -> dict:
    """Send one read-only document, appending it with its reported cost to the ledger.

    The document must select `actualCost:rateLimit{cost}` (or `rateLimit(dryRun:true)`) for
    its cost to be read; the ledger records what the answer reports, else 1 as a bound."""
    refuse_writes(document)
    check_cap()
    body = json.dumps({"query": document, "variables": variables or {}}).encode()
    status, headers, data, elapsed = send("POST", "/graphql", body)
    answer = json.loads(data or b"{}")
    charged = None if dry else reported_cost(answer)
    price = dry_price(answer) if dry else None
    entry = {
        "dry_run_price": price,
        "at": time.time(),
        "purpose": purpose,
        "status": status,
        "charged": charged if charged is not None else 1,
        "cost_reported": charged is not None,
        "remaining": headers.get("x-ratelimit-remaining"),
        "reset": headers.get("x-ratelimit-reset"),
        "request_bytes": len(body),
        "response_bytes": len(data),
        "seconds": round(elapsed, 3),
        "resource": headers.get("x-ratelimit-resource"),
    }
    ledger(entry)
    if OBSERVER is not None:
        OBSERVER(entry)
    if status != 200 or (answer.get("errors") and not tolerate_errors):
        raise RuntimeError(f"GitHub answered {status}: {json.dumps(answer.get('errors'))[:800]}")
    return answer


def dry_price(answer: dict) -> float | None:
    """What a dry run prices its document at. A document holding `rateLimit(dryRun:true)` is
    not evaluated at all, and every `rateLimit` it selects reports that price, so its own
    charge cannot be read: the ledger bounds it at 1, the least any query costs."""
    held = (answer.get("data") or {}).get("dry")
    return float(held["cost"]) if isinstance(held, dict) and "cost" in held else None


def reported_cost(answer: dict) -> float | None:
    """What a non-dry-run answer says the request cost: `actualCost` or a top-level
    `rateLimit` that was not a dry run. A dry run's own price is not what it was charged."""
    data = answer.get("data") or {}
    for key in ("actualCost", "rateLimit"):
        held = data.get(key)
        if isinstance(held, dict) and "cost" in held and key == "actualCost":
            return float(held["cost"])
    return None
