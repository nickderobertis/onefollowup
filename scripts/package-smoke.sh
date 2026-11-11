#!/usr/bin/env bash
# Build the distributable packages for one target from this tree, install each the way a
# user does, and run the installed `onefollowup`:
#
#   usage: package-smoke.sh [--profile dev|release] [--target <triple>] [--out <dir>]
#
# - the `onefollowup-cli` wheel (maturin `bin` bindings), installed into a fresh virtual
#   environment, whose `onefollowup` must be on that environment's PATH;
# - the npm carrier for the target and the `onefollowup-cli` launcher, packed and installed
#   offline into an empty directory, whose `onefollowup` the launcher starts.
#
# Each installed executable must print this tree's version and report the effective
# configuration with the shipped defaults (scripts/smoke.sh). The defaults are the target
# the host is (`rustc -vV`) and the dev profile; release.sh builds with `--profile release`
# for each release target. What it built stays under --out (default: target/package-smoke).
# Quiet on success; a failure names the step and what to do.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail() {
  printf 'package-smoke: %s\n' "$1" >&2
  printf 'package-smoke: next: %s\n' "$2" >&2
  exit 1
}

profile=dev
target="$(rustc -vV | sed -n 's/^host: //p')"
out="$ROOT/target/package-smoke"
while [ $# -gt 0 ]; do
  case "$1" in
    --profile) profile="${2:-}"; shift 2 ;;
    --target) target="${2:-}"; shift 2 ;;
    --out) out="${2:-}"; shift 2 ;;
    *) fail "unknown argument $1" "pass --profile dev|release, --target <triple> or --out <dir>" ;;
  esac
done
case "$profile" in
  dev) built=debug; maturin_profile=() ;;
  release) built=release; maturin_profile=(--release) ;;
  *) fail "unknown profile $profile" "pass --profile dev or --profile release" ;;
esac
case "$target" in
  x86_64-unknown-linux-gnu) npm_os=linux; npm_cpu=x64; exe= ;;
  aarch64-unknown-linux-gnu) npm_os=linux; npm_cpu=arm64; exe= ;;
  aarch64-apple-darwin) npm_os=darwin; npm_cpu=arm64; exe= ;;
  x86_64-pc-windows-msvc) npm_os=win32; npm_cpu=x64; exe=.exe ;;
  *) fail "$target is not a release target" "build on one of the four release platforms (release-targets.toml)" ;;
esac
for tool in cargo uv node npm; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool is not on PATH" "install it; release.yml's build jobs and ci.yml's test jobs set it up"
done
pin() { sed -n "s/^$1=//p" "$ROOT/.github/ci-tool-versions"; }
maturin_version="$(pin maturin)"
[ -n "$maturin_version" ] || fail "no maturin pin" "add maturin=<version> to .github/ci-tool-versions"
version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml)"

log="$(mktemp)"
trap 'rm -f "$log"' EXIT
quietly() {
  local what="$1"; shift
  "$@" >"$log" 2>&1 || { cat "$log" >&2; fail "$what failed (above)" "fix what it reports, then re-run"; }
}

rm -rf "$out"
mkdir -p "$out/wheels" "$out/npm"

quietly "the binary's build" cargo build --profile "$profile" --locked -p onefollowup --target "$target"
binary="$ROOT/target/$target/$built/onefollowup$exe"
quietly "the wheel's build" uvx --from "maturin==$maturin_version" maturin build "${maturin_profile[@]}" \
  --locked --target "$target" --out "$out/wheels"
shopt -s nullglob
wheels=("$out"/wheels/onefollowup_cli-"$version"-*.whl)
[ "${#wheels[@]}" -eq 1 ] || fail "expected one onefollowup_cli-$version wheel in $out/wheels" "read maturin's output"

carrier="$(node scripts/npm-build.mjs platform --target "$target" --binary "$binary" --out "$out/npm")"
launcher="$(node scripts/npm-build.mjs launcher --out "$out/npm")"
quietly "packing the carrier" npm pack --silent --pack-destination "$out" "$carrier"
quietly "packing the launcher" npm pack --silent --pack-destination "$out" "$launcher"

# The wheel, into a fresh virtual environment.
venv="$out/venv"
quietly "creating the virtual environment" uv venv --quiet "$venv"
quietly "installing the wheel" uv pip install --quiet --python "$venv" "${wheels[0]}"
case "$npm_os" in win32) venv_bin="$venv/Scripts" ;; *) venv_bin="$venv/bin" ;; esac
[ -x "$venv_bin/onefollowup$exe" ] || fail "the wheel put no onefollowup on the environment's PATH" "check pyproject.toml's [tool.maturin] bindings"
bash scripts/smoke.sh "$venv_bin/onefollowup$exe" "$version" "the wheel"

# The npm carrier and launcher, offline, into an empty directory.
prefix="$out/npm-install"
mkdir -p "$prefix"
quietly "installing the carrier" npm install --silent --prefix "$prefix" --offline --no-audit --no-fund \
  --os="$npm_os" --cpu="$npm_cpu" "$out/onefollowup-cli-$npm_os-$npm_cpu-$version.tgz"
quietly "installing the launcher" npm install --silent --prefix "$prefix" --offline --no-audit --no-fund \
  --omit=optional --os="$npm_os" --cpu="$npm_cpu" "$out/onefollowup-cli-$version.tgz"
case "$npm_os" in win32) launched="$prefix/node_modules/.bin/onefollowup.cmd" ;; *) launched="$prefix/node_modules/.bin/onefollowup" ;; esac
bash scripts/smoke.sh "$launched" "$version" "the npm launcher"
