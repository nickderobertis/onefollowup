# Grooming report — `groom-20261121T093012Z`

Destination **followups** (GitHub Project #3) · verified against each repository's `main` at the commit prepare fetched · 2026-11-21 09:30 UTC → 10:47 UTC

| Verdict | Tickets |
| --- | ---: |
| Fixed (closed as completed) | 9 |
| Obsolete (closed as not planned) | 3 |
| Duplicate (closed as duplicate) | 6 |
| Survivor (kept open, absorbed a duplicate) | 5 |
| Kept | 539 |
| — of which unresolved | 4 |
| — of which protected (Queued, In Progress) | 3 |
| **Open at the start** | **562** |
| **Open now** | **544** |

Selected 120 of 562 open tickets: 439 were unchanged since their last grooming record, 3 were protected. 12 shards of 10; every changed ticket read back and confirmed (23 of 23).

## Changed tickets

| Ticket | Verdict | Deciding commit | Survivor |
| --- | --- | --- | --- |
| [ai-orchestrator#2330](https://github.com/nickderobertis/ai-orchestrator/issues/2330) Issue size checks must include markup added by the board source | fixed | [onetaskgraph@b4e4a94](https://github.com/nickderobertis/onetaskgraph/commit/b4e4a94) feat: report oversize copies before writing (#3971) | — |
| [ai-orchestrator#2339](https://github.com/nickderobertis/ai-orchestrator/issues/2339) Build caches evict reusable test proofs before release | fixed | [onepipeline@6599014](https://github.com/nickderobertis/onepipeline/commit/6599014) ci: keep proven test results in the Actions cache (#1447) | — |
| [onevcs#325](https://github.com/nickderobertis/onevcs/issues/325) Default publication window expires before normal required CI | fixed | [onevcs@1b2f41f](https://github.com/nickderobertis/onevcs/commit/1b2f41f) feat(session)!: merge a moved base on conflict (#372) | — |
| [onevcs#441](https://github.com/nickderobertis/onevcs/issues/441) New worktrees must run repository provisioning before use | fixed | [onevcs@4a2e2a0](https://github.com/nickderobertis/onevcs/commit/4a2e2a0) feat(session): record post-checkout provisioning (#642) | — |
| [onepipeline#1651](https://github.com/nickderobertis/onepipeline/issues/1651) Driver reenters scheduling after final retry | fixed | [onepipeline@6f7fde2](https://github.com/nickderobertis/onepipeline/commit/6f7fde2) feat(start)!: redo changed work and its dependents (#1721) | — |
| [crozier#836](https://github.com/nickderobertis/crozier/issues/836) Visual-doc tool downloads survive transient connection failures | fixed | [crozier@3071c37](https://github.com/nickderobertis/crozier/commit/3071c37) ci: deliver the final required verdict within the gate budget (#855) | — |
| [crozier#842](https://github.com/nickderobertis/crozier/issues/842) Wire aliases are emitted as literal data | fixed | [crozier@380b980](https://github.com/nickderobertis/crozier/commit/380b980) fix(parity): emit valid worked examples and wire aliases (#871) | — |
| [onepipeline-ui#761](https://github.com/nickderobertis/onepipeline-ui/issues/761) Linux release checks must fit inside their enforced job window | fixed | [onepipeline-ui@84e452a](https://github.com/nickderobertis/onepipeline-ui/commit/84e452a) fix(ci): finish the release gate inside its timeout (#764) | — |
| [ai-orchestrator#2595](https://github.com/nickderobertis/ai-orchestrator/issues/2595) The Codex stand-in applies directory trust checks only where enforced · *Todo* | fixed | [ai-orchestrator@e96f2c3](https://github.com/nickderobertis/ai-orchestrator/commit/e96f2c3) fix(e2e): make the fake Codex refuse app-server flags | — |
| [ai-orchestrator#2481](https://github.com/nickderobertis/ai-orchestrator/issues/2481) Proposal: host follow-up drafts need their own store | obsolete | [ai-orchestrator@7ebe3bd](https://github.com/nickderobertis/ai-orchestrator/commit/7ebe3bd) feat(follow-ups): run follow-ups through onefollowup | — |
| [onepipeline-ui#803](https://github.com/nickderobertis/onepipeline-ui/issues/803) Proposal: paginate the transcript drawer | obsolete | [onepipeline-ui@038f4b8](https://github.com/nickderobertis/onepipeline-ui/commit/038f4b8) feat(dag-ui): node timelines that always finish loading (#832) | — |
| [ai-orchestrator#2318](https://github.com/nickderobertis/ai-orchestrator/issues/2318) Host planning reads a second template copy · *Todo* | obsolete | [ai-orchestrator@e48ac9c](https://github.com/nickderobertis/ai-orchestrator/commit/e48ac9c) feat: run planning as configured plan templates | — |
| [onevcs#437](https://github.com/nickderobertis/onevcs/issues/437) Auto-merge capability read uses a field gh rejects | duplicate | — | [onevcs#447](https://github.com/nickderobertis/onevcs/issues/447) |
| [onejudge#213](https://github.com/nickderobertis/onejudge/issues/213) Judge retries reuse an expired session | duplicate | — | [onejudge#250](https://github.com/nickderobertis/onejudge/issues/250) |
| [onejudge#216](https://github.com/nickderobertis/onejudge/issues/216) A timed-out judge reports a rejection | duplicate | — | [onejudge#250](https://github.com/nickderobertis/onejudge/issues/250) |
| [onepipeline#1253](https://github.com/nickderobertis/onepipeline/issues/1253) Verdict parse failures read as refusals | duplicate | — | [onejudge#249](https://github.com/nickderobertis/onejudge/issues/249) |
| [onetaskgraph#4063](https://github.com/nickderobertis/onetaskgraph/issues/4063) Board listing walks closed items · *Todo* | duplicate | — | [onetaskgraph#3696](https://github.com/nickderobertis/onetaskgraph/issues/3696) · *In Progress* |
| [onepipeline-ui#727](https://github.com/nickderobertis/onepipeline-ui/issues/727) Check tables overflow at phone widths | duplicate | — | [onepipeline-ui#759](https://github.com/nickderobertis/onepipeline-ui/issues/759) |
| [onevcs#447](https://github.com/nickderobertis/onevcs/issues/447) Publications whose checks outlast the watch fail to arm auto-merge | survivor | — | absorbed onevcs#437 |
| [onejudge#250](https://github.com/nickderobertis/onejudge/issues/250) Judge sessions are not renewed across retries | survivor | — | absorbed onejudge#213, onejudge#216 |
| [onejudge#249](https://github.com/nickderobertis/onejudge/issues/249) Verdict parsing treats malformed output as a refusal | survivor | — | absorbed onepipeline#1253 |
| [onepipeline-ui#759](https://github.com/nickderobertis/onepipeline-ui/issues/759) Remote check tables must stay readable at phone widths | survivor | — | absorbed onepipeline-ui#727 |
| [onetaskgraph#3696](https://github.com/nickderobertis/onetaskgraph/issues/3696) Open-item listings read the whole board · *In Progress* | survivor | — | absorbed onetaskgraph#4063 (its Todo acceptance carried over) |

One survivor sits outside the repository that owns the seam: [onejudge#249](https://github.com/nickderobertis/onejudge/issues/249) absorbed onepipeline#1253, whose members name `github.com/nickderobertis/onepipeline` as the seam owner.

## Unresolved, kept open

| Ticket | Why it stays open |
| --- | --- |
| [ai-orchestrator#2313](https://github.com/nickderobertis/ai-orchestrator/issues/2313) | The cited resolver and journey were deleted by e48ac9c; the claim may live on elsewhere and no fixing change was found. |
| [ai-orchestrator#2347](https://github.com/nickderobertis/ai-orchestrator/issues/2347) | The named mechanism moved to `scripts/dispatch-env.sh`; whether the new path still drops the authoring root was not exercised. |
| [onepipeline#1536](https://github.com/nickderobertis/onepipeline/issues/1536) | The premise does not match the tree (every job has a stop step), but the reported hang was not reproduced either way. |
| [ai-orchestrator#2546](https://github.com/nickderobertis/ai-orchestrator/issues/2546) | Board fields are projected for new copies only; no commit backfills existing ones. Confidence low. |

## Protected, kept open

These tickets are Queued or In Progress, so grooming never closes or folds them.

| Ticket | Status | Verdict it would have had |
| --- | --- | --- |
| [onetaskgraph#3696](https://github.com/nickderobertis/onetaskgraph/issues/3696) | In Progress | survivor |
| [onepipeline#1656](https://github.com/nickderobertis/onepipeline/issues/1656) | Queued | fixed (6f7fde2) — left to its owner |
| [onevcs#693](https://github.com/nickderobertis/onevcs/issues/693) | In Progress | keep |
