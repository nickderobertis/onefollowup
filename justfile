# Canonical command surface for onefollowup.
#
# `just bootstrap` works from a clean clone; `just check` is the full deterministic gate.
# The repo-wide verbs delegate to Nx, which fans each uniformly-named target out over the
# projects that declare it (crates/onefollowup, crates/onefollowup-e2e, packaging, the
# workspace); what a target does stays in that project's project.json. Recipes are quiet
# on success and specific on failure.

# llmlint: ignore-file[tool_output_is_signal] recipes that hand straight to cargo, clippy, cargo-deny, bun or Nx inherit those tools' diagnostics, which already name the exact problem and its fix; a wrapper message would bury them. The recipes whose failure needs project context (bootstrap, the lint-llm tier) add one explicitly.

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Keep the gate's own output to signal: successes are silent, failures are not.
export CARGO_TERM_QUIET := "true"

# The MSRV has one source of truth — Cargo.toml's `rust-version` — which `msrv` builds at.
msrv-version := `sed -n 's/^rust-version *= *"\([^"]*\)".*/\1/p' Cargo.toml`

# The merge base the affected tier keys off, as it arrives from the environment. CI
# exports NX_BASE (`nx-set-shas`); locally it falls back to origin/main.
_nx_base_input := env_var_or_default("NX_BASE", "origin/main")

# Validate that environment input at the boundary, before anything interpolates it into a
# command: a git ref or SHA is letters, digits and `. _ / -`.
base := if _nx_base_input =~ '^[A-Za-z0-9._/-]+$' { _nx_base_input } else { error("NX_BASE must be a plain git ref or SHA — letters, digits and . _ / - only; got: " + _nx_base_input) }

# List available recipes.
default:
    @just --list

# Set up from a clean clone: Nx and onebudgetspec through bun, then the one Cargo
# workspace's dependencies (rust-toolchain.toml pins the toolchain rustup installs).
bootstrap:
    @command -v cargo >/dev/null 2>&1 || { echo "cargo not found — install rustup (https://rustup.rs); rust-toolchain.toml pins the toolchain"; exit 1; }
    @command -v bun >/dev/null 2>&1 || { echo "bun not found — install it (https://bun.sh); Nx runs on it"; exit 1; }
    bun install --frozen-lockfile --silent
    cargo fetch --locked

# The full deterministic gate, at the AFFECTED tier by default (every target this change
# can reach, keyed off the merge base) or `all` for one full sweep. Format, lint (clippy
# and rustdoc), the tests with their 95% line-coverage bar, and the build run through Nx;
# the dependency-ban check runs first, and the e2e tier and the budgets after, at the
# same tier.
check tier="affected": deny-bans && (test-e2e tier) (budgets tier)
    {{ if tier == "all" { "bunx nx run-many" } else if tier == "affected" { "bunx nx affected --base=" + base } else { error("unknown tier '" + tier + "' — use 'affected' (the default) or 'all'") } }} -t format lint test build

# The full local gate, timed: the everyday gate plus the supply chain, run inside
# scripts/gate-time.sh, which records its wall clock to .gate-time.json and then checks
# `onefollowup-gate-time` (budgets.yaml, `host-variable`) against it — over its 30
# minutes warns and still passes. No pre-push hook runs it: CI owns the gate.
gate:
    @bash scripts/gate-time.sh just gate-body

# What `just gate` times.
gate-body: check supply-chain

# The supply chain, once, in the CI `lint` job: the dependency bans, advisories, licenses
# and sources (deny.toml), unused dependencies, and the build at the MSRV.
supply-chain: deny-bans msrv
    @out="$(cargo deny check advisories licenses sources 2>&1)" || { echo "$out"; exit 1; }
    @out="$(cargo machete --skip-target-dir 2>&1)" || { echo "$out"; exit 1; }

# The dependency-ban check alone: refuses a graph carrying any crate deny.toml bans.
# Offline, so `check` runs it every time.
deny-bans:
    @out="$(cargo deny check bans 2>&1)" || { echo "$out"; exit 1; }

# Build the crate at the minimum supported Rust version Cargo.toml declares.
msrv:
    @rustup toolchain list | grep -q '^{{msrv-version}}' || rustup toolchain install {{msrv-version}} --profile minimal --no-self-update >/dev/null
    cargo +{{msrv-version}} check -p onefollowup --locked

# What each CI test job runs on its platform: the tests, builds, e2e journeys and budgets
# of the projects this change can reach. Formatting, linting and the supply chain run
# once, in the `lint` job.
ci-test:
    bunx nx affected --base={{base}} -t test build e2e budgets budgets-host

# What each leg of .github/workflows/rust-cache-warm.yml runs on a push to main: compile
# everything `ci-test` compiles on that platform and run nothing, so a pull request restores
# it. Each Rust project declares `test-compile` beside its `test` target.
ci-warm:
    @bunx nx show projects --with-target test-compile | grep -q . || { echo "ci-warm: no project declares a test-compile target; add one beside each Rust project's test target" >&2; exit 1; }
    bunx nx run-many --skip-nx-cache -t build test-compile

# One phase of one release job: `just release <job> <phase> --tag <tag>`, where <job> is
# a job of .github/workflows/release.yml (build-linux, build-linux-arm64, build-macos,
# build-windows, publish) and <phase> is build, publish, verify or available.
# scripts/release.sh says what each does.
release job phase *args:
    bash scripts/release.sh {{job}} {{phase}} {{args}}

# The local packaging proof on this host's platform: the crate's publish dry run, the
# wheel built and installed into a fresh virtual environment, and the npm launcher and
# this platform's carrier packed and installed into an empty directory — each installed
# executable run. The packaging project's `e2e` target.
package-smoke:
    bash scripts/package-smoke.sh

# The test tier on its own, for iterating: the projects this change can reach.
test:
    bunx nx affected --base={{base}} -t test

# The e2e tier: crates/onefollowup-e2e drives the built binary; packaging installs it
# from the wheel and the npm packages. `check` runs this after its body, at the same tier.
test-e2e tier="affected":
    {{ if tier == "all" { "bunx nx run-many" } else if tier == "affected" { "bunx nx affected --base=" + base } else { error("unknown tier '" + tier + "' — use 'affected' (the default) or 'all'") } }} -t e2e

# Format check and lint (clippy, rustdoc, the workspace's own checks) for the projects
# this change can reach; fail on findings.
lint:
    bunx nx affected --base={{base}} -t format lint

# Format the whole tree in place.
format:
    cargo fmt --all

# Upgrade every ecosystem's dependencies, then re-run the full gate as the broader tier:
# an upgrade can reach any project.
upgrade:
    cargo update
    bun update --silent
    @just check all

# Provision the dev toolchain for a session (ensures `just`, then runs setup-llmlint).
# The Claude Code SessionStart hook runs it; idempotent, and a no-op in CI.
session-setup:
    ./scripts/session-setup.sh

# Install or refresh the llmlint toolchain (oneharness + llmlint). Idempotent.
setup-llmlint:
    ./scripts/setup-llmlint.sh

# LLM-judge lint (llmlint), on demand over the configured set (or the given paths). Kept
# out of `check` on purpose: the gate stays deterministic and offline.
lint-llm *paths:
    @command -v llmlint >/dev/null 2>&1 || { echo "llmlint not installed — run 'just setup-llmlint'"; exit 1; }
    llmlint {{paths}}

# The deterministic llmlint gate — no model call, no credential: config structure,
# `llmlint: ignore` directives and fragment version bumps. CI runs it before the model.
lint-llm-validate *args:
    @command -v llmlint >/dev/null 2>&1 || { echo "llmlint not installed — run 'just setup-llmlint'"; exit 1; }
    llmlint validate {{args}}

# llmlint over the files this branch changed since it forked from BASE (three-dot), the
# judge restricted to the changed lines: the blocking `llmlint` step of CI's lint job.
lint-llm-diff base="origin/main" *args:
    @command -v llmlint >/dev/null 2>&1 || { echo "llmlint not installed — run 'just setup-llmlint'"; exit 1; }
    llmlint --diff --diff-base "{{base}}" {{args}}

# The onebudgetspec budgets `check` runs at its tier: every domain's budget targets, then
# the root `budgets.yaml` — strictly, leaving out the `release-tail` budgets the release's
# `publish` job reports and the `host-variable` one `just gate` checks as a warning.
budgets tier="affected":
    {{ if tier == "all" { "bunx nx run-many" } else if tier == "affected" { "bunx nx affected --base=" + base } else { error("unknown tier '" + tier + "' — use 'affected' (the default) or 'all'") } }} -t budgets budgets-host
    [ ! -f budgets.yaml ] || bunx onebudgetspec check budgets.yaml --exclude-label release-tail --exclude-label host-variable
