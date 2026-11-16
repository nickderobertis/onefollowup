---
# onetaskgraph#4039 on the `followups` board (followups:I_kwDOUBBgxs8AAAABfa1cVw,
# https://github.com/nickderobertis/onetaskgraph/issues/4039), read through the store's own
# read (`onetaskgraph task show`, ai-orchestrator's `.venv/bin/onetaskgraph`) on 2026-11-15.
# Its title, its `orchestrator.follow-up` record and its body as they stood there, the
# record's `schema` set from 15 to 14 as this plan's relabel node sets it; nothing else.
title: "onetaskgraph: Unchanged empty metadata projections must not reread board fields"
metadata:
  orchestrator.follow-up: {"schema": 14, "root_cause": "absent-metadata-projections-avoid-board-reads", "repository": "github.com/nickderobertis/onetaskgraph", "created_by_run": "Move-follow-up-ticket-metadata-onto-board-fields-and-collapse-its-long-optional-parts", "owning_runs": ["Move-follow-up-ticket-metadata-onto-board-fields-and-collapse-its-long-optional-parts"], "drafts": ["drafts:Move-follow-up-ticket-metadata-onto-board-fields-and-collapse-its-long-optional-parts/drafts/20261114T003447Z-a-projected-metadata-field-with-no-value-costs-e"], "draft_sources": {"drafts:Move-follow-up-ticket-metadata-onto-board-fields-and-collapse-its-long-optional-parts/drafts/20261114T003447Z-a-projected-metadata-field-with-no-value-costs-e": "author node; node follow-up-ticket-fields"}, "transcripts": {"drafts:Move-follow-up-ticket-metadata-onto-board-fields-and-collapse-its-long-optional-parts/drafts/20261114T003447Z-a-projected-metadata-field-with-no-value-costs-e": "onepipeline transcript Move-follow-up-ticket-metadata-onto-board-fields-and-collapse-its-long-optional-parts follow-up-ticket-fields"}, "basis": {"github.com/nickderobertis/onetaskgraph": "de029a41dbf848f4f00441e15aad0d90dcd1793c"}, "witnesses": ["github.com/nickderobertis/onetaskgraph crates/onetaskgraph-github-projects/src/lib.rs:9007 at de029a41dbf848f4f00441e15aad0d90dcd1793c", "github.com/nickderobertis/onetaskgraph crates/onetaskgraph-github-projects/src/lib.rs:9033 at de029a41dbf848f4f00441e15aad0d90dcd1793c"], "verified_at": "2026-11-15T11:32:11Z", "host": "shulgin", "priority_estimate": "medium", "frequency": "consistent", "board_estimate": "medium"}
---
## Root cause

A metadata write rereads the board’s field definitions whenever a configured projection is empty, even when that projected value did not change. Tickets without transcripts therefore pay an extra shared API request on every later metadata update.

## Impact

**Who is affected.** Follow-up automation updating records with absent projected values.

**What it costs them.** One additional board-fields request per affected metadata write.

**What it costs the product owner.** Repeated maintenance consumes shared API allowance and risks delaying board workflows; the evidence does not measure an outage or spending increase.

<details>
<summary>Cost of the workaround</summary>

- Workaround: Keep source drafts available so transcript projections remain populated where a transcript exists.
- Applying it: Retain draft material through subsequent maintenance; manager-only tickets still have no transcript.
- Side effects on the outcome: No work is lost; verification or delivery is delayed.
- Discovery: The extra request is visible only in request telemetry, so investigation is needed.

</details>

<details>
<summary>Priority calculation</summary>

- Severity: low
- Severity with the workaround: low
- Priority estimate: medium (severity with the workaround low; fires consistently; 1 occurrence; raised one level because it fires consistently)

</details>

## Examples

The transcript records an extra board-fields read when transcripts were absent, and no such read with the drafts present.

## Suggested fix

Compare old and new rendered projection values before resolving field definitions. When all projections for the changed metadata key are unchanged, write the metadata body without reading or writing board fields.

<details>
<summary>Contributing locations</summary>

- onetaskgraph `crates/onetaskgraph-github-projects/src/lib.rs`: Resolves projection board fields before determining whether any projected value moved.
- onetaskgraph `crates/onetaskgraph-github-projects/src/lib.rs`: Requires every projected field to be present on the item to avoid a board-field read.

</details>

### Unchanged empty metadata projections must not reread board fields — `onetaskgraph`

Compare old and new rendered projection values before resolving field definitions. When all projections for the changed metadata key are unchanged, write the metadata body without reading or writing board fields. This changes behavior without adding a public interface.

## Evidence

The transcript records an extra board-fields read when transcripts were absent, and no such read with the drafts present. The basis resolves the board unconditionally for a projected metadata key, then falls back to a board read when an empty projection is not carried by the item.