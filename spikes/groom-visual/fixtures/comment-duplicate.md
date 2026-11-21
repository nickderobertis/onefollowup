**Duplicate of [nickderobertis/onevcs#447](https://github.com/nickderobertis/onevcs/issues/447)**, closed by a grooming pass over the follow-ups board.

**Shared root cause:** onevcs reads auto-merge capability with `gh repo view --json autoMergeAllowed` (`crates/onevcs/src/host.rs:2523`), a field gh 2.100.0 rejects.

**Why #447 survives:** both name the same query in the same function; #447 also states its consequence (healthy publications fail while checks are still running), so this ticket's claim is a subset. This ticket's distinct evidence — the exact gh error text — is carried on #447.

**Verified against:** `github.com/nickderobertis/onevcs@9c0e4d1a7b` (`main`, 2026-11-21 09:30 UTC) · confidence **high**

Follow #447 for the fix. If this is a different problem, reopen it and say how it differs.

<!-- orchestrator:follow-up-comment run="groom-20261121T093012Z" kind="groom" verdict="duplicate" -->
