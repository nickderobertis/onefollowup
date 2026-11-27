"""A recording forwarder between `onefollowup prepare` and https://api.github.com.

    python3 forwarder.py --port PORT --run NAME

A destination source whose `endpoint` is `http://127.0.0.1:PORT/graphql` reaches GitHub through
it. It replaces whatever authorization the client sent with the projects token (`ghq`), so the
client is given a placeholder token and never the real one. For every GraphQL request it:

- refuses a document holding `mutation`, and any request once the spike's reported total is
  within `ghq.MARGIN` of `ghq.CAP` (answered 503 with a GraphQL error naming why);
- adds `actualCost:rateLimit(dryRun:false){cost}` to the operation's top-level selection, and
  strips it from the answer the client sees;
- records the document's name, the variables, GitHub's reported cost, the fall of
  `x-ratelimit-remaining` since the previous response of the same window, rows answered,
  request and response bytes and elapsed time, to `recordings/<run>/requests.jsonl`; each
  distinct document once to `recordings/<run>/documents/<sha>.graphql`; and each answer
  to `recordings/<run>/responses.jsonl.gz`, verbatim but for `public.Filter`: every item whose
  repository GitHub does not report as public is dropped before anything is written, every
  recorded variable naming a dropped item's id reads `"(dropped)"`, and only the count of
  items dropped is kept (`recordings/<run>/dropped.json`).

`GET /rate_limit` is forwarded as it is and recorded (GitHub charges nothing for it).
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import re
import signal
import threading
import time
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import ghq
import public

INJECTED = "actualCost:rateLimit(dryRun:false){cost}"
PASSED_HEADERS = (
    "content-type",
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "x-ratelimit-reset",
    "x-ratelimit-used",
    "x-ratelimit-resource",
    "retry-after",
    "x-github-request-id",
)


def name_of(document: str) -> str:
    """The name onetaskgraph 0.17.0's `graphql` module gives `document`."""
    if re.search(r"\bs0:search\(", document):
        return "SEARCH_BATCH"
    if document.startswith("query($search:String!"):
        return "SEARCH_ISSUES"
    if "$id23:ID!" in document:
        return "ISSUE_DETAILS" if "$comments:Boolean!" in document else "ISSUE_RELATIONS"
    if document.startswith("query($id:ID!,$first:Int!,$after:String"):
        return "ISSUE_DETAIL"
    if document.startswith("query($id:ID!"):
        return "ISSUE"
    return "other:" + hashlib.sha256(document.encode()).hexdigest()[:12]


def inject(document: str) -> str:
    """`document` with [`INJECTED`] first in its operation's top-level selection."""
    at = document.index("query") + len("query")
    while document[at].isspace() or document[at].isalnum() or document[at] == "_":
        at += 1
    if document[at] == "(":
        depth = 0
        while True:
            if document[at] == "(":
                depth += 1
            elif document[at] == ")":
                depth -= 1
                if depth == 0:
                    break
            at += 1
    brace = document.index("{", at)
    return document[: brace + 1] + INJECTED + " " + document[brace + 1 :]


def rows_of(data: dict | None) -> int:
    """Issue rows an answer holds: each top-level connection's nodes, each top-level node."""
    rows = 0
    for key, value in (data or {}).items():
        if not isinstance(value, dict):
            continue
        if isinstance(value.get("nodes"), list):
            rows += len(value["nodes"])
        elif "search" in value and isinstance(value["search"], dict):
            rows += len(value["search"].get("nodes") or [])
        elif value.get("__typename"):
            rows += 1
    return rows


class Recorder:
    def __init__(self, run: str, kept_file: Path | None = None):
        self.kept_file = kept_file
        self.directory = ghq.RECORDINGS / run
        (self.directory / "documents").mkdir(parents=True, exist_ok=True)
        self.requests = (self.directory / "requests.jsonl").open("a")
        self.responses = gzip.open(self.directory / "responses.jsonl.gz", "at")
        # Every visibility answer the run needs is read before it starts, and none during it,
        # so the shared allowance's fall over the run holds no request of the recorder's own:
        # a repository the owner's walk did not answer for, and a node id mentioned in a kept
        # item's text that is not itself a kept item, are dropped unasked.
        visibility = public.Visibility()
        visibility.preload(public.BOARD_OWNER)
        visibility.offline = True
        self.filter = public.Filter(
            visibility,
            board_project_id=public.board_project_id(),
            resolve_nodes=lambda ids, board, purpose: set(),
        )
        self.lock = threading.RLock()
        self.sequence = 0
        self.last_remaining: dict[str, int] = {}
        self.started = time.time()
        self.confirm = True
        self.checked: set[str] = set()

    def observe(self, sent: dict) -> None:
        """The spike's own request (a visibility or dry-run check), counted in the fall."""
        remaining = sent.get("remaining")
        self.record(
            {
                "document": "forwarder:" + sent["purpose"],
                "status": sent["status"],
                "cost": sent["charged"] if sent.get("cost_reported") else None,
                "dry_run": sent.get("dry_run_price") is not None or not sent.get("cost_reported"),
                "remaining": int(remaining) if remaining is not None else None,
                "reset": sent.get("reset"),
                "resource": sent.get("resource"),
                "request_bytes": sent.get("request_bytes"),
                "response_bytes": sent.get("response_bytes"),
                "seconds": sent.get("seconds"),
            },
            None,
        )

    def write_dropped(self) -> None:
        (self.directory / "dropped.json").write_text(
            json.dumps({"items_dropped_as_not_public": self.filter.dropped}) + "\n"
        )
        if self.kept_file is not None:
            self.kept_file.write_text("".join(one + "\n" for one in sorted(self.filter.kept_ids)))

    def record(self, entry: dict, answer: dict | None) -> int:
        with self.lock:
            self.sequence += 1
            # Held locally: a check this record triggers records itself, and takes a later number.
            seq = self.sequence
            entry["seq"] = seq
            entry["t"] = round(time.time() - self.started, 3)
            remaining, reset = entry.get("remaining"), entry.get("reset")
            if remaining is not None and reset is not None and entry.get("resource") == "graphql":
                before = self.last_remaining.get(reset)
                entry["fall"] = None if before is None else before - remaining
                self.last_remaining[reset] = remaining
            scrubbed = self.filter.scrub(answer) if answer is not None else None
            if isinstance(entry.get("variables"), dict):
                entry["variables"] = self.filter.scrub_variables(entry["variables"])
            if entry.get("errors"):
                entry["errors"] = self.filter.scrub(entry["errors"])
            self.requests.write(json.dumps(entry, sort_keys=True) + "\n")
            self.requests.flush()
            if scrubbed is not None:
                self.responses.write(json.dumps({"seq": seq, "answer": scrubbed}) + "\n")
                self.responses.flush()
            self.write_dropped()
            return seq

    def document(self, text: str) -> str:
        sha = hashlib.sha256(text.encode()).hexdigest()[:16]
        path = self.directory / "documents" / f"{sha}.graphql"
        if not path.exists():
            path.write_text(text)
        return sha


def handler_for(recorder: Recorder):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def reply(self, status: int, headers: dict, body: bytes) -> None:
            self.send_response(status)
            for name in PASSED_HEADERS:
                if name in headers:
                    self.send_header(name, headers[name])
            if "content-type" not in headers:
                self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if not self.path.startswith("/rate_limit"):
                self.reply(404, {}, b'{"message":"the forwarder serves /graphql and /rate_limit"}')
                return
            status, headers, body, elapsed = ghq.send("GET", "/rate_limit", None)
            window = (
                (json.loads(body).get("resources") or {}).get("graphql") or {}
                if status == 200
                else {}
            )
            recorder.record(
                {
                    "document": "REST /rate_limit",
                    "status": status,
                    "cost": 0,
                    "remaining": window.get("remaining"),
                    "reset": str(window["reset"]) if "reset" in window else None,
                    "resource": "graphql" if window else None,
                    "seconds": round(elapsed, 3),
                    "response_bytes": len(body),
                },
                None,
            )
            self.reply(status, headers, body)

        def do_POST(self):
            length = int(self.headers.get("content-length") or 0)
            raw = self.rfile.read(length)
            request = json.loads(raw)
            document = request.get("query", "")
            variables = request.get("variables") or {}
            name = name_of(document)
            sha = recorder.document(document)
            try:
                ghq.refuse_writes(document)
                ghq.check_cap()
            except ghq.Refused as refused:
                recorder.record(
                    {"document": name, "sha": sha, "refused": str(refused), "variables": variables},
                    None,
                )
                body = json.dumps({"errors": [{"message": f"spike forwarder: {refused}"}]}).encode()
                self.reply(503, {}, body)
                return
            if recorder.confirm and name not in recorder.checked:
                recorder.checked.add(name)
                import pricing

                checked = pricing.injection_check(document, variables, name=name)
                with (recorder.directory / "injection-check.jsonl").open("a") as out:
                    out.write(json.dumps(checked, sort_keys=True) + "\n")
            forwarded = json.dumps({"query": inject(document), "variables": variables}).encode()
            status, headers, body, elapsed = ghq.send("POST", "/graphql", forwarded)
            try:
                answer = json.loads(body)
            except json.JSONDecodeError:
                answer = None
            cost = None
            if isinstance(answer, dict) and isinstance(answer.get("data"), dict):
                held = answer["data"].pop("actualCost", None)
                if isinstance(held, dict):
                    cost = held.get("cost")
                body = json.dumps(answer).encode()
            remaining = headers.get("x-ratelimit-remaining")
            entry = {
                "document": name,
                "sha": sha,
                "variables": variables,
                "status": status,
                "cost": cost,
                "remaining": int(remaining) if remaining is not None else None,
                "reset": headers.get("x-ratelimit-reset"),
                "used": headers.get("x-ratelimit-used"),
                "resource": headers.get("x-ratelimit-resource"),
                "rows": rows_of(answer.get("data")) if isinstance(answer, dict) else 0,
                "errors": (answer or {}).get("errors") if isinstance(answer, dict) else None,
                "request_bytes": len(raw),
                "forwarded_bytes": len(forwarded),
                "response_bytes": len(body),
                "seconds": round(elapsed, 3),
            }
            ghq.ledger(
                {
                    "at": time.time(),
                    "purpose": f"prepare via forwarder ({recorder.directory.name} {name})",
                    "status": status,
                    "charged": cost if cost is not None else 1,
                    "cost_reported": cost is not None,
                    "remaining": remaining,
                    "reset": headers.get("x-ratelimit-reset"),
                }
            )
            # Prepare has its answer before the recording's filtering runs, so the
            # recording costs prepare no time.
            self.reply(status, headers, body)
            recorder.record(entry, answer)

    return Handler


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--run", required=True)
    parser.add_argument("--kept", type=Path, help="where the node ids of kept items are written")
    args = parser.parse_args()
    recorder = Recorder(args.run, args.kept)
    ghq.OBSERVER = recorder.observe
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(recorder))

    def stop(*_):
        recorder.responses.close()
        recorder.requests.close()
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    server.serve_forever()


if __name__ == "__main__":
    main()
