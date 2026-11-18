## Root cause

An Nx task declares only outputs it produces; the shared Cargo target directory must never be cached as a task output.

## Impact

**Who is affected.** Agents and people building or gating oneharness-ui, whose Nx runs restore the `ipc-contract` build from cache.

**What it costs them.** That TypeScript build inherits `{workspaceRoot}/target` as an output, so Nx snapshots and restores the whole Cargo artifact tree with it. Each restore rewrites gigabytes and invalidates Cargo's fingerprints, so the Rust builds that follow start over. This happens on every cached build of that target. The quoted triage comment additionally reports 24 GiB of cache bloat from the same output declaration; this is disk/cache overhead, not an established dollar or subscription-quota cost.

**What it costs the product owner.** the regularly used build and gate workflow pays repeated Cargo rebuilds and gigabytes of disk writes. That is a real slowdown of that workflow, so `high`.

<details>
<summary>Cost of the workaround</summary>

- Workaround: none
There is no acceptable workaround. Nothing signals that the slow Rust builds come from an unrelated target's cache restore, so nobody is directed to skip the Nx cache for it, and skipping it everywhere gives up the cache.

</details>

<details>
<summary>Priority calculation</summary>

- Severity: high
- Severity with the workaround: high
- Priority estimate: high (severity with the workaround high; fires consistently; 3 occurrences; already high, where a raise is capped, so not raised though it fires consistently and it has 3 or more occurrences)

</details>

## Examples

The draft recorded this behavior during run `rust-dev-profile-and-target-dir`; the cited basis still contains the triggering configuration.

## Suggested fix

Remove the workspace Cargo target directory from the inherited build outputs, and let each project declare only the outputs it really writes. A non-Cargo build's cache entry then no longer carries or restores the Cargo tree.

<details>
<summary>Contributing locations</summary>

- github.com/nickderobertis/oneharness-ui `nx.json`: targetDefaults.build.outputs lists `{workspaceRoot}/target` for inherited builds.
- github.com/nickderobertis/oneharness-ui `packages/*/project.json`: build targets, including ipc-contract, inherit the default instead of declaring their own outputs.

</details>

### Nx workspace — `oneharness-ui` (`nx.json`)

```diff
 "build": {
-  "outputs": ["{projectRoot}/out", "{projectRoot}/dist", "{workspaceRoot}/target"]
+  "outputs": ["{projectRoot}/out", "{projectRoot}/dist"]
 }
```

### Project build outputs — `oneharness-ui` (`packages/*/project.json`)

Declare only each project build’s actual outputs instead of inheriting unrelated directories; in particular, `ipc-contract` must not cache the shared Cargo tree.

## Related tickets

- [oneharness-ui: oneharness ui build cache copies shared cargo target](https://github.com/nickderobertis/oneharness-ui/issues/105) — related: The quoted triage reports this ticket merged as a duplicate; its 24 GiB cache bloat is another symptom of this same output declaration.

## Evidence

At basis `a669dae4fc04792bdbc2578500fc258851d8f398`, `nx.json:25` still lists `{workspaceRoot}/target`; `packages/ipc-contract/project.json` inherits the default.

The quoted triage comment reports the same declaration still present at `origin/main df7c11c` and identifies the inherited project outputs and 24 GiB cache-bloat symptom. This is reported evidence from that comment, not a new local reproduction.
