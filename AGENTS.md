# AGENTS.md

Durable instructions for humans and agents working in this repo. `CLAUDE.md` is a symlink
to this file; edit `AGENTS.md` only. Keep it terse: add a line only if a future task needs
it and would not surface it anyway (a failing gate, `just --list`, the code, a linked doc).
Folder-scoped rules live in that folder's `AGENTS.md`.

## What this repo is

`onefollowup`: a Rust CLI and library for the follow-up loop of any orchestrator
repository — record drafts of what a run noticed, verify them, file one ticket per root
cause on the destination its team watches, and answer people's comments on those tickets.
Every store access goes through onetaskgraph. Shipped as the `onefollowup` crate, the
`onefollowup-cli` PyPI wheels and the `onefollowup-cli` npm launcher; the PyPI and npm
packages carry the CLI binary only — there is no other language SDK. The contract (verbs,
configuration, record formats) is documented in `README.md`.

## Two standing goals on every task

The user's request is the priority, but carry two goals into every task, folding them in
when they are the lowest-error path and drafting follow-ups otherwise:

1. **Engineer the context for next time**: realistic e2e tests of what the user sees,
   scripts that shrink repetitive steps to signal, terse notes of what the code hides.
2. **Engineer the codebase and environment**: keep it clean, the gate strict, and setup
   reproducible from a clean clone (`just bootstrap`), with local/CI parity.

## Stack and composition

- **Product shape:** cli
- **Language(s):** rust
- **References composed:** base.md, project-graph.md, shapes/cli.md, languages/rust.md,
  intersections/rust-cli.md, ci.md, llmlint.md, releasing.md, tools/onebudgetspec.md
  (`compose_repo_plan.py --shape cli --language rust --releasing`, then `--wiring .` and
  `--ci-cd . --language rust --name onefollowup`, create-repo v1.58.1).
- **Projects in the graph:** `onefollowup` (`crates/onefollowup`: the library, the
  `onefollowup` binary over it, their unit and library tests — format, lint, test,
  build), `onefollowup-e2e` (`crates/onefollowup-e2e`: drives the built binary as a
  subprocess; depends on `onefollowup:build`), `packaging` (`npm/`: the npm launcher, the
  release-targets reconciliation test, and as its `e2e` the wheel and npm packages built
  from the tree, installed and run), `workspace` (the root: the project-boundary check),
  and the composed `report-job`.
- **Excluded, and why:** asdf/direnv — `rust-toolchain.toml` pins the toolchain and
  nothing else needs a version manager; a separate repo-level `coverage` target — the
  library crate's `test` target measures and enforces the 95% line bar itself over its
  own suite (which drives the CLI in process), the e2e crate holding no library code; splitting the record formats or the
  configuration into crates of their own — the distribution publishes one crate, so they
  are modules of it, and the architecture test holds the store boundary; a bench tier —
  no hot path has a performance requirement yet; GitHub Release archives and an install
  script — the family installs this CLI from PyPI or npm, which carry the same binary.

## Command surface

Use the `just` recipes (`just --list`); each delegates to Nx. `just check` is the
deterministic gate (affected tier; `just check all` sweeps), `just gate` adds the supply
chain and times it. `just package-smoke` is the local packaging proof. `just lint-llm-diff`
is the judged tier, out of `check`.

## Commits, releases, and merging

- **Squash-merge only, via PR, with auto-merge**; head branches auto-delete; admins may
  override. The PR title (Conventional Commits, linted in CI) becomes the squash subject
  and the PR body (`.github/pull_request_template.md`: What, Why) its body.
- **Required checks** are exactly those in `.github/governance.json`; branches need not be
  up to date. The repository owner applies them with the create-repo skill's
  `setup_github_governance.py` (it needs admin rights).
- **No broader-tier sweep runs in CI**: the composed CI/CD shape gates each pull request
  at the affected tier, and a push to main runs only `budgets`; the release re-gates
  nothing.
- **Releases**: release-plz on every push to main opens the release PR (version,
  CHANGELOG.md) and arms auto-merge on it; when it merges, release-plz tags `v<version>`
  and cuts the GitHub Release, which runs `release.yml`: each `build-<platform>` job builds
  the binary, its wheel and its npm carrier, installs and runs them, and publishes them;
  `publish` publishes the crate to crates.io and the npm launcher after every carrier.
  `release-targets.toml` declares what is published. Nobody hand-edits a version, tags or
  dispatches a publish. Pre-1.0 regime: `feat` is a minor bump, `fix` a patch, a breaking
  change a minor.

## Invariants (non-negotiable)

- The gate is strict: no warnings-only mode; a diagnostic is fixed or suppressed at its
  site with a stated reason.
- **Tests are realistic and complete.** E2E drives the built binary as a subprocess; store
  tests use onetaskgraph's `local-md` source alone (destination behaviour is
  onetaskgraph's to prove). Proof is proportionate per the
  [create-repo base rules](https://github.com/nickderobertis/dero-skills/blob/main/skills/bootstrap/create-repo/assets/llmlint/base.llmlint.yml).
- **One store seam.** Only `crates/onefollowup/src/store.rs` calls onetaskgraph, and only
  its public crate API; nothing names a destination's API host, a plugin's internals, or
  branches on a plugin or destination name. No GitHub, Linear, HTTP or GraphQL client of
  our own, and nothing of onepipeline. `tests/architecture.rs` holds this.
- **Library parity.** Every verb is one public function in `onefollowup::api`; the CLI
  only resolves settings, calls it and prints the result. `tests/parity.rs` holds this.
- **Formats are contracts.** Each record format reads and writes only its current schema
  and refuses any other, naming the format, the schema found and the schema read. A change
  to a format bumps its schema and its fixtures in the same change.
- Validate every external input at its trust boundary; no secret in the tree.

## Scripts and output are context

Scripts are quiet on success; on failure they print the exact error and a next action.

## Tests are context engineering

- **Never mock the layer under test**; a green mocked suite is worse than none.
- Done means happy path *and* failure/recovery for journeys owing e2e.
- Coverage (95% lines) is a floor, not the target.
- A cost figure is a budget, never a test assertion.
- A test driving the real `gh` at a fake host sets `DBUS_SESSION_BUS_ADDRESS=disabled:`.

## Keeping the allowlist current

The agent allowlist is `.claude/settings.json`; add a routine command there rather than
re-approving it, and keep it narrow.

## After the main task: refine and hand off

Propose only materially helpful follow-ups (scripts, `AGENTS.md` notes, skills, tests),
each with its likely impact; otherwise say so and stop.

## Budget reports

Approved budget thresholds alone decide failures: the required `budgets` check goes
red only on an ordinary budget verdict, and the budget stays the requirement. CI
artifacts retain the base branch's recent measurements for informational comparison,
and each report states their provenance, retention and coverage. A stored measurement
is never a line to hold.

A row shows the figure its project's `budgets` target saved in `test-linux`, or a figure
this job measured. A budget labelled `not-measured-on-pr` has no row and one line counts it:
a pull request cannot measure it or move its figure, and its check elsewhere is unchanged. A
base push takes the merged pull request's figures, so a pull request's value and its baseline
are one measurement.

1. When a performance problem appears that a failing budget check does not already identify (for example a CI timeout), read the PR report first through `onevcs change comments` or the host PR view. If the failing check already identifies it, skip the duplicate read. Read earlier PR comments and job results when several changes may contribute.
2. Never time base versus branch on a shared host. Profile to investigate, and use CI comparisons through the repository's existing publication path.
3. When reading a report or seeing a failing budget check, fix the regression as part of the work. For an adverse change of at least one standard deviation shown in a report being read (a `max` budget rising, a `min` budget falling), make the change cheaper or make room in existing code, profiling to find where. Improvements show numbers without flags, in a row's history too, and are no reason to optimize further, and other figures need no work. Informational flags do not fail a check or start other work.
4. Stay inside existing budgets. Make room for new tests and code by speeding up existing work; never raise limits or timeouts.
5. Suspect the current change first when something degrades, before intermittent slowness or the outside world.
