<a id="root-cause"></a>
## Root cause

the implementation detailed below opens a fresh event stream and initializes fresh watermarks for each follow. Those marks are not seeded from the run journal when the same session is followed again, so earlier session events are relayed again on republication.

<a id="impact"></a>
## Impact

**Who is affected.** Managers and telemetry consumers reading retried lifecycle changes.

**What it costs them.** The third publication appended 73 previously recorded events, including old failures; status counted 663 events while very little new progress had occurred.

**What it costs the product owner.** Users and development suffer recurring diagnostic friction and unreliable event counts. Distinct gate intervals are unioned by the adopted telemetry contract, so duplicate records are not additional gate executions.

**Cost of the workaround.**
- Applying it: Deduplicate the affected journal read by producer identity.
- Side effects on the outcome: none to the intended delivered result; verification or automatic progress may complete later.
- Discovery: Repeated timestamps and sequence numbers permit detection, but the status counter does not identify replay.

- Severity: medium
- Workaround: Compare source, stream and sequence before treating relayed records as new occurrences.
- Severity with the workaround: low
- Priority estimate: medium (severity with the workaround low; fires consistently; 2 occurrences; raised one level because it fires consistently)

<a id="examples"></a>
## Examples

During the third publication of node otg-assets-core-r2, the run's events.jsonl gained 73 onevcs events for session s-8646eed50936 whose source, stream, sequence, kind and timestamp already appear earlier in the same journal — starting from seq 1 `fetch` at 2026-10-06T21:34:57.924Z and including both earlier publication failures. `just status` then counted 663 events for the node where the real new progress was a worker settlement and one `fetch`. A reader of the stream sees old gate failures as if fresh, which is how a third refusal would be misread.

<a id="suggested-fix"></a>
## Suggested fix

The original suggested change is retained below; no additional remedy is inferred.

**Contributing locations.**
- github.com/nickderobertis/onepipeline `src/vcs.rs`: Named in the original text reproduced below.



src/vcs.rs opens a fresh event stream and initializes fresh watermarks for each follow. Those marks are not seeded from the run journal when the same session is followed again, so earlier session events are relayed again on republication.

<a id="suggested-change--githubcomnickderobertisonepipeline"></a>
### Suggested change — `github.com/nickderobertis/onepipeline`

Resume each session relay from the producer watermarks already present in the run journal. A later publication then appends only events beyond those marks.

<a id="session-event-relay--githubcomnickderobertisonepipeline-srcvcsrs"></a>
#### Session event relay — `github.com/nickderobertis/onepipeline` (`src/vcs.rs`)

Documented behavior: every producer source/stream/sequence identity is appended to a run at most once across republications; event shapes and CLI syntax stay unchanged.

<a id="owning-runs"></a>
## Owning runs

`Before-and-after-screenshots-in-the-design-document-for-plans-that-change-what-a-user-sees`

<a id="evidence"></a>
## Evidence

Verification host: `shulgin`. Basis: `github.com/nickderobertis/onepipeline` at `7bf311fe9cd9d824d85fa01884fdb4799d3c6916`.

- `drafts:Before-and-after-screenshots-in-the-design-document-for-plans-that-change-what-a-user-sees/drafts/20261007T005655Z-a-run-journal-re-appends-a-session-s-earlier-vcs`; run `Before-and-after-screenshots-in-the-design-document-for-plans-that-change-what-a-user-sees`; author `manager`, node `None`. Verified at src/vcs.rs:1727; src/vcs.rs:1732; src/vcs.rs:1734: src/vcs.rs opens a fresh event stream and initializes fresh watermarks for each follow. Those marks are not seeded from the run journal when the same session is followed again, so earlier session events are relayed again on republication. Transcript: `none (manager draft)`.
