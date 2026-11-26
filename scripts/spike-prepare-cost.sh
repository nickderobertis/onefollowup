#!/usr/bin/env bash
# spike-prepare-cost: one measurement of `onefollowup prepare` over the 50-draft loopback
# workload (crates/onefollowup-e2e/tests/e2e/prepare_cost.rs), on the release build.
#
#   scripts/spike-prepare-cost.sh base      # today's prepare, the loopback reporting a full allowance
#   scripts/spike-prepare-cost.sh bounded   # the spike's patch on, 2,000 of 5,000 points left
#
# Prints points a draft, total points, seconds and their breakdown by phase, and writes
# target/spike-prepare-cost/<design>.json; a bounded run compares candidates with a completed
# base run's. SPIKE_GOVERNOR=off switches the governor off (a base whose run the governor would
# hold, measured for candidates and the loopback's prices); SPIKE_RESET_SECONDS (default 1800)
# says how far the quota window's reset is from the start.
set -euo pipefail
design="${1:-base}"
case "$design" in base|bounded) ;; *) echo "usage: $0 base|bounded" >&2; exit 2 ;; esac
cd "$(dirname "$0")/.."
cargo build --quiet --release -p onefollowup
SPIKE_DESIGN="$design" SPIKE_GOVERNOR="${SPIKE_GOVERNOR:-on}" cargo test --quiet --release -p onefollowup-e2e --test e2e \
  prepare_cost::spike_prepare_cost -- --ignored --exact --nocapture
