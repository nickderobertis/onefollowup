<details>
<summary>Additional evidence from follow-up run nightly-2026-11-09.</summary>

- Host: build-host-1
- Basis: github.com/nickderobertis/ai-orchestrator at 8bf72a3029360978101838ea47f80033292d626f
- Draft: drafts:nightly-2026-11-09/drafts/20261114T045033Z-plans-adopting-an-onetaskgraph-release-omit-the; author manager
- Witness: github.com/nickderobertis/ai-orchestrator personas/orchestrator.yaml:142 at 8bf72a3029360978101838ea47f80033292d626f
- Witness: github.com/nickderobertis/ai-orchestrator budgets.yaml:56 at 8bf72a3029360978101838ea47f80033292d626f

</details>

budget cycle-time: actual 129683.479 seconds, budget 10800.0 seconds, headroom -118883.479 seconds (-1100.772953703704%) — over; host: load1 9.919921875, cpus 20, available memory 87689 MiB, dispatches 4. The adoption-planning draft also records the consumer parked unpublished while its missing linked-release prerequisites were resolved. Scheduling was 79275.461 seconds; agent 26491.962; release wait 14979.122; gates 8887.012 across one failed and one passed run (4210.333 and 4676.679 seconds); publication 47.281; merge queue 2.641. Five dispatches and two publication attempts were recorded. No segment is unmeasured. The telemetry establishes prolonged scheduling/recovery wait but cannot apportion it among individual decisions or holds. The host snapshot is below CPU capacity with ample available memory and does not explain a long gate; it cannot establish host conditions throughout either gate.

<!-- orchestrator:follow-up-comment run="nightly-2026-11-09" root_cause="export-dialog-drops-unicode" -->
