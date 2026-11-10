#!/usr/bin/env bash
# One phase of one job of .github/workflows/release.yml:
#
#   usage: release.sh <job> <phase> --tag v<version>
#
# What is published (release-targets.toml): the `onefollowup` crate on crates.io; the
# `onefollowup-cli` wheels on PyPI, one per platform; the `onefollowup-cli-<platform>` npm
# carriers, one per platform, and the `onefollowup-cli` launcher that resolves them.
#
#   build-<platform> build    build the binary, its wheel, its npm carrier and the launcher
#                             for the platform's target, install each and run it
#                             (scripts/package-smoke.sh --profile release)
#   build-<platform> publish  upload the wheel to PyPI and the carrier to npm, each
#                             skipped where the registry already serves it
#   build-<platform> verify   wait until PyPI serves the version, then install the wheel
#                             back from PyPI and run it
#   publish available         confirm npm serves every platform's carrier and PyPI the
#                             version, so nothing platform-independent ships ahead of them
#   publish build             package the crate, assemble the launcher
#   publish publish           publish the crate and then the launcher, each skipped where
#                             already served
#   publish verify            wait until crates.io and npm serve them, then install the
#                             launcher back from npm and run it
#
# Idempotent: a re-run after a partial failure skips what is already published. Quiet on
# success; a failure names the step and what to do.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
job="${1:-}"
phase="${2:-}"
shift 2 || true
tag=""
while [ $# -gt 0 ]; do
  case "$1" in
    --tag) tag="${2:-}"; shift 2 ;;
    *) printf 'release: unknown argument %s\n' "$1" >&2; exit 2 ;;
  esac
done

fail() {
  printf 'release: %s\n' "$1" >&2
  printf 'release: next: %s\n' "$2" >&2
  exit 1
}

[[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?)$ ]] \
  || fail "'--tag ${tag}' is not a release tag v<semver>" "pass the tag the GitHub Release names"
version="${BASH_REMATCH[1]}"
manifest_version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml)"
[ "$manifest_version" = "$version" ] || fail \
  "the tag says $version but Cargo.toml's [workspace.package] says $manifest_version" \
  "release only from the commit release-plz tagged"

out="$ROOT/target/release-artifacts"
served() { bash "$ROOT/scripts/release-probe.sh" "$1"; }

# Wait up to ten minutes for `$1` to be served at this version.
await() {
  local current=""
  for _ in $(seq 1 30); do
    current="$(served "$1" || true)"
    [ "$current" = "$version" ] && return 0
    sleep 20
  done
  fail "$1 is not served at $version after ten minutes (it serves '${current}')" \
    "check the registry's page for it, then re-run the job"
}

platform_of() {
  case "$1" in
    build-linux) echo "x86_64-unknown-linux-gnu linux-x64" ;;
    build-linux-arm64) echo "aarch64-unknown-linux-gnu linux-arm64" ;;
    build-macos) echo "aarch64-apple-darwin darwin-arm64" ;;
    build-windows) echo "x86_64-pc-windows-msvc win32-x64" ;;
    *) return 1 ;;
  esac
}

case "$job:$phase" in
  build-*:build)
    read -r target _ < <(platform_of "$job") || fail "unknown job $job" "use a job of release.yml"
    bash scripts/package-smoke.sh --profile release --target "$target" --out "$out"
    ;;
  build-*:publish)
    read -r _ platform < <(platform_of "$job") || fail "unknown job $job" "use a job of release.yml"
    [ -n "${PYPI_TOKEN:-}" ] || fail "PYPI_TOKEN is not set" "add the PYPI_TOKEN repository secret, then re-run the job"
    # --check-url skips a file PyPI already holds, so a re-run publishes only what is missing.
    uv publish --quiet --token "$PYPI_TOKEN" --check-url https://pypi.org/simple/ "$out"/wheels/*.whl \
      || fail "PyPI refused the $platform wheel" "read uv's error above; a re-run skips files already uploaded"
    bash scripts/publish-npm.sh "$out/onefollowup-cli-$platform-$version.tgz"
    ;;
  build-*:verify)
    await pypi:onefollowup-cli
    work="$(mktemp -d)"
    uv venv --quiet "$work/venv"
    # PyPI's simple index can lag its JSON API: retry the install briefly before failing.
    for attempt in 1 2 3 4 5 6; do
      uv pip install --quiet --refresh --python "$work/venv" "onefollowup-cli==$version" && break
      [ "$attempt" -lt 6 ] || fail "onefollowup-cli $version did not install from PyPI" "re-run the job"
      sleep 20
    done
    bin="$work/venv/bin/onefollowup"
    [ -x "$bin" ] || bin="$work/venv/Scripts/onefollowup.exe"
    bash scripts/smoke.sh "$bin" "$version" "the wheel from PyPI"
    ;;
  publish:available)
    await pypi:onefollowup-cli
    for platform in linux-x64 linux-arm64 darwin-arm64 win32-x64; do
      await "npm:onefollowup-cli-$platform"
    done
    ;;
  publish:build)
    cargo package -p onefollowup --locked --quiet || fail "the crate does not package" "fix what cargo package reports above"
    launcher="$(node scripts/npm-build.mjs launcher --out "$out/npm")"
    npm pack --silent --pack-destination "$out" "$launcher" >/dev/null
    ;;
  publish:publish)
    if [ "$(served crate:onefollowup || true)" != "$version" ]; then
      [ -n "${CARGO_REGISTRY_TOKEN:-}" ] || fail "CARGO_REGISTRY_TOKEN is not set" \
        "add the CARGO_REGISTRY_TOKEN repository secret, then re-run the job"
      cargo publish -p onefollowup --locked --quiet \
        || fail "cargo publish failed for $version" "read cargo's error above; a re-run skips a version already published"
    fi
    bash scripts/publish-npm.sh "$out/onefollowup-cli-$version.tgz"
    ;;
  publish:verify)
    await crate:onefollowup
    await npm:onefollowup-cli
    work="$(mktemp -d)"
    for attempt in 1 2 3 4 5 6; do
      npm install --silent --prefix "$work" --no-audit --no-fund "onefollowup-cli@$version" && break
      [ "$attempt" -lt 6 ] || fail "onefollowup-cli@$version did not install from npm" "re-run the job"
      sleep 20
    done
    bash scripts/smoke.sh "$work/node_modules/.bin/onefollowup" "$version" "the npm launcher from npm"
    ;;
  *)
    fail "unknown job and phase '$job $phase'" \
      "use a job of release.yml (build-linux, build-linux-arm64, build-macos, build-windows, publish) and build, publish, verify or available"
    ;;
esac
