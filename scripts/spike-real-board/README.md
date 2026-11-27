# Real-board cost spike (throwaway)

Measures what `onefollowup prepare` 0.14.0's duplicate search costs on the real `followups`
board (nickderobertis project 3). Nothing here lands; it is evidence for the report
`followup-prepare-real-board-cost-2026-11-27-spike-real-board-cost-report`.

Every recorded file holds content only from issues whose repository GitHub reports as public:
`public.py` drops every other item before anything is written and keeps only the count
dropped. `python3 check_public.py --fresh` scans every file here and fails otherwise.

| Command | What it does |
| --- | --- |
| `scripts/spike-real-board/measure.sh RUN [DRAFTS]` | one measurement: builds 0.14.0, records `onefollowup draft` of the synthetic drafts, runs `prepare` through `forwarder.py` into `recordings/RUN/`, summarizes it (`analyze.py`) and checks it (`check_public.py`) |
| `python3 snapshot.py` | the board's public issues and their shape, `recordings/board-snapshot.json.gz`, `board-summary.json` |
| `python3 price.py candidates\|hits\|latency recordings/RUN` | dry-run prices of today's documents and the candidates, hits per query, one timed request per candidate shape |
| `SPIKE_RUN=$PWD/recordings/RUN cargo test -p onefollowup-e2e --test e2e real_board_prices -- --ignored` | the loopback's price of every recorded request, `loopback-prices.jsonl` |
| `python3 attribute.py` | every report figure, offline, `recordings/attribution.json` |
| `python3 -m unittest test_public` | the public filter, offline |

A run's `requests.jsonl` holds one line a request: document name, variables (non-public ids
read `(dropped)`), GitHub's reported `cost`, `remaining`/`reset` of the window that answered,
`fall` since that window's previous reading, rows, bytes and seconds. `documents/` holds each
distinct document; `responses.jsonl.gz` each answer by `seq`. Lines whose document starts
`forwarder:` are the spike's own requests in the interval; `REST /rate_limit` lines are free
readings (from `ten-drafts-governed-2` on, with the `core` window their headers report).

The token is `GH_PROJECTS_TOKEN` from the orchestrator checkout's `.env` (manager ruling a0),
read by the Python processes alone; the ledger (`recordings/ledger.jsonl`) caps the spike at
1,500 GitHub-reported points and the forwarder refuses any document holding `mutation`.
