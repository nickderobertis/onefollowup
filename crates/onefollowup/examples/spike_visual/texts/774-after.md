<details>
<summary>Additional evidence from follow-up run run-4fc7918e3c78.</summary>

- Host: lima-hp
- Basis: github.com/nickderobertis/onepipeline at c3f74a07da428dcf7094d18c4c6d21d250111bc9
- Draft: drafts:run-4fc7918e3c78/drafts/20261031T144336Z-pr-author-drafting-fails-to-start-with-argument

</details>

The monitor records a title-unverified surface at 2026-10-31T14:42:33Z: the rework's description drafter could not start because of Argument list too long (os error 7), leaving its previous description in place. At this basis, src/agentgraph.rs:1205 still passes the complete task in one argument, and src/lifecycle.rs:3758-3800 composes the drafting assignment.

Bears on the ticket: The impact also includes automatic change-description drafting after rework, not only worker-node startup. The existing file-input transport fix covers that caller too.

<!-- orchestrator:follow-up-comment run="run-4fc7918e3c78" root_cause="retained-task-text-exceeds-argv-limit" -->
