<a id="root-cause"></a>
## Root cause

An Nx task declares only outputs it produces; the shared Cargo target directory must never be cached as a task output.

<a id="impact"></a>
## Impact

**Who is affected.** Agents and people building or gating oneharness-ui, whose Nx runs restore the `ipc-contract` build from cache.

**What it costs them.** That TypeScript build inherits `{workspaceRoot}/target` as an output, so Nx snapshots and restores the whole Cargo artifact tree with it. Each restore rewrites gigabytes and invalidates Cargo's fingerprints, so the Rust builds that follow start over. This happens on every cached build of that target. The quoted triage comment additionally reports 24 GiB of cache bloat from the same output declaration; this is disk/cache overhead, not an established dollar or subscription-quota cost.

**What it costs the product owner.** the regularly used build and gate workflow pays repeated Cargo rebuilds and gigabytes of disk writes. That is a real slowdown of that workflow, so `high`.

**Cost of the workaround.** There is no acceptable workaround. Nothing signals that the slow Rust builds come from an unrelated target's cache restore, so nobody is directed to skip the Nx cache for it, and skipping it everywhere gives up the cache.

- Severity: high
- Workaround: none
- Severity with the workaround: high
- Priority estimate: high (severity with the workaround high; fires consistently; 3 occurrences; already high, where a raise is capped, so not raised though it fires consistently and it has 3 or more occurrences)

<a id="examples"></a>
## Examples

The draft recorded this behavior during run `rust-dev-profile-and-target-dir`; the cited basis still contains the triggering configuration.

<a id="suggested-fix"></a>
## Suggested fix

Remove the workspace Cargo target directory from the inherited build outputs, and let each project declare only the outputs it really writes. A non-Cargo build's cache entry then no longer carries or restores the Cargo tree.

**Contributing locations.**
- github.com/nickderobertis/oneharness-ui `nx.json`: targetDefaults.build.outputs lists `{workspaceRoot}/target` for inherited builds.
- github.com/nickderobertis/oneharness-ui `packages/*/project.json`: build targets, including ipc-contract, inherit the default instead of declaring their own outputs.

<a id="nx-workspace--oneharness-ui-nxjson"></a>
### Nx workspace — `oneharness-ui` (`nx.json`)

```diff
 "build": {
-  "outputs": ["{projectRoot}/out", "{projectRoot}/dist", "{workspaceRoot}/target"]
+  "outputs": ["{projectRoot}/out", "{projectRoot}/dist"]
 }
```

<a id="project-build-outputs--oneharness-ui-packagesprojectjson"></a>
### Project build outputs — `oneharness-ui` (`packages/*/project.json`)

Declare only each project build’s actual outputs instead of inheriting unrelated directories; in particular, `ipc-contract` must not cache the shared Cargo tree.

<a id="owning-runs"></a>
## Owning runs

`rust-dev-profile-and-target-dir`

<a id="related-tickets"></a>
## Related tickets

- [oneharness-ui: oneharness ui build cache copies shared cargo target](https://github.com/nickderobertis/oneharness-ui/issues/105) — related: The quoted triage reports this ticket merged as a duplicate; its 24 GiB cache bloat is another symptom of this same output declaration.

<a id="evidence"></a>
## Evidence

Verification host: `lima-hp`. `drafts:rust-dev-profile-and-target-dir/drafts/20260920T185242Z-nx-build-targetdefault-snapshots-the-whole-cargo`. At basis `a669dae4fc04792bdbc2578500fc258851d8f398`, `nx.json:25` still lists `{workspaceRoot}/target`; `packages/ipc-contract/project.json` inherits the default. Transcript: `onepipeline transcript rust-dev-profile-and-target-dir oneharness-ui`.

The quoted triage comment reports the same declaration still present at `origin/main df7c11c` and identifies the inherited project outputs and 24 GiB cache-bloat symptom. This is reported evidence from that comment, not a new local reproduction.
