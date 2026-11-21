**Closed as fixed** by a grooming pass over the follow-ups board.

**Deciding change:** [nickderobertis/onevcs@1b2f41f](https://github.com/nickderobertis/onevcs/commit/1b2f41f) feat(session)!: merge a moved base on conflict and arm timed-out changes (#372), an ancestor of `main`.

**Verified against:** `github.com/nickderobertis/onevcs@9c0e4d1a7b` (`main`, 2026-11-21 09:30 UTC) · confidence **high**

**Evidence:** the publication window now comes from each identity's `checks.wait_seconds` rule (`crates/onevcs/src/session/publish.rs:412`), and a change whose required checks outlast it is armed for auto-merge instead of failing. The repository's `onevcs.toml` sets 1470 s, longer than its slowest required check (≈ 1210 s over the last 20 runs).

**Paths:** `crates/onevcs/src/session/publish.rs`, `onevcs.toml`

If this is not fixed for you, reopen it with what you saw; the next grooming pass reads every comment made after this one.

<!-- orchestrator:follow-up-comment run="groom-20261121T093012Z" kind="groom" verdict="fixed" -->
