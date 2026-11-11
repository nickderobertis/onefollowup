#!/usr/bin/env bash
# Run an installed `onefollowup` as a user would and hold it to its version and its shipped
# defaults: `--version` prints the expected version, and `config show`, in an empty
# directory with no ONEFOLLOWUP_ variable, reports the effective configuration with the
# defaults the binary carries (README.md, "Configuration").
#
#   usage: smoke.sh <executable> <version> <label>
set -euo pipefail

[ "$#" -eq 3 ] || { echo "smoke: usage: smoke.sh <executable> <version> <label>" >&2; exit 2; }
executable="$1" version="$2" label="$3"

fail() {
  printf 'smoke: %s: %s\n' "$label" "$1" >&2
  printf 'smoke: next: %s\n' "$2" >&2
  exit 1
}

said="$("$executable" --version 2>&1)" || fail "--version failed: $said" "run $executable --version and read its error"
[ "$said" = "onefollowup $version" ] || fail "--version printed '$said', not 'onefollowup $version'" "install the package this tree built"

empty="$(mktemp -d)"
trap 'rm -rf "$empty"' EXIT
shown="$(cd "$empty" && env -u ONEFOLLOWUP_CONFIG "$executable" config show 2>&1)" \
  || fail "config show failed: $shown" "run $executable config show in an empty directory and read its error"
printf '%s' "$shown" | node -e '
  let input = "";
  process.stdin.on("data", (chunk) => (input += chunk)).on("end", () => {
    const { schema_version, config } = JSON.parse(input);
    const expected = {
      schema_version: [schema_version, 1],
      store: [config.store, null],
      drafts_source: [config.drafts_source, "drafts"],
      destination: [config.destination, "followups"],
      namespace: [config.namespace, "orchestrator"],
      default_kind: [config.default_kind, "root-cause"],
      kinds: [config.kinds.map((kind) => `${kind.name}:${kind.duplicates}`).join(","), "root-cause:agent"],
      dispositions: [config.dispositions.map((one) => one.name).join(","), "filed,not-reproducible,already-fixed,too-low-impact"],
      raise_at: [config.estimate.raise_at, 3],
      hooks: [JSON.stringify(config.hooks), "{}"],
    };
    const wrong = Object.entries(expected).filter(([, [got, want]]) => got !== want);
    if (wrong.length) {
      for (const [key, [got, want]] of wrong) console.error(`${key}: ${JSON.stringify(got)}, not ${JSON.stringify(want)}`);
      process.exit(1);
    }
  });
' || fail "config show does not report the shipped defaults (above)" "the installed binary is not this tree's; rebuild the package"
