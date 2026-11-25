#!/usr/bin/env bash
# SPIKE MOCK: re-capture every image of the followup-duplicate-search-cost visual spike.
# "before" is the unchanged base (BASE, default 04ce7b4), built from `git archive`; "after" is
# this tree, whose `prepare` prints the mocked progress lines. Both run `prepare` over copies
# of one world: a folder-of-Markdown destination of 30 items and a batch of 5 drafts.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
base="${BASE:-04ce7b4}"
freeze="${FREEZE:-$HOME/.local/bin/freeze}"
batch=nightly-2026-11-03
out="$here/shots"
mkdir -p "$out"

# The base binary, built out of tree.
src="$repo/target/spike-base-src"
rm -rf "$src" && mkdir -p "$src"
git -C "$repo" archive "$base" | tar -x -C "$src"
cargo build -q --manifest-path "$src/Cargo.toml" -p onefollowup --bin onefollowup \
  --target-dir "$repo/target/spike-base"
before_bin="$repo/target/spike-base/debug/onefollowup"
# The mock binary, from this tree.
cargo build -q --manifest-path "$repo/Cargo.toml" -p onefollowup --bin onefollowup
after_bin="$repo/target/debug/onefollowup"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
world="$work/world"
mkdir -p "$world"/{drafts-root,followups-root}/tasks "$world/home"
cat > "$world/onetaskgraph.yaml" <<YAML
sources:
  drafts:
    plugin: local-md
    config:
      root: drafts-root
  followups:
    plugin: local-md
    config:
      root: followups-root
YAML
printf 'host_label: build-host-1\n' > "$world/onefollowup.yaml"
titles=("the layout resets after a resize" "the palette resets after a resize" \
  "the toolbar resets after a resize" "gizmo calibration drifts after a restart" \
  "the session token is printed in the request log")
for at in $(seq 0 29); do
  t="gizmos: unrelated item $at"
  [ "$at" = 3 ] && t="widgets: the layout resets after a resize"
  [ "$at" = 11 ] && t="gizmos: calibration drifts after a restart"
  printf -- '---\ntitle: "%s"\nstatus: backlog\npriority: low\n---\nAn unrelated item, number %s, about gizmo calibration.\n' \
    "$t" "$at" > "$world/followups-root/tasks/other-$(printf %03d "$at").md"
done
run() { # binary, directory, args...
  local bin="$1" dir="$2"; shift 2
  (cd "$dir" && env -i HOME="$dir/home" PATH=/usr/bin:/bin "$bin" "$@")
}
for title in "${titles[@]}"; do
  echo "A follow-up: $title." | run "$after_bin" "$world" draft --batch "$batch" --title "$title" \
    --repository github.com/example/widgets --author node:worker-1 --stamp session=s-1 \
    --path src/widgets.rs > /dev/null
done

shoot() { # name, binary
  local name="$1" bin="$2" dir="$work/$1"
  cp -a "$world" "$dir"
  local cmd="onefollowup prepare --batch $batch"
  {
    printf '$ %s\n' "$cmd"
    run "$bin" "$dir" prepare --batch "$batch" 2>&1 >"$work/$name.stdout"
    # stdout is ~250 lines of JSON; the image keeps its head and tail and marks the cut.
    local n; n=$(wc -l < "$work/$name.stdout")
    head -n 8 "$work/$name.stdout"
    printf '    … %s lines of duplicate_searches and store_operations cut from this image …\n' "$((n - 20))"
    tail -n 12 "$work/$name.stdout"
  } | sed "s#$dir#/home/you/widgets#g" | fold -s -w 110 > "$work/$name.txt"
  "$freeze" "$work/$name.txt" -l text --window -o "$out/prepare-$name.png" >/dev/null
}
shoot before "$before_bin"
shoot after "$after_bin"
