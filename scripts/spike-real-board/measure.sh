#!/usr/bin/env bash
# One measurement: `onefollowup prepare`, built as 0.14.0 from this tree, on the synthetic
# drafts, against the real `followups` board (nickderobertis project 3) through the recording
# forwarder; then the summary of what it recorded.
#
#   scripts/spike-real-board/measure.sh RUN [DRAFTS]
#
# RUN names `scripts/spike-real-board/recordings/RUN/`, where the forwarder's requests, the
# documents, the answers, prepare's stdout and stderr and the governor's state afterwards are
# kept, public content only: the forwarder and `outputs.py` drop every item whose repository
# GitHub does not report as public before writing, and `check_public.py` then scans the run. DRAFTS (default 10) is how many of `drafts.json` to record. The governor starts from a
# fresh host state of its own (`SPIKE_GOVERNOR_STATE=FILE` starts it from a copy of FILE
# instead, and `SPIKE_GOVERNOR_ENABLED=false` sends every request at once, the host's own
# floor and peak window otherwise); nothing of the host's own state, drafts or configuration is read or written.
#
# The projects token is read by the spike's own Python processes alone, from SPIKE_ENV_FILE
# (default the orchestrator checkout's .env), never printed or written; prepare is given a
# placeholder. SPIKE_TIMEOUT (seconds, default
# none) stops prepare there, its exit then 124. The forwarder refuses every
# `mutation` and stops the run before the spike's ledger would pass SPIKE_CAP (1500) points.
set -euo pipefail

run=${1:?usage: measure.sh RUN [DRAFTS]}
count=${2:-10}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out="$here/recordings/$run"
if [ -e "$out/requests.jsonl" ]; then
  echo "measure.sh: $out already holds a run; name a new RUN" >&2
  exit 1
fi
binary="$root/target/release/onefollowup"
cargo build --quiet --release -p onefollowup --bin onefollowup --manifest-path "$root/Cargo.toml"
version=$("$binary" --version)
case "$version" in
  *0.14.0*) ;;
  *) echo "measure.sh: built $version, not 0.14.0; run from the tree the spike started from" >&2; exit 1 ;;
esac
python3 "$here/drafts.py" --check

world=$(mktemp -d "${ONEPIPELINE_NODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/real-board-world.XXXXXX")
mkdir -p "$world/home/config" "$world/home/cache" "$world/state" "$world/drafts-root/tasks" "$out"
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')

cat > "$world/onetaskgraph.yaml" <<YAML
sources:
  drafts:
    plugin: local-md
    config:
      root: drafts-root
  followups:
    plugin: github-projects
    config:
      owner: nickderobertis
      project_number: 3
      repository: nickderobertis/ai-orchestrator
      endpoint: http://127.0.0.1:$port/graphql
      token_env: SPIKE_FORWARDER_TOKEN
      status_mapping:
        backlog: Proposal
        draft: Deferred
        queued: Queued
        done: Done
        cancelled: Cancelled
YAML
cat > "$world/onefollowup.yaml" <<YAML
destination: followups
drafts_source: drafts
host_label: spike-real-board
governor:
  enabled: ${SPIKE_GOVERNOR_ENABLED:-true}
  floor_percent: 20
  peak_window_seconds: 3600
YAML
if [ -n "${SPIKE_GOVERNOR_STATE:-}" ]; then
  mkdir -p "$world/state/onefollowup/governor"
  cp "$SPIKE_GOVERNOR_STATE" "$world/state/onefollowup/governor/github-projects.json"
fi

isolated=(env -i "HOME=$world/home" "XDG_CONFIG_HOME=$world/home/config"
  "XDG_CACHE_HOME=$world/home/cache" "XDG_STATE_HOME=$world/state" "PATH=/usr/bin:/bin"
  "DBUS_SESSION_BUS_ADDRESS=disabled:" "SPIKE_FORWARDER_TOKEN=placeholder-the-forwarder-replaces")

python3 "$here/forwarder.py" --port "$port" --run "$run" --kept "$world/kept-ids.txt" \
  2> "$world/forwarder.stderr" &
forwarder=$!
trap 'kill "$forwarder" 2>/dev/null || true' EXIT INT TERM
for _ in $(seq 300); do
  python3 -c "import socket; socket.create_connection(('127.0.0.1',$port),0.2)" 2>/dev/null && break
  sleep 0.1
done
# The shared allowance's level as the run starts and ends, read free, so its fall over the
# whole interval is known.
rate_limit() { curl -fsS "http://127.0.0.1:$port/rate_limit" > /dev/null; }
rate_limit

batch="spike-$run"
(cd "$world" && "${isolated[@]}" python3 "$here/drafts.py" --record "$world" --binary "$binary" \
  --batch "$batch" --count "$count") > "$out/drafts.txt"

started=$(date +%s.%N)
set +e
(cd "$world" && timeout --signal=TERM "${SPIKE_TIMEOUT:-0}" "${isolated[@]}" "$binary" prepare --batch "$batch") \
  > "$world/prepare.stdout.json" 2> "$world/prepare.stderr"
status=$?
set -e
ended=$(date +%s.%N)
rate_limit
echo "{\"exit\": $status, \"seconds\": $(python3 -c "print(round($ended-$started,3))"), \"drafts\": $count, \"version\": \"$version\", \"governor_enabled\": ${SPIKE_GOVERNOR_ENABLED:-true}}" > "$out/run.json"
cp "$world/state/onefollowup/governor/github-projects.json" "$out/governor-state.json" 2>/dev/null || true
kill "$forwarder" 2>/dev/null || true
wait "$forwarder" 2>/dev/null || true
trap - EXIT INT TERM
python3 "$here/outputs.py" "$world/kept-ids.txt" "$world/prepare.stdout.json" \
  "$out/prepare.stdout.json" "$world/prepare.stderr" "$out/prepare.stderr" > "$out/outputs-dropped.json"
if [ -s "$world/forwarder.stderr" ]; then
  echo "measure.sh: the forwarder reported errors (kept out of the recordings): $world/forwarder.stderr" >&2
else
  rm -rf "$world"
fi
python3 "$here/analyze.py" "$out"
python3 "$here/check_public.py" "$out"
exit "$status"
