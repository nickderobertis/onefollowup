#!/usr/bin/env bash
# Throwaway mock for spike-visual: re-captures every grooming screenshot into
# spikes/groom-visual/shots/ from the fixtures beside this script.
# Needs freeze, python3, node, and a node_modules holding playwright + marked
# (MOCK_NODE_MODULES, default the orchestrator checkout's).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out="$here/shots"
rm -rf "$out"; mkdir -p "$out"  # freeze stops to ask before overwriting
export MOCK_NODE_MODULES="${MOCK_NODE_MODULES:-/home/whoop/ai-orchestrator/node_modules}"
freeze="${FREEZE:-$(command -v freeze || echo /home/whoop/.local/bin/freeze)}"

python3 "$here/terminals.py" "$out"
for view in just-groom governor quota; do
    "$freeze" --language ansi --window --theme dracula --font.size 14 --padding 20 \
        --output "$out/groom-$view.png" "$out/$view.ansi" </dev/null >/dev/null
done
node "$here/render.mjs" "$here/fixtures" "$out"
rm -f "$out"/*.ansi "$out"/*.html
