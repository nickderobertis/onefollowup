# onefollowup

The follow-up loop for any orchestrator repository: record what a run noticed but could
not fix as a **draft**, verify the drafts, file **one ticket per root cause** on the
destination its team watches, and answer people's comments on those tickets. Every store
access goes through [onetaskgraph](https://github.com/nickderobertis/onetaskgraph), so a
destination is any source onetaskgraph serves (a local Markdown folder, GitHub Projects,
Linear) and onefollowup holds no client of its own.

It ships as a CLI and a Rust library with the same surface. **v1 lands the contract**: every
verb, the configuration file and the record formats. The verify loop, the comment pass,
`config show` and `kinds` are implemented; `statuses` parses its arguments and refuses with
exit 2 and `not implemented in this release` until the release that implements it.

## Install

```sh
uv tool install onefollowup-cli      # or: pip install onefollowup-cli
npm install -g onefollowup-cli
cargo install onefollowup --locked
```

The PyPI and npm packages carry the prebuilt `onefollowup` executable alone (Linux x86_64
and arm64, macOS arm64, Windows x86_64); there is no Python or JavaScript API. The Rust
library is the `onefollowup` crate.

## Commands

| Command | What it does |
|---|---|
| `draft --batch <ID> --title <T> --repository <ORIGIN> [--path <P>]... [--author <LABEL>] [--stamp <KEY=VALUE>]...` | Record one draft; its body is read from stdin. Filed under the batch's drafts project, `<drafts_source>:<batch>`; see Private drafts, below. |
| `snapshot --batch <ID> [--alias <ID>]...` | Run `prepare`'s input-set step alone — the drafts search for the batch and its aliases, and the `batch_preparing` hook — and say whether the batch holds anything to verify: `empty` is `true` exactly when the input set holds no draft and no hook input. It reads no destination and renders no task. An empty batch leaves no working file; a non-empty first pass is saved as the batch's state, which the next `prepare` continues without running the hook again. After a settled pass it reports the new pass's input set but saves nothing — the batch state cannot mark a pass open once its last one settled — so the next `prepare` opens that pass itself, running the hook and the drafts search again. Prints `{"schema_version": 1, "batch", "drafts", "inputs", "empty", "store_operations"}`. |
| `prepare --batch <ID> [--alias <ID>]... [--unanswered]` | Snapshot the batch's inputs (its drafts and the `batch_preparing` hook's inputs), make its destination reads in one batched search — carrying every input's duplicate queries (Duplicate candidates, below) — and one comments read — under relations, also one `task relations` read of the tickets found and one read of the tickets at their edges' other ends — and write the verifying agent's task, each input's duplicate candidates rendered in it; it continues a pass `snapshot` opened, running no hook again. Prints `{"schema_version": 1, "batch", "drafts", "inputs", "answers", "task_template"}`, with `absent_targets` and `missing_links` when there are any (Relations, below) and `duplicate_searches` under a kind whose duplicates the agent judges. |
| `ticket validate --batch <ID> <TICKET>` | Hold one ticket to the rules; `absent_targets` names each edge its fields derive whose target does not exist. |
| `ticket board-status --batch <ID> <ITEM>` | Read one destination item's state and comments from the batch's prepared reads. |
| `ticket search --batch <ID> --ticket <TICKET> <QUERY>` | Search the destination for a ticket's duplicates. |
| `ticket copy --batch <ID> <TICKET>` | **Stage** a ticket's copy to its destination. |
| `ticket comment --batch <ID> (--item <ITEM> \| --field <NAME=VALUE>...) [--root-cause <RC>] [--sources-file <F>] [--bears-on-file <F>] [--question-file <F>]` | **Stage** the batch's evidence comment (read from stdin) on an item; given the declared fields observed, on the most specific existing ticket they name. The root cause defaults to the item's own. `--sources-file` is one JSON object of exactly `host`, `basis`, `drafts` and `witnesses`, validated as a ticket record's are (each draft one of the batch's inputs, each witness at the commit `basis` records); each draft's source and transcript are read from the draft. `--bears-on-file` and `--question-file` hold the paragraph saying what the evidence bears on and the question it asks. The comment is `comment_templates.evidence` rendered, then its marker (Templates, below). |
| `ticket re-estimate --batch <ID> <ITEM>` | **Stage** an item's re-estimate. |
| `ticket relate --batch <ID> --relation <NAME> --from <TICKET> --to <TICKET>` | **Stage** an edge of a relation the agent decides; a ticket is its root cause, its file, or an item the batch read. |
| `check --batch <ID>` | Hold the batch's dispositions — each that files a ticket to its input's duplicate candidates too — tickets, edges and staged writes to the rules before anything is written; exit 0 when sound. |
| `publish --batch <ID>` | Send the staged writes in batched store calls — under relations, the batch's edges in one `task parent set-many` and one `task link add-many` — and receipt every outcome. |
| `settle --batch <ID>` | Read back only the writes no answer confirmed, run the `batch_settled` hook, retire the batch's working files. |
| `comments gather [--batch <ID> \| --item <ID>] [--dry-run]` | List each destination's comments since its watermark in one comments-since listing — or every comment of the one item, or every item the batch filed — select each one to answer for the batch owning its item, and write the snapshot every later step reads (The comment pass, below). A dry run's result carries the `launch_command` that launches exactly that snapshot. |
| `comments launches --snapshot <S>` | Claim the snapshot once and write one feedback file per batch it selected comments for, reading no destination; refused for a snapshot already launched, a dry run's older than `snapshot.launchable_for`, or one gathered before another sharing a comment was launched. |
| `comments check --feedback <F>` | Hold a feedback dispatch's response account to its snapshot, and its batch's tickets to changing only quoted issues' tickets; `sound` says whether it breaks no rule. |
| `comments settle --snapshot <S> [--launched <BATCH>=<FEEDBACK>]...` | Verify every reply in one batched read, send every ticket change in one `update-many`, record what did not certainly land in `pending.json`, and move the watermark when it may. Without `--launched`, every batch the snapshot launched. |
| `comments answer [--dry-run]` | The comment pass in one verb: gather exactly as `comments gather`, plan the launches exactly as `comments launches --snapshot` that snapshot, run `comments.launch_command` once per launch in the launch list's order, and — only when every batch's command exited 0, an empty launch list included — settle exactly as `comments settle --snapshot` that snapshot. A batch whose command exits non-zero does not stop the others; then nothing is settled, so the watermark holds, and the verb exits 1, still printing its document — every batch's exit status, `failed` naming every failed batch, `settled` `null` — and naming the failed batches on stderr; a re-run gathers afresh. Refused (exit 2) without `comments.launch_command`, before anything is gathered. `--dry-run` prints exactly what `comments gather --dry-run` prints and launches and settles nothing. |
| `groom prepare [--full] [--dry-run] [--shard-size <N>]` | List the destination's open tickets with their bodies, read every comment made or edited on them since the window start in one `comments_since` read, fetch a blob-less clone of each repository they name, select the tickets to verify (Grooming, below), read the selected tickets' comments in one batched read, and write the batch under `.onefollowup/groom/<batch>/`: a snapshot of every open ticket and one task per shard of `--shard-size` (`groom.shard_size`) tickets. Three store operations. `--full` selects every open ticket; `--dry-run` answers the same counts, makes no third read and writes nothing: the clones its counts are computed from are made in temporary storage (`TMPDIR`) and removed, so the configured clones and batches are left as they were. Nothing selected answers `"shards": []` and writes no batch. |
| `groom check --batch <ID>` | Hold every shard's `verdicts.jsonl` to the verdict rules, plan the closes, duplicate folds and survivors (Grooming, below), and hold the batch's `plan.json`, as well as the plan computed, to the protection rules. With no problem standing it writes the plan to `plan.json`, what `groom publish` sends; while any stands it exits 2, still printing its document, and writes nothing. It makes no store operation. |
| `groom publish --batch <B>` | Send what the batch's passing check planned, in three store operations once the batch is admitted: one batched status read of the tickets to close or fold, one `update-many` carrying each close that read still admits, each duplicate close and every verified ticket's `groom` record, and one `comment add-many` of the evidence comments on the tickets shown closed, then each survivor's consolidation comment (Grooming, below). Every outcome is receipted in `receipts.json`; run again, it sends only what no receipt confirms, and on a fully receipted batch nothing. Then one batched read-back of every ticket changed, `report.md` and `report.json` in the batch directory, and every other working file retired but `plan.json` and `receipts.json`. Exits 1, still printing its document, while a write the destination refused or a read-back mismatch stands. |
| `statuses` | Read the destination's statuses against the configured status roles. |
| `kinds` | Print the effective kinds, relations, dispositions and routes, as the verifying agent reads them. |
| `config show [--sources]` | Print the effective configuration as JSON; `--sources` names the layer each key came from. |

The `ticket copy|comment|re-estimate|relate` tools write nothing to the destination: they
stage into the batch's outbox, refusing what the rules refuse, and `publish` sends it.

Every verb prints a JSON document on success. Exit codes: **0** success; **1** failed while
running; **2** refused — a usage error, a refused configuration, or a verb not implemented
in this release.

### Private drafts

onetaskgraph decides who may read each draft: a draft about a repository the store's
`write_policy.visibility_command` does not answer `public` for — one it cannot classify
included — is private, and onetaskgraph writes a private item only to a source declared
`visibility: private` whose backend agrees. So the batch's drafts project is created private
when the drafts source is declared private, and holds drafts of either classification, each
keeping its own: a public draft under it stays public, the verify loop reads both, and a
ticket copied onto a destination keeps its own classification (a public draft's ticket lands
public). A drafts source declared public, or declaring nothing, keeps a public drafts
project, and `draft` refuses a private draft there (exit 2) naming the visibility the source
declares, with nothing written. onefollowup never declares or overrides a source's
visibility; that is the store's configuration.

### Results

Each verb's result is one typed document of the library (`onefollowup::api`), stated here
once by example; `tests/contract.rs` holds every example to the library's type, so the two
cannot drift. A field shown `null`-able in the type (`status`, `version`, `url`, `author`,
`item`, `hook`) is `null` when there is none. Every document carries
`schema_version` 1. The nodes that implement a verb may add optional fields; changing a
field declared here is a contract change agreed with the manager first. `kinds` and
`config show` print the documents the configuration section describes.

#### `check`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "sound": false,
  "problems": [
    {
      "subject": "drafts:nightly-2026-11-01/drafts/20261101T102500Z-flaky-resize-test",
      "message": "the input has no disposition; give it exactly one"
    }
  ]
}
```

#### `comments answer`

Without `--dry-run`; with it, exactly the `comments gather --dry-run` document. `launches`
names every launched batch in launch order with its command's `exit` (`-1` when a signal ended
it; `null`, with an `error`, when it could not start). When a batch's command did not exit 0
the verb still prints this document, exiting 1: `failed` lists each such batch in launch order
and `settled` is `null`, since nothing was settled.

```json
{
  "schema_version": 1,
  "snapshot": "feedback/.snapshots/20261101T120000Z",
  "launches": [
    {
      "batch": "nightly-2026-11-01",
      "feedback": "feedback/nightly-2026-11-01/20261101T120000Z.md",
      "exit": 0
    }
  ],
  "settled": {
    "schema_version": 1,
    "snapshot": "feedback/.snapshots/20261101T120000Z",
    "settled": [
      {
        "batch": "nightly-2026-11-01",
        "feedback": "feedback/nightly-2026-11-01/20261101T120000Z.md",
        "writes": 2,
        "pending": 0,
        "verified": 2
      }
    ],
    "watermark": "2026-11-01T12:00:00Z"
  }
}
```

#### `comments check`

```json
{
  "schema_version": 1,
  "feedback": "feedback/nightly-2026-11-01/20261101T120000Z.md",
  "sound": true,
  "problems": []
}
```

#### `comments gather`

```json
{
  "schema_version": 1,
  "snapshot": "feedback/.snapshots/20261101T120000Z",
  "dry_run": false,
  "items": 3,
  "selected": {
    "nightly-2026-11-01": 2
  },
  "left": 1
}
```

#### `comments launches`

```json
{
  "schema_version": 1,
  "snapshot": "feedback/.snapshots/20261101T120000Z",
  "launches": [
    {
      "batch": "nightly-2026-11-01",
      "feedback": "feedback/nightly-2026-11-01/20261101T120000Z.md"
    }
  ]
}
```

#### `comments settle`

```json
{
  "schema_version": 1,
  "snapshot": "feedback/.snapshots/20261101T120000Z",
  "settled": [
    {
      "batch": "nightly-2026-11-01",
      "feedback": "feedback/nightly-2026-11-01/20261101T120000Z.md",
      "writes": 2,
      "pending": 0,
      "verified": 2
    }
  ],
  "watermark": "2026-11-01T12:00:00Z"
}
```

#### `draft`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "draft": "drafts:nightly-2026-11-01/drafts/20261101T101500Z-layout-save-loses-resize",
  "path": ".follow-ups/tasks/nightly-2026-11-01/drafts/20261101T101500Z-layout-save-loses-resize.md"
}
```

#### `groom check`

```json
{
  "batch": "groom-20261121T090000000Z",
  "problems": [
    {
      "ticket": "followups:item-7",
      "rule": "low-confidence-close",
      "detail": "groom-20261121T090000000Z-1 line 2: a `fixed` verdict closes the ticket, which `low` confidence never does: raise the evidence to `medium` or `high`, or answer `keep` or `unresolved`"
    }
  ],
  "plan": {
    "fixed": [],
    "obsolete": [],
    "duplicates": [
      {
        "ticket": "followups:item-9",
        "status": "backlog",
        "verdict": "keep",
        "confidence": "high",
        "evidence": "src/layout.rs reads the size once since the fix",
        "deciding_commit": null,
        "evidence_paths": [
          "src/layout.rs"
        ],
        "repository": "github.com/example/widgets",
        "commit": "89abcdef0123456789abcdef0123456789abcdef",
        "duplicate_of": "followups:item-3",
        "root_cause": "the layout's save reads the window size twice",
        "left_for_person": null
      }
    ],
    "survivors": [
      {
        "ticket": "followups:item-3",
        "members": [
          "followups:item-3",
          "followups:item-9"
        ],
        "chosen_by": "claimed"
      }
    ],
    "kept": [
      {
        "ticket": "followups:item-3",
        "status": "in-progress",
        "verdict": "fixed",
        "confidence": "high",
        "evidence": "src/layout.rs reads the size once since the fix",
        "deciding_commit": "0123456789abcdef0123456789abcdef01234567",
        "evidence_paths": [
          "src/layout.rs"
        ],
        "repository": "github.com/example/widgets",
        "commit": "89abcdef0123456789abcdef0123456789abcdef",
        "duplicate_of": null,
        "root_cause": null,
        "left_for_person": "it is claimed (Queued or In Progress), which grooming never closes"
      }
    ],
    "unresolved": []
  }
}
```

#### `groom publish`

```json
{
  "batch": "groom-20261121T090000000Z",
  "confirmed": 3,
  "mismatched": [
    {
      "ticket": "followups:item-9",
      "expected": "status category `cancelled`",
      "found": "status category `backlog` (Backlog)"
    }
  ],
  "report": "/work/.onefollowup/groom/groom-20261121T090000000Z/report.md",
  "report_json": "/work/.onefollowup/groom/groom-20261121T090000000Z/report.json",
  "store_operations": [
    {
      "op": "read",
      "source": "followups",
      "destination": true,
      "items": 2
    },
    {
      "op": "update-many",
      "source": "followups",
      "destination": true,
      "items": 4
    },
    {
      "op": "comment-add-many",
      "source": "followups",
      "destination": true,
      "items": 3
    },
    {
      "op": "read",
      "source": "followups",
      "destination": true,
      "items": 4
    }
  ]
}
```

#### `groom prepare`

```json
{
  "schema_version": 1,
  "batch": "groom-20261121T090000000Z",
  "destination": "followups",
  "selected": 2,
  "unchanged": 47,
  "protected": 1,
  "shards": [
    {
      "shard": "groom-20261121T090000000Z-1",
      "task": "/work/.onefollowup/groom/groom-20261121T090000000Z/groom-20261121T090000000Z-1/task.md",
      "verdicts": "/work/.onefollowup/groom/groom-20261121T090000000Z/groom-20261121T090000000Z-1/verdicts.jsonl",
      "tickets": 2
    }
  ],
  "store_operations": [
    {
      "op": "list",
      "source": "followups",
      "destination": true,
      "items": 49
    },
    {
      "op": "comments-since",
      "source": "followups",
      "destination": true,
      "items": 3
    },
    {
      "op": "read",
      "source": "followups",
      "destination": true,
      "items": 2
    }
  ]
}
```

#### `prepare`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "drafts": 3,
  "inputs": 1,
  "answers": "dispositions/nightly-2026-11-01.json",
  "task_template": "tasks/nightly-2026-11-01/task.md",
  "duplicate_searches": [
    {
      "input": "drafts:nightly-2026-11-01/drafts/20261101T101500Z-layout-save-loses-resize",
      "stoplisted": [
        "after"
      ],
      "queries": [
        {
          "text": "layout save loses its size after a resize",
          "kind": "title",
          "hits": 0,
          "retained": 0,
          "capped": false
        },
        {
          "text": "layout",
          "kind": "term",
          "hits": 31,
          "retained": 21,
          "capped": true
        },
        {
          "text": "resize",
          "kind": "term",
          "hits": 2,
          "retained": 2,
          "capped": false
        },
        {
          "text": "layout.rs",
          "kind": "file",
          "hits": 1,
          "retained": 1,
          "capped": false
        }
      ],
      "matching": 1,
      "candidates": [
        {
          "item": "followups:item-17",
          "title": "widgets: a resize must keep the saved layout",
          "status": {
            "category": "done",
            "name": "Done"
          },
          "url": null,
          "root_cause": "resize-keeps-layout",
          "shared": [
            "resize",
            "layout.rs"
          ],
          "capped": [
            "layout"
          ]
        }
      ]
    }
  ]
}
```

#### `publish`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "receipts": "receipts/nightly-2026-11-01.jsonl",
  "outcomes": [
    {
      "index": 0,
      "op": "copy",
      "item": "followups:item-42",
      "outcome": "created"
    }
  ],
  "quota_reads": 3,
  "governor": {
    "waits": [
      {
        "source": "followups",
        "resource": "graphql",
        "seconds": 60,
        "reason": "reserve"
      }
    ]
  }
}
```

#### `settle`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "read_back": 1,
  "confirmed": 1,
  "hook": {
    "argv": [
      "notify",
      "--batch-settled"
    ],
    "exit": 0
  },
  "retired": [
    "outbox/nightly-2026-11-01.json"
  ]
}
```

#### `snapshot`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "drafts": 3,
  "inputs": 1,
  "empty": false,
  "store_operations": [
    {
      "op": "find",
      "source": "drafts",
      "destination": false,
      "items": 3,
      "searches": 1
    }
  ]
}
```

#### `statuses`

```json
{
  "schema_version": 1,
  "destination": "followups",
  "roles": [
    {
      "role": "proposal",
      "category": "backlog",
      "destination_names": [
        "Proposal"
      ]
    }
  ]
}
```

#### `ticket board-status`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "item": "followups:item-42",
  "title": "widgets: a layout save reads one window size",
  "status": {
    "category": "backlog",
    "name": "Proposal"
  },
  "priority": "high",
  "version": "2026-11-01T11:59:58Z",
  "url": "https://github.com/example/widgets/issues/42",
  "comments": [
    {
      "id": "IC_kwDOA1001",
      "author": "alice",
      "url": "https://github.com/example/widgets/issues/42#issuecomment-1001",
      "last_changed": "2026-11-01T11:00:00Z",
      "body": "Seen again after a resize."
    }
  ]
}
```

#### `ticket comment`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "index": 1,
  "write": {
    "op": "comment",
    "item": "followups:item-13",
    "root_cause": "export-dialog-drops-unicode",
    "body": "Additional evidence from follow-up run `nightly-2026-11-01`.\n\nSeen again.\n\n<!-- orchestrator:follow-up-comment run=\"nightly-2026-11-01\" root_cause=\"export-dialog-drops-unicode\" -->\n"
  }
}
```

#### `ticket copy`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "index": 0,
  "write": {
    "op": "copy",
    "ticket": "drafts:nightly-2026-11-01/tickets/layout-save-reads-size-twice",
    "destination": "followups"
  }
}
```

#### `ticket re-estimate`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "index": 2,
  "write": {
    "op": "re-estimate",
    "item": "followups:item-13",
    "priority_estimate": "high"
  }
}
```

#### `ticket relate`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "index": 3,
  "write": {
    "op": "relate",
    "relation": "of-error",
    "from": "drafts:nightly-2026-11-01/tickets/client-acme-e1042",
    "to": "followups:item-7"
  }
}
```

#### `ticket search`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "ticket": "layout-save-reads-size-twice",
  "query": "layout save window size",
  "hits": [
    {
      "item": "followups:item-42",
      "title": "widgets: a layout save reads one window size",
      "status": {
        "category": "backlog",
        "name": "Proposal"
      },
      "url": "https://github.com/example/widgets/issues/42"
    }
  ]
}
```

#### `ticket validate`

```json
{
  "schema_version": 1,
  "batch": "nightly-2026-11-01",
  "ticket": "layout-save-reads-size-twice",
  "sound": false,
  "problems": [
    "the ticket records the field `client`, which `fields` does not declare"
  ]
}
```
.

### As a library

Each command is one function in `onefollowup::api`, taking the resolved settings and the
command's own arguments and returning the typed result the command prints:

```rust,no_run
use onefollowup::api::{self, ConfigShowArgs};
use onefollowup::config::{self, Inputs};

let settings = config::resolve(&Inputs::from_process()?)?;
let shown = api::config_show(&settings, ConfigShowArgs { sources: true })?;
println!("{}", shown.config.destination);
# Ok::<(), onefollowup::Error>(())
```

Settings can also be given directly: `Settings::new(config)` holds a `Config` to the same
rules a file is held to.

## Configuration

Every setting is resolved from, first to last, the first layer that states it winning:

1. **A flag.** `--set <KEY=VALUE>` sets any key (a dotted path for a nested one:
   `--set estimate.raise_at=5`); the most used keys have flags of their own —
   `--destination`, `--drafts-source`, `--store` — which beat `--set` for their key.
2. **An environment variable**, `ONEFOLLOWUP_<KEY>`, a nested key's segments joined by
   `__`: `ONEFOLLOWUP_ESTIMATE__RAISE_AT=5`.
3. **The file** `onefollowup.yaml`, found by `--config <PATH>`, else `ONEFOLLOWUP_CONFIG`,
   else the working directory.
4. **The default**, shipped in the binary
   ([`crates/onefollowup/defaults/onefollowup.yaml`](crates/onefollowup/defaults/onefollowup.yaml)).

A flag's or variable's value is YAML, so `5` is a number, `[a, b]` a list and
`{same: [x]}` a mapping. Mappings merge key by key across layers; a list or a scalar is
replaced whole. An unknown key, a wrongly typed value or a broken rule is refused, naming
the key and the layer that stated it. `config show --sources` names, for each top-level
key, the layer its value came from.

| Key | Default | Meaning |
|---|---|---|
| `store` | the store's own discovery | The onetaskgraph configuration: an `onetaskgraph.yaml`, or the directory holding one. |
| `drafts_source` | `drafts` | The source drafts are recorded in; declare it `visibility: private` in the store to record drafts about private repositories (Private drafts, below). |
| `destination` | `followups` | The source every ticket is copied to unless its kind names another; the store's own routes on that source still apply. |
| `namespace` | `orchestrator` | Prefix of the comment markers (`orchestrator:follow-up-comment`) and the record's metadata key (`orchestrator.follow-up`). |
| `host_label` | the machine's hostname | The host a ticket records as having verified it. |
| `batch_id_pattern` | word characters, `.`, `@`, `+`, `-`, not dots alone | A batch id, matched in full. |
| `statuses` | proposal → `backlog`, accepted → `todo`, deferred → `draft`, queued → `queued`, in-progress → `in-progress`, done → `done`, cancelled → `cancelled` | Each status role mapped to a store status category, with `description`, `protected` (no batch withdraws an item at this role), `selected` (an agent picking up accepted work selects it) and `answered` (people's comments on an item at this role are answered by its batch; proposal and deferred by default). |
| `rubric` | four severity levels, critical → low | Severity `levels` (each with the priority it estimates at and its meaning for users, development and resources), `areas`, `guidance`, `impact_lines`, `no_workaround`, `impact_parts`, `workaround_costs`. |
| `estimate` | `raise_at: 3`, `raise_cap: high`, `line_label: Priority estimate`, `line_template: builtin:estimate-line.j2` | An estimate is raised one level, capped at `raise_cap`, when a root cause fires consistently or has `raise_at` or more occurrences. `line_template` renders the estimate line and `line_label` is the label it is found by (The priority's inputs, below). |
| `task_template` | `builtin:task-template.md.j2` | The verifying agent's task template; a path, or `builtin:` for a shipped file. |
| `persona` | `builtin:persona.yaml` | The verifying agent's persona. |
| `comment_templates` | `evidence: builtin:comment-evidence.md.j2`, `reply: builtin:comment-reply.md.j2` | The visible part of an evidence comment and of a reply, each a path or `builtin:`; onefollowup appends the marker line after it (Templates, below). The default evidence comment, handed its sources, opens with one collapsed block summarised `Additional evidence from follow-up run <batch>.` holding `- Host:`, `- Basis: <origin> at <commit>`, `- Draft: <id>; <source>; transcript: <command>` and `- Witness:` bullets, then the evidence, any `Bears on the ticket:` paragraph and any question; without sources it opens with 0.5.0's line. The default reply is 0.5.0's, byte for byte. |
| `fields` | `{}` | Declared fields (below). |
| `kinds` | one kind, `root-cause` | Ticket kinds (below). |
| `default_kind` | `root-cause` | The kind a record without one is read as. |
| `relations` | `[]` | Relations between kinds (below). |
| `dispositions` | `filed`, `not-reproducible`, `already-fixed`, `too-low-impact` | The disposition catalogue (below). |
| `default_dispositions` | none | The dispositions every kind allows unless it lists its own; none allows the whole catalogue. |
| `routes` | `[]` | Ordered routes (below). |
| `hooks` | none | Commands run at named moments, and the `owner` a context file names (below). |
| `snapshot` | `launchable_for: 24h` | How long a dry run's comment snapshot stays launchable. |
| `groom` | `shard_size: 50`, `comment_overlap_seconds: 3600`, `max_age_days: 30`, `clones: .onefollowup/groom/clones`, `task_template: builtin:groom-task.md.j2`, `persona: builtin:groom-persona.yaml`, `comment_template: builtin:groom-comment.md.j2` | The grooming pass (Grooming, below): how many tickets one shard's task holds (at least 1); how far before a record's `verified_at` a comment is still read as possibly unseen, well past GitHub's roughly 30-second search-index lag; how many days a record stays current (at least 1); where each repository's blob-less clone is kept, a relative path read against the store's directory; the shard task's template and its persona; and the visible part of every comment `groom publish` posts, before the marker line onefollowup appends; each template a path or `builtin:`. |
| `comments` | none | `launch_command`: the argv `comments answer` runs once per launched batch, in onefollowup's working directory and environment, with `{"schema_version": 1, "batch", "feedback", "snapshot"}` on its stdin and, wherever an argument holds one, `{batch}` replaced with the batch, `{feedback}` with the path of the feedback file `comments launches` rendered for it, and `{snapshot}` with the snapshot directory. Its stdout and stderr are not printed; its exit code decides whether the pass settles. Without it `comments answer` is refused. |
| `governor` | `enabled: true`, `floor_percent: 20`, `reserve_factor: 1`, `peak_window_seconds: 300`, `burn_history_seconds: 86400`, `min_peak_per_minute: {graphql: 40, core: 60}`, `sample_seconds: 30`, `max_content_writes_per_hour: 250`, `max_content_writes_per_minute: 40`, `state_dir: $XDG_STATE_HOME/onefollowup/governor` (else `~/.local/state/onefollowup/governor`) | How every destination call is paced against the quota its destination reports (The quota governor, below): the floor as a percentage of each window's limit, the factor on the reserve kept for others, the span others' peak burn is measured over and how long it is remembered, the least rate in points a minute others are ever assumed to spend each resource at (a resource not named, 0), the longest a reading is used before it is taken again, the most content-creating writes every onefollowup process on the host sends one backend kind in any rolling hour and minute (counts, at least 1: no backend reports GitHub's secondary content-creation limit; half its stated 500 and 80, leaving room for a second machine), and the absolute host-level directory every onefollowup process shares the governor's state in (`null` when neither `XDG_STATE_HOME` nor `HOME` names an absolute directory: the state is then the process's alone). `enabled: false` sends every call at once. |

No key says *when* the loop runs; that is the host's schedule.

### Triage as configuration

Triage is declared, not programmed: kinds, the relations between them, dispositions,
ordered routes and command hooks, with today's strategy as the default. There is no rules
engine and no lifecycle policy.

**`fields`** — `{<name>: {description: <text>}}`, every field a ticket may record beyond
the record's own keys (`error_signature`, `client`, …). The verifying agent is shown each
description; any kind may record any declared field; a ticket recording an undeclared
field is refused by `ticket validate` and `check`. Values live in the ticket record (the
store's metadata slot) and in the ticket's visible section; a kind's `projections` can render
any of them into the record's `projected`, which a destination's `metadata_fields` can show on
a board field. A field declares no format.

**`kinds`** — a list, and `default_kind` names one. The verifying agent picks one kind per
ticket.

```yaml
- name: client-error
  description: One client's error.          # shown to the verifying agent
  ticket_template: templates/client.md.j2   # optional; default the shipped template
  render_template: templates/client-body.md.j2  # optional; default the authored body
  checks: [headings, title, {command: [lint-ticket]}]  # optional; default 0.5.0's (Checks, below)
  projections:                              # optional; default none
    client: "{{ fields.client }}"
  evidence_section: Evidence                # the default
  duplicates: {same: [error_code, client]}  # or `agent` (the default)
  priority: {model: estimate}               # or {fixed: high}
  initial_status: proposal                  # the status role a new ticket is copied in
  destination: errors                       # optional; default the top-level destination
  dispositions: [filed, wont-fix]           # optional; default `default_dispositions`
```

`ticket_template` is the shape the verifying agent writes and `ticket validate` holds a ticket
to. `render_template` (a path, or `builtin:`) is what the destination body is rendered from at
`ticket copy` and `publish`, given every datum (Templates, below); omitted, the body is the
authored one with its estimate line, as 0.5.0 wrote it. The shipped `root-cause` kind names
`builtin:ticket-render.md.j2`. `projections` maps a name to a template
string, each rendered with the same data and written as `record.projected.<name>`, so any datum
— a section's text, or one part of it — reaches a board field through the destination's own
`metadata_fields` at path `[projected, <name>]`, with no field written by the agent; omitted, no
`projected` is written, and an authored record supplying it is refused. `evidence_section`
(default `Evidence`) is the section a `same` duplicate's merge stages as its evidence comment's
evidence, with the ticket's record as its sources. A `render_template` or comment template that
names no file and no shipped template, a projection that is not a string or whose name is not
one word, and an empty `evidence_section` are refused by key when the configuration loads.

`duplicates` says how a ticket of the kind is found to be one already filed. `agent` is the
agent searching the destination and judging whether an open ticket states the same thing;
its evidence then goes to that ticket by its id. `{same: [...]}` is exact matching on
declared fields, for kinds whose values are already exact: two tickets are one when every
listed field is equal after trimming whitespace and case-folding, and a ticket with any
listed field empty never merges. `priority: {model: estimate}` is the severity × frequency
estimate over `rubric` and `estimate`; `{fixed: <priority>}` gives every ticket that
priority.

**Duplicate candidates.** A duplicate worded differently shares no sentence with the ticket it
repeats, only a few distinctive words and the failing file, and a destination's text search
matches a query's words exactly. So under a kind whose duplicates the agent judges, `prepare`
searches each input for them itself, inside its one batched search: its title, then one query
per distinctive term — up to 5 words of its title, then of its body's opening paragraph,
longest first, never a word of the shipped stoplist
([`duplicate-stoplist.txt`](crates/onefollowup/defaults/duplicate-stoplist.txt): common English
and words frequent on a follow-up board), the repository's own name, a word with a digit or a
path — and one per file name it names, up to 2, its paths' first. Every follow-up ticket another
batch filed that those queries found, at any status, open or closed, is a **candidate**, ranked by
how many of the uncapped queries found it, then the capped, then by how few items those queries
answered; a query answering more than 20 items is capped: it keeps only 21 of them, so its
reads and candidates stay bounded however crowded the destination, and what only it found ranks
last. The
first 8 are rendered into the task under their draft or input and printed as
`duplicate_searches`; these numbers are onefollowup's constants, not settings. `check` refuses
an answer whose disposition requires a ticket while a rendered candidate of its input is neither
**related** — this batch's evidence comment staged or published on it, under a root cause the
answer names — nor **dismissed** in the answer's `dismissed` (`[{"item", "reason"}]`) with a
reason; an earlier pass's carried answers stand as they are. A second ticket of one root cause
filed by the same batch, or one a concurrent batch files after this batch searched, is never a
candidate: only what the destination held when `prepare` searched is.

**The shipped ticket shape.** The verifying agent writes every part plain
([`ticket-template.md.j2`](crates/onefollowup/defaults/ticket-template.md.j2)): `## Root cause`;
`## Impact`'s labelled parts, the impact values in the record's `impact` alone;
`## The quota governor

Every destination call onefollowup makes, in every verb, is paced by a governor, so a pass
never spends a token it shares with others below their share, and never waits while quota is
ample. It reads what the destination reports through onetaskgraph (`GET /rate_limit` on a
GitHub source, and every response's `x-ratelimit-*` headers); a destination that reports
nothing — local-md, a stdio peer without `quota` — is never held.

- **Where.** At the store, once per destination call before it is sent, and once per backend
  request inside it through onetaskgraph's request gate, so one batched call is paced request
  by request. It only ever waits, never refuses: a batch it paces completes, however long it
  takes. No verb knows it is there.
- **The line.** For each resource (`graphql`, `core`, …) and each rate-limit window of it that
  has not reset, a request estimated to cost `c` goes at once while
  `remaining - own spend since that reading - c >= floor + reserve`. The floor is
  `floor_percent` of the window's limit; the reserve is others' peak burn rate × the seconds
  until that window resets × `reserve_factor`, so it shrinks to nothing as the reset nears and
  a pass may then spend down to the floor. A request's cost is the destination's hint when it
  gives one, else the mean cost observed of a read or a write on that resource (1 before any).
- **Windows.** One token can be answered from two windows of one resource, each with its own
  reset, and `GET /rate_limit` now and then reports the other's figures. Readings are kept per
  window: admission is against the least of the windows not yet past their reset, and a
  remaining that rises inside one window before its reset is ignored.
- **Others' burn.** The fall in remaining between two readings of one window, less
  onefollowup's own spend in between, over the interval. The reserve uses the peak over any
  `peak_window_seconds` within the last `burn_history_seconds`, never less than
  `min_peak_per_minute` — 40 GraphQL and 60 core points a minute, just above the 5-minute peaks
  measured on the shared token (35.6 and 57.1) — so a fresh process is protected before it has
  measured anything, and a quiet hour cannot shrink the reserve to nothing.
- **Waiting.** A held request prints one line on standard error naming the destination, the
  resource, the remaining, the floor and reserve, others' mean and peak burn and the expected
  resume, then re-reads every 30 seconds — that read itself passing the gate — and goes as soon
  as a reading admits it, or the window resets.
- **What it guarantees.** Each request is admitted only against the latest reading, less
  onefollowup's own spend since. One process has at most one metered request in flight, even
  where a destination sends several at once: the next waits for its response's reading, or
  for its store call to end when the response carried none. Every process on the host admits
  against the one host state, which counts every process's cost in flight. So onefollowup's own
  spend can take a window under the floor by at most one request's estimate error per process. When others' spend
  takes remaining under the floor plus the reserve, nothing more is sent until a reading
  admits it. **It does not hold the floor against others**: another machine or consumer of
  the token can cross the floor by its own spend, which no reading shows until after.
- **Content-creating writes.** A request onetaskgraph's gate is told creates content (an
  issue, a comment), in a destination call, goes only while the host's content-creating writes
  to that backend kind in the last minute and the last hour, across every onefollowup process,
  leave room under `max_content_writes_per_minute` and `max_content_writes_per_hour` for every
  item it may carry, and those items are counted as it goes; otherwise it waits — one line on
  standard error — until the oldest counted writes leave the window. It is never refused. The
  caps count items, not requests: a source may batch several comments into one request
  (GitHub's `comment add-many` does), and the gate is told only that a request creates
  content, so a comment batch or a copy sends at most the lesser cap's items in one call, in
  order, and each call's first content-creating request is admitted only with room for all of
  them and counts all of them. A call refused after an earlier one of the same batch wrote
  answers each of its items as failed with that refusal.
- **One state per host.** What the governor learns of a backend kind — its windows, the spend
  every process was admitted and the cost still in flight, others' burn history, observed
  costs and the content-write window — is one state in `state_dir`, one file per backend kind
  (`<kind>.json`, written by nothing but the governor, under a lock beside it), so every
  destination of one kind and every onefollowup process on the host, whatever its work
  directory, share it, and a fresh process starts from the burn estimate others learned. A
  missing, unreadable or other-schema state is no history, never a refusal; the cost in flight
  of a process that is gone is dropped. *Known limit:* two GitHub accounts used from one host
  share one window, each held to caps and a burn estimate that count the other's spend.
- **Reported.** Each verb's result carries `quota_reads` — the readings taken, which no
  `store_operations` entry counts — and `governor`, `{"waits": [{"source", "resource",
  "seconds", "reason"}]}`, `reason` `floor`, `reserve`, `content-per-minute` or
  `content-per-hour`; both are absent when the verb reached no destination.

## Examples`; `## Suggested fix`; the optional `## Rejected fixes` and `## Related tickets`;
and `## Evidence`, last, prose about what happened and nothing the record holds. The shipped
render ([`ticket-render.md.j2`](crates/onefollowup/defaults/ticket-render.md.j2)) ends
`## Impact` with a collapsed `Cost of the workaround` block, its first bullet the `- Workaround:`
line, then a collapsed `Priority calculation` block holding the `- Severity:`, `- Severity with
the workaround:` and estimate lines, all written from `impact`; and opens `## Suggested fix`'s
locations as a collapsed `Contributing locations` block. A fix with one unit is that block then
the unit, whose opening sentence or two says why the change removes the root cause; with two or
more units an overview saying how they fit together comes first. Each block is written as
ai-orchestrator 8bf72a30's `collapsed()` writes it.

**Checks** — `kinds[].checks`, the shape checks `ticket validate`, `ticket copy`, `check` and
`comments check` hold a ticket of the kind to, in order. Each is a built-in, `{check: <name>,
...parameters}` (or its bare name, every parameter at its default), or `{command: [argv...]}`.
Omitted, a kind keeps 0.5.0's: `headings`, `title`, and `impact` for an estimated kind. The
shipped `root-cause` kind names `[headings, title, impact, fix-units, evidence-prose,
witnesses]`. An unknown check, a parameter a check does not take or one of the wrong type is
refused by key when the configuration loads.

| Check | Parameters (default) | Refuses a ticket |
|---|---|---|
| `headings` | none | whose level-2 headings are not the `ticket_template`'s, in its order — a section whose guidance opens `<optional` may be left out — or one of which is empty. |
| `title` | `prefix` (`repository-name`, or `null` for none), `limit` (`120`) | whose title does not open with `<repository name>: `, the last part of the record's `repository`, or is longer than `limit` characters. |
| `impact` | `section` (`Impact`) | whose named section does not carry the rubric's `impact_parts` as `**<part>.**`, in order. The values are `impact`'s, held below whatever the checks. |
| `fix-units` | `section` (`Suggested fix`), `locations` (`Contributing locations`) | whose named section carries no `### <unit> — `<repository>`` subsection, or no non-empty `**<locations>.**` part of one `- <repository> `<path>`: <what it does>` bullet per location before the first unit; with exactly one unit, anything but that part before it; with two or more, no overview paragraph opening the section. Each refusal names the form that applies. |
| `evidence-prose` | `section` (`Evidence`) | whose named section carries a `Host:` or `Basis:` line, a qualified draft id, a 40-character commit or a transcript command — what the record holds. |
| `witnesses` | `required` (`true`) | whose record carries no witness (each is always held to `basis`). |
| `{command: [argv...]}` | — | for which the command, run in the verb's working directory and environment with the ticket on stdin as one JSON document (`schema_version: 1`, `authored_body`, `rendered_body` — `null` when the ticket does not render — `record` and `kind`), exits non-zero or prints anything: each line it printed is a refusal. Exit 0 and no output accepts it. |

**The priority's inputs.** Under every configuration, whatever the templates and `checks`, an
estimated kind's ticket records its impact values in its record's `impact`, one per
`rubric.impact_lines` label: `ticket validate`, `ticket copy` and `check` refuse one with no
`impact`, missing a label, carrying a label the rubric does not declare, or giving a severity —
the first and last labels — that is not a rubric level (and hold the two severities and the
workaround to the rubric's rules). For a ticket this release writes, the estimate at `publish`,
`ticket re-estimate`, the comment pass's settle, the relations rollup and a route's `severity`
read those values from `impact` alone, so a template may show them anywhere, in any syntax, or
nowhere. A record without `impact` — every ticket 0.5.0 wrote — is read by one fixed legacy read:
each label's `- <label>: <value>` line (or Linear's `* `) wherever it sits outside fenced code, a
collapsed block's content included. A bound `ticket copy` or a re-estimate of such a ticket
writes what the read found into `impact`; an unbound ticket must carry `impact` itself. A
legacy body whose lines cannot be read (a label missing or repeated) is estimated from its
`## Impact` as 0.5.0 estimated it and keeps no `impact`.

The estimate line is `estimate.line_template` rendered with `label`, `level`, `mitigated` (the
severity with the workaround), `frequency` (`consistent`, `intermittent` or `null`),
`occurrences`, `raise` (how it was raised, or why not) and `held` (the level its related
tickets' highest holds it at, or `null`): one line that, after an optional `- ` or `* `
bullet, opens with `<line_label>:`, or the estimate is refused. `publish`, `ticket
re-estimate`, the comment pass's settle and the relations rollup find that line by its label
anywhere in a body, outside fenced code and inside a collapsed block alike, and replace it in
place, keeping a `* ` bullet; a body carrying none has only its record and priority updated.
With no `render_template`, `publish` places a line an authored body lacks where 0.5.0 placed
it, after its `## Impact`'s severity with the workaround.

**`relations`** — named, typed edges between kinds, pointing from the more specific ticket
to the more general:

```yaml
- name: of-error
  from: client-error
  to: error
  cardinality: many-to-one         # or many-to-many
  decided_by: {match: [error_code]} # or `agent`
  store: parent                    # the store's parent relation, or `link` (a typed link)
  rollup: occurrences              # or `max`, or `none` (the default)
```

`decided_by: {match}` is deterministic and allowed only between two kinds that both find
duplicates by `same`, its fields exactly the `to` kind's `same` fields and all among the
`from` kind's: the related ticket is the `to` ticket — staged in the batch, or an open item of
its destination — whose values of those fields equal the `from` ticket's, compared as `same`
compares (trimmed, case-folded, an empty value matching nothing). Such an edge is **derived,
never asserted**: `ticket relate` and `check` refuse one staged by hand. No stub is generated:
`prepare` and `ticket validate` report each derived edge whose target does not exist as
`absent_targets` (`{"ticket", "relation", "kind", "fields"}`), the agent writes and stages that
ticket under its kind's template, and `check` refuses a staged `from` ticket whose target
neither exists nor is staged, naming the kind and fields. `agent` is the verifying agent
asserting edges as judgment, each with `ticket relate`.

Every proposed ticket and edge is held to this schema, at `ticket relate` and again at `check`:
a known kind and relation; ends of the relation's `from` and `to` kinds; at most one edge of a
`many-to-one` relation from one `from` ticket, counting the edges the store already holds; and
a `store: parent` edge joining two tickets of one source (a `link` may cross sources). An
edge's identity is its relation and its two ends, so a retry writes nothing for an edge that
exists and receipts it `exists`. `publish` copies a ticket after each new ticket its edges
point to, so its record's `edges` (`[{"relation", "to"}]`) names that ticket's item, then
writes the batch's edges in one `task parent set-many` and one `task link add-many` — a link's
`type` being the relation's name — receipting each. `prepare` reads the edges the tickets it
found already hold in one `task relations`; an item a staged edge names is read too, and
`check` asks for another `prepare` when an edge's item end was found after it. A link
`task relations` reports with `other_end: missing` — one end lost it, as a whole-body rewrite
of a local-md item does — is reported as `missing_links` (`{"item", "relation", "other"}`) and
restored when the batch states that edge again. A configuration with no relations makes none
of these calls.

`rollup` feeds the `to` ticket's priority estimate over its direct edges, staged ones counted:
`occurrences` adds each related ticket's occurrence count to the count the estimate raises on;
`max` keeps the estimate no lower than the highest of its related tickets' own estimates. A
related ticket's own estimate takes nothing of its own related tickets, so a rollup is one
hop. The result is written as any re-estimate is, guarded by the priority onefollowup last
wrote, so a person's priority stands; an existing item the batch's edges reach is re-estimated
in the batch's `update-many`. Closing a ticket closes no related ticket. Refused: `store:
parent` with `many-to-many`; `match` with `many-to-many`; a `rollup` into a kind of fixed
priority.

New evidence goes on the most specific ticket observed: `ticket comment` given the declared
fields observed (`--field`) resolves, of the `same` kinds whose listed fields they fill, the
existing ticket of the most specific — a kind being more specific than the `to` of a
match-decided relation from it — and refuses a comment naming a less specific ticket while
the more specific one exists.

**`dispositions`** — the catalogue every input is given exactly one of, carrying what its
`requires` names:

```yaml
- name: filed
  description: Filed as a new ticket or as evidence on an existing one.
  requires: [ticket]   # any of: ticket, change, reason
```

`requires: [ticket]` is the only statement that an input is filed; `reason` and `change`
file nothing. `default_dispositions` (a nonempty catalogue subset) is what every kind
allows unless it lists its own `dispositions`; with neither, a kind allows the whole
catalogue. The verifying agent assigns every input a kind, including one it does not file,
and an input's kind need not equal the kind of a ticket it names.

**`routes`** — an ordered list matched deterministically against each ticket; the first
match wins:

```yaml
- name: on-call
  match:                       # every key optional
    kind: [client-error]
    severity: [critical]       # the rubric severity the ticket records
    repository: ["github.com/example/*"]
    path: ["src/**"]
    relations: {of-error: {min: 2}}   # edges at either end, staged ones counted
  set:                         # every key optional; applied over the kind's defaults
    status_role: accepted
    priority: urgent
    labels: [incident]
    assignee: alice            # one person, as the destination names people
  command: [rotation, today]   # optional escape hatch
```

`command` runs before the ticket's write with
`{"schema_version": 1, "ticket": <record>, "kind", "severity", "repository", "paths", "route"}`
on stdin; its stdout is empty or a JSON object of `status_role`, `priority`, `labels` and
`assignee` overriding `set`, and a non-zero exit withholds that ticket's write, receipted
as `route-command-failed`. `relations` counts a relation's edges at either end of the ticket,
staged ones included. An existing item the batch re-estimates is matched too: its route's
`command` runs before that item's write with the same document, its stdout's `priority` and
`assignee` overriding the route's `set`, and a non-zero exit withholds that item's writes alone
(its re-estimate, assignee and evidence comment), receipted `route-command-failed`. A route's
`labels`, like its `status_role`, are written only when its ticket is copied: an existing item
keeps its labels unchanged, since onetaskgraph's batched update has no additive label update on
an existing item (its `TaskUpdate` deliberately carries no labels). So an existing root cause a
later batch's third edge makes `widespread-root-cause` takes the route's `high` priority, and
not its `widespread` label.

A ticket has **one assignee**, as a Linear issue does, written only to a source whose store
configuration sets `config.assignees: true` — the destination, and the drafts source the copy
carries it from; `check` refuses a route assigning on a kind where either does not, naming the
route, the source and the setting. A new ticket's assignee lands with the copy that creates it;
an existing ticket's is written in the batch's `update-many`, held with `when_assignees` to
exactly the person its record's `board_assignee` names (none: unassigned), so it lands only
while the item holds exactly what onefollowup last wrote. A write that lands records the person
in `board_assignee`; one the guard holds keeps the person's assignment — a second assignee a
person added included — and is receipted `assignee-held`. The assignee travels in calls the
batch already sends; a configuration that assigns nobody writes no assignee and records no
`board_assignee`.

**`hooks`** — a map from the moment a hook fires to its argv, and the `owner` a context
file names. Two moments exist; any other key is refused:

- `batch_preparing`: `prepare` and `snapshot` run it before they snapshot (a pass `snapshot`
  opened is continued by `prepare` without running it again, except one after a settled pass,
  which `snapshot` does not save), with
  `{"schema_version": 1, "batch": "<id>"}` on stdin. Its stdout is the additional inputs
  needing a disposition, `{"schema_version": 1, "inputs": [{"id", "source", "summary",
  "detail"}]}`, `source` saying where the input came from (such as `budget-overrun`). A
  non-zero exit or any other stdout fails `prepare` or `snapshot`, naming the hook, with nothing
  snapshotted.
- `batch_settled`: `settle` runs it with the settled batch's summary on stdin,
  `{"schema_version": 1, "batch", "writes", "read_back", "confirmed", "pending",
  "needs_attention", "receipts"}`.

A hook runs in the verb's working directory and environment, its argv exactly as configured —
unless an argument holds the placeholder **`{context_file}`**. Then onefollowup writes a
JSON **context file** for that one call, replaces the placeholder with the file's path
wherever it appears, and removes the file once the hook exits; stdin is unchanged. The file is
an instance of an engine's human-item phase context — onepipeline 0.110.2's `PhaseContext`
(its `schemas/human-items.schema.json`), read by `onepipeline human-items raise --phase <P>
--context <FILE>`, which refuses any member it does not know — and
[`crates/onefollowup/schemas/hook-context.schema.json`](crates/onefollowup/schemas/hook-context.schema.json)
declares it:

- `owner`: `{"session", "run", "plan"}`, that release's `HumanItemOwner`, from `hooks.owner`
  (`run` and `plan` `null` when not given);
- `dedup_key`: `<namespace>.follow-up/<batch>/<hook>`, the same every time the batch runs the
  hook, so settling a batch again names the same item;
- `tickets`: one `{"id", "url", "priority"}` — that release's `Ticket`, in the store's
  priority vocabulary — per ticket the batch's receipts say it created or updated, in the order
  the journal first names each, read in one store read: `url` is the URL the destination states,
  else a `file://` URL of the file a source keeps it in, else its id;
- `vars`: the hook's stdin document but for its `schema_version`, each member as text (a
  string as itself, any other scalar as its JSON spelling, a list or mapping as compact JSON),
  which an engine's title and body templates expand as `{{ batch }}`.

**`hooks.owner`** — `{session, run, plan}`, `session` required and the others optional; each
value is a literal, or one whole `${NAME}` read from the verb's environment when it runs. A hook
naming `{context_file}` without `hooks.owner` is refused when the configuration loads; a
variable that is unset or empty refuses the verb before it changes anything, naming the
variable (exit 2). A hook naming no placeholder is handed exactly what it was before, and no
file is written.

A worked example: the configuration below has an engine raise its `follow-up-review` phase —
a `ticket_review` hook of its `human_items` configuration — after every settle:

```yaml
host_label: build-host-1
hooks:
  batch_settled: [onepipeline, human-items, raise, --phase, follow-up-review, --context, '{context_file}']
  owner:
    session: follow-ups@build-host-1
    run: ${FOLLOW_UP_RUN}
```

A worked example: the context file a settle of a batch that updated one ticket and created
another hands it, its store at `/srv/follow-ups`:

```json
{
  "owner": {
    "session": "follow-ups@build-host-1",
    "run": "run-2026-11-08",
    "plan": null
  },
  "dedup_key": "orchestrator.follow-up/nightly-2026-11-08/batch_settled",
  "tickets": [
    {
      "id": "followups:widgets-the-theme-resets-after-an-update",
      "url": "https://tracker.example/widgets/issues/7",
      "priority": "medium"
    },
    {
      "id": "followups:widgets-a-resize-flickers-the-toolbar",
      "url": "file:///srv/follow-ups/followups-root/tasks/widgets-a-resize-flickers-the-toolbar.md",
      "priority": "medium"
    }
  ],
  "vars": {
    "batch": "nightly-2026-11-08",
    "writes": "2",
    "read_back": "0",
    "confirmed": "0",
    "pending": "0",
    "needs_attention": "false",
    "receipts": "/srv/follow-ups/.onefollowup/receipts/nightly-2026-11-08.jsonl"
  }
}
```

`crates/onefollowup-e2e/tests/e2e/hook_context.rs` runs this configuration and holds the file its
settle writes to this example.

### Templates

Every ticket-facing template — a kind's `ticket_template` and `render_template`, both
`comment_templates`, each of a kind's `projections`, and the task template's ticket example —
is a Jinja template (minijinja) given the same data:

| Variable | What it is |
|---|---|
| `rubric` | The configured rubric (`levels`, `areas`, `guidance`, `impact_lines`, `no_workaround`, `impact_parts`, `workaround_costs`). |
| `estimate` | The configured `estimate` (`raise_at`, `raise_cap`, `line_label`, `line_template`). |
| `fields` | For a ticket, each declared field it records mapped to its value; otherwise each declared field mapped to `{description}`. |
| `declared_fields` | Every declared field mapped to `{description}`, with or without a ticket. |
| `kind` | The ticket's kind as configured (`name`, `description`, …); for a comment, the kind of the ticket it is on. |
| `batch` | The batch rendering. |
| `destination` | The source the ticket is copied to, or the comment's item is on. |
| `namespace` | The configured `namespace`. |
| `ticket` | For a ticket: `title`, `slug`, `body` (authored, verbatim), `sections`, and `section(name)` (the section named, or none). Each section is `{name, content, lead, parts, lines, subsections}`: `content` its text trimmed, `lead` its text before the first `**Label.**` part, `- Label: value` line or `###` subsection, `parts` every `**Label.**` part of the section, those inside its subsections included, as `{label, text}` (the text, every paragraph of it, running to the next part, the next `###` subsection or the end of the section), `lines` every `- Label: value` line as `{label, value, text}` (`text` the whole line), and `subsections` every `###` subsection as `{heading, content}`. Fenced code is never read as any of these. |
| `record` | For a ticket: every key of its record as it will be stored — `projected` included once the projections are rendered, for the render template. |
| `estimate_line` | For a ticket: `{level, line, mitigated, frequency, occurrences, raise}` — the estimate's level, the estimate line `estimate.line_template` renders (`null` for a fixed-priority kind), the severity with the workaround, `consistent`/`intermittent` or `null`, the occurrences counted, and how the estimate was raised or why not. The authored `body` carries no estimate line; a render template places `estimate_line.line` itself. |
| `comment` | For a comment: `batch`, `root_cause`, `evidence`, `bears_on`, `question` (each `null` when not given) and `sources` (`host`, `basis`, `drafts`, `witnesses`, `draft_sources`, `transcripts`, or `null`); for a reply, `answers`, `verdict`, `url`, `author` and `response` instead of the evidence's parts. The reply shape a feedback file quotes is rendered once per verdict, its `response` the placeholder `<your reply, in your words>`. |

Every template may call **`collapsed(summary, content)`**, which writes one expandable block in
the authored form onetaskgraph's README states ("Expandable blocks"): `<details>` alone,
`<summary>…</summary>`, a blank line, the content, a blank line, `</details>`. The store writes
that form as each destination's own (Linear's `+++`). So a host's `render_template`, comment
templates and `projections` put any datum in the body, in a collapsed block, on a board field,
or nowhere, while the record keeps every value.

Only the renderer writes a block and only onefollowup writes a marker: under every
configuration, a ticket's authored title or body, and every part handed to `ticket comment`
(the evidence, the bears-on paragraph and the question), carrying a `<details>` or `</details>`
line outside fenced code, or a comment-marker line, is refused before anything is staged. A
comment's text is its template rendered, a blank line, then the marker as its last line; a
template never writes the marker. A comment's owner and kind are read from that marker alone,
so 0.5.0's comments (an opening line, the text, the marker), those opening with a collapsed
block, and those any configured template rendered are each counted as the occurrence of, and
owned by, the batch and root cause their marker names.

[`examples/replaced-shape`](examples/replaced-shape) replaces all four templates, and
`crates/onefollowup-e2e/tests/e2e/template_data.rs` prints every variable above through them.

## Examples

[`examples/`](examples) holds five worked configurations, each directory an
`onefollowup.yaml`, the `onetaskgraph.yaml` it relies on, and the templates and scripts it
names. `crates/onefollowup-e2e/tests/e2e/examples.rs` runs one batch of each through the built
binary, its sources replaced by local-md sources of the same names, to the outcome stated here.

**[`replaced-shape`](examples/replaced-shape)** — the default `root-cause` kind with its ticket
shape, destination body and both comments the host's own, in YAML and templates alone. Its
`render_template` shows the record's host, basis and witnesses in a `## Provenance` section,
collapses the workaround's cost and the contributing locations, and shows no estimate line; its
agents record the impact values in `impact` alone, and its `checks` leave `fix-units` out, its
fix one paragraph and its locations;
its evidence template puts the evidence first and lists its sources (host, basis, each draft with
its source and transcript, each witness) in the open; its reply puts the response first; its
`projections` render the root cause's text and the `Who is affected` part of `## Impact` into
`projected`, which its `onetaskgraph.yaml`'s `metadata_fields` show on board fields. Batch: one
draft filed as a ticket carrying a witness; a later batch's evidence comment on it, with its
sources, bears-on paragraph and question, and its re-estimate; a third batch's evidence and
re-estimate counting three occurrences, which move the item's priority from `medium` to `high`
with no estimate line in the body; the first batch's next pass counting those comments as
occurrences and re-copying the changed ticket, its projections updated. Where: `followups`.

**[`single-root-cause`](examples/single-root-cause)** — one root-cause kind and no relations:
the configuration an orchestrator repository uses to keep today's behaviour, every input given
the `root-cause` kind. Batch: two drafts name the root cause `stale-cache-lease`, a third names
one already on the board, and the `batch_preparing` hook
([`scripts/follow-up-budget-inputs.sh`](examples/single-root-cause/scripts/follow-up-budget-inputs.sh))
adds one over-budget change the agent cannot reproduce. Filed: one new `root-cause` ticket
naming both drafts, at Proposal, its priority estimated; one evidence comment on the existing
ticket; the input disposed `not-reproducible`. Linked: nothing. Where: every ticket on
`followups`, or wherever that source's own routes send it. The `batch_settled` hook raises the
batch's follow-up review through `onepipeline`, handing it the context file (`hooks`, above)
owned by the session `${FOLLOW_UP_SESSION}` names.

**[`symptoms-and-root-causes`](examples/symptoms-and-root-causes)** — a team on Linear:
symptom tickets, one sub-issue per client decided by match, and root causes linked to symptoms
many-to-many by the agent, with rollups. Batch: drafts report `TIMEOUT-42` for clients `acme`
and `globex`, and `PAY-7` for `acme`; the agent judges that the same stale cache lease causes
both. `prepare` tells the agent the two symptom targets are absent, and it writes and stages
them. Filed: the root cause `stale-cache-lease`, symptoms `TIMEOUT-42` and `PAY-7`, and
client-symptoms `TIMEOUT-42/acme`, `TIMEOUT-42/globex` and `PAY-7/acme`. Linked: each
client-symptom a sub-issue of its symptom (derived from the `same` field `error_signature`,
never asserted, while the root cause, whose duplicates the agent judges, is only ever linked by
the agent); each symptom `caused-by` the root cause, as two edges the agent asserted, each also
shown as a Linear `related` relation through the source's `link_types`. Rollups: each symptom
counts its client-symptoms' occurrences; the root cause's estimate is no lower than its highest
symptom's. A later draft for `TIMEOUT-42` and `acme` adds evidence to that client-symptom, the
most specific ticket observed (`ticket comment --field error_signature=TIMEOUT-42 --field
client=acme`). When a third symptom is linked, the root cause matches `widespread-root-cause`;
linked in a later batch, the existing root cause takes its `high` priority and keeps its labels
(a route's labels are written when its ticket is copied).
Where: everything on the Linear team `ENG`. A GitHub variant needs `sub_issues: parent` on a
board of its own, since a parent there is a sub-issue.

**[`typed-destinations`](examples/typed-destinations)** — bugs, improvements and security
findings on different destinations, critical bugs escalated by a notify command. Batch: a
critical bug, a high-severity bug, an improvement and a security finding; `check` would refuse
the security input disposed `too-low-impact`, which its kind does not allow. Filed: the critical
bug on `bug-tracker` at urgent, labelled `escalated`, after
[`scripts/notify-oncall.sh`](examples/typed-destinations/scripts/notify-oncall.sh) received it
on stdin, paged through the team's `oncall` CLI and printed `{"assignee": "<this week's on-call
display name>"}`, so it is assigned to that person; the high bug on `bug-tracker`, labelled
`bug`, at its estimate, assigned to the person `jane.doe` (a Linear display name); a later batch
that finds a person reassigned either ticket leaves that person's choice; the improvement on
`product-triage` at low; the security finding on `security-tracker`, merged with any open ticket
of the same `error_signature` (the bug and improvement duplicates being the agent's judgment).
If the notify command exits non-zero only that ticket's write is withheld, receipted
`route-command-failed`, and `settle` records it pending so a retry stays visible. Linked:
nothing. The assignee travels from the drafts source to `bug-tracker`, so both set
`config.assignees: true` — the drafts source too, or `check` refuses the routes that assign.

**[`inline-fixes`](examples/inline-fixes)** — small certain fixes land as `fixed-inline`,
allowed only for the kind this repository's own change can fix; everything else is proposed
under a fixed low priority. Batch: one `root-cause` draft fixed in the run's own change, one
other `root-cause` draft and one `external-defect` draft. `external-defect` lists no
`dispositions`, so it inherits `default_dispositions`, which omit `fixed-inline`. Filed: nothing
for the first, disposed `fixed-inline` naming its change (and `check` refuses that disposition
with no change named, and on an `external-defect` input); one ticket for each of the others on
`backlog`, at Proposal, priority low. Linked: nothing.

## The comment pass

People answer follow-up tickets on the destination — a question, a correction, a reason a
ticket is wrong — and each comment is answered once, by the batch that owns the ticket, with any
change it calls for made without overwriting what a person edited meanwhile. The pass is four
verbs, its destination work three store calls:

1. **`comments gather`** makes one comments-since listing of each destination, narrowed to this
   host's items (`<namespace>.follow-up/host` is `host_label`) at a status whose role is
   `answered`, from the destination's watermark (none: every comment). Who owns each item, and
   what onefollowup last wrote to it, come from this host's own records — each batch's receipts
   and its tickets in the drafts source — never another destination read. Each comment is
   selected for its owning batch or left out for one reason, tried in order: no batch here filed
   the item; its batch is no batch id; the comment is marked as any batch's own; a bot wrote it;
   a reply of any batch answers it as it now stands (a reply answers the version its gathering
   quoted, so a comment edited after it is answered again). Under a relation with a `rollup`,
   gathering also reads the selected items' relations in one `task relations` and their related
   tickets in one read; a configuration with no rollup makes neither call. The snapshot,
   `feedback/.snapshots/<stamp>/`, holds `snapshot.json` (what was selected and left out, and
   why), `gathering.json` (the listing, what onefollowup last wrote to each selected item, the
   rollup reads) and `last-written/`, one readable file per selected item, labelled as what
   onefollowup last wrote and not the destination's current text.
2. **`comments launches`** claims the snapshot once (`launched.json`) and writes each selected
   batch's feedback file, `feedback/<batch>/<stamp>.md`: the task template rendered with `mode`
   `feedback` (the shipped one's `task-template/feedback.md.j2`), quoting each comment under its
   anchor with the reply opening and markers its reply carries. The answering agent changes the
   quoted issues' tickets in the batch's tickets directory — `status: cancelled` withdraws one —
   posts one reply per comment through the store, and writes its account,
   `feedback/<batch>/<stamp>.responses.json`. Each batch's tickets are kept as they stood at the
   launch, under the snapshot's `tickets/`, so what the agent changed is known.
3. **`comments check`** holds the account to the snapshot: one response per quoted comment, in
   order, each on the issue it was quoted on and owned by the batch, with its reply's id; and
   the batch's tickets to changing only quoted issues' tickets, each still sound.
4. **`comments settle`** makes one batched read of every item a sound account quotes and every
   item `pending.json` names, with their comments, verifying each reply there. It then sends, in
   one `update-many`, every change: a changed ticket's title, content, record and repositories,
   the re-estimate its comments and confirming replies give (a rollup's related tickets
   included), guarded by the priority onefollowup last wrote, and a withdrawal's status while
   the item is at a status that is not `protected` — each item's write held to the version the
   read returned. A write is **withheld** into `pending.json` where the read shows the item
   differs from what onefollowup last wrote to it (its title, its content less the estimate
   line, or its record less what onefollowup's own writes move), so a person's edit — before
   the gathering or after it — stands. An entry `pending.json` already held is cleared once the
   read shows it landed, retried in the same `update-many` once the item again holds what
   onefollowup last wrote, and otherwise kept. A reply the read does not show is recorded there
   too. A settle with anything pending, or an unsound account, says `needs_attention`. Landed
   writes are kept in step in the drafts source.

**The watermark moves only in settle**, to the listing's own watermark — the source's time set
back by its consistency window — and only for an unscoped snapshot whose every selected batch
settled a sound account and whose every reply was verified. So a comment no verified reply
answers is listed again by the next pass, and one a verified reply answers is left as answered.
A pass scoped by `--item` or `--batch` reads every comment of its items and moves nothing.

**`comments answer`** is the pass in one verb, so a host keeps no state between its steps: it
gathers unscoped, launches that snapshot, runs `comments.launch_command` as each batch's
answering dispatch, and settles only when every one exited 0. A failed batch settles nothing
and moves no watermark, so the next `comments answer` gathers its comments again.

## Grooming

`groom prepare` and `groom check` keep a destination's open tickets true, current and unique:
prepare, then one verifying agent per shard, then check until it passes, then `groom publish`
sends the plan. **The agent only judges**, by filling a verdict record per ticket:
whether the ticket is fixed or obsolete on `main`, and whether it shares one root cause with
another open ticket. Selection, checking that judgement's evidence, clustering, survivors and
protection are code.

**The batch**, `.onefollowup/groom/<batch>/` (`<batch>` is `groom-<UTC stamp>`), holds
`batch.json` (what prepare listed, fetched and selected, each ticket's protection, digest and
overlap comments, and `verified_at`), `snapshot.json` (every open ticket's id, title, status,
URL, repositories and content, for duplicate detection), one `<batch>-<k>/task.md` per shard —
`groom.task_template` rendered with its tickets and their comments, each repository's clone path
and fetched `main` commit, the snapshot's path, the verdict schema and the rules below — and the
`verdicts.jsonl` beside it the agent writes. A passing check writes `plan.json`; publish writes
`receipts.json`, `report.md` and `report.json`, and retires every other file of the batch. A repository
`host/owner/name` is cloned from `https://host/owner/name.git`; git's own
`url.<base>.insteadOf` points that elsewhere.

**Incremental selection.** A ticket is selected when it holds no `groom` record, or one of a
schema this does not read; its digest differs from the one recorded; its repository's `main`
moved since the recorded commit and the diff touches one of its `evidence_paths` (a path, or a
directory holding a changed path) — or moved at all when it has none, or no longer holds that
commit; its record is older than `groom.max_age_days`; `--full` is given; or a comment on it
that carries no grooming marker (a line `<!-- <namespace>:follow-up-comment … kind="groom" …
-->`, the line every comment `groom publish` posts ends in) is new activity against its own record:
created or edited later than its `verified_at`, or inside the overlap (later than `verified_at`
less `groom.comment_overlap_seconds`) with its id absent from the record's `overlap_comments` or
held there with a different last-edited time. An older comment is compared by time, so is never
new activity. A `protected` count is the selected tickets at a claimed status.

**The comment watermark** is derived on every pass and never stored: the `comments_since` read
starts at the earliest `verified_at` among the open tickets holding a record, less the overlap,
floored at `now - groom.max_age_days - overlap`, since an older record selects its ticket anyway.
So a ticket whose record did not move holds the read's start back until it does, and no comment
activity on an open ticket is passed over unless a published record of that ticket was taken
after it. `verified_at` is when prepare's comment read began, never the publish time, so a
comment made or edited while the batch is verified, checked or published selects the ticket next
pass.

**The verdict record**, one JSON object per line of a shard's `verdicts.jsonl`, `schema_version`
1, every key required and no other read: `ticket` (an id prepare listed in the shard); `verdict`
(`keep`, `fixed`, `obsolete` or `unresolved`); `confidence` (`high`, `medium` or `low`);
`evidence` (non-empty text); `deciding_commit` (a 40-hex commit for `fixed` and `obsolete`, else
`null`); `evidence_paths` (repository-relative paths, possibly `[]`); `duplicate_of` (a ticket id
of the snapshot, or `null`); `root_cause` (one line, set exactly when `duplicate_of` is);
`seam_repository` (`host/owner/name`, or `null`).

**Check's rules**, each problem naming its `rule` and `ticket`:

| Rule | Refused when |
|---|---|
| `verdict-record` | A line is not one JSON object. |
| `schema-version` | Its `schema_version` is not 1. |
| `unknown-key` | It carries a key the record does not have. |
| `missing-field` | It leaves out a key (`null` is how a nullable one is left empty). |
| `invalid-value` | A value is outside its enum or shape: an unknown `verdict` or `confidence`, a `deciding_commit` that is not 40 hex, a path that leaves the repository, a `seam_repository` that is not `host/owner/name`. |
| `evidence-required` | Its `evidence` is empty. |
| `deciding-commit` | A `fixed` or `obsolete` verdict names no `deciding_commit`, or another verdict names one. |
| `root-cause-required` | It names `duplicate_of` and no `root_cause`. |
| `duplicate-of-self`, `duplicate-of-unknown` | `duplicate_of` names the ticket itself, or no open ticket of the snapshot. |
| `unknown-ticket` | It names a ticket its shard does not list. |
| `missing-verdict`, `duplicate-verdict` | A shard's ticket has no verdict record, or more than one. |
| `low-confidence-close` | A `fixed` or `obsolete` verdict is `low` confidence. |
| `commit-not-on-main` | A `fixed` or `obsolete` verdict's `deciding_commit` is not an ancestor, in its clone, of the `main` commit prepare fetched for any of the ticket's repositories. |
| `claimed-closed`, `claimed-folded` | A plan closes, or folds as a duplicate, a ticket at the `queued` or `in-progress` role's status. |
| `todo-folded` | A plan folds a ticket at the `accepted` role's status (Todo). |
| `todo-close-below-bar` | A plan closes a Todo ticket short of definitely outdated: a `high`-confidence `fixed` or `obsolete` verdict whose `deciding_commit` is verified an ancestor of the fetched `main`. |
| `plan-record` | `plan.json` is not this batch's plan, or plans a ticket the batch did not list. |

**The plan.** A ticket at a non-claimable status (the proposal or deferred role's, or one no
role maps) may be closed or folded. A Queued or In Progress ticket is never closed or folded,
and a Todo ticket never folded and closed only when definitely outdated; any of them may survive
a cluster, and with a closing or duplicate verdict it is kept, `left_for_person` saying why, with
its verdict and evidence. Duplicate links are unioned into clusters, which members closing as
`fixed` or `obsolete` leave. Each cluster's survivor is, in order: claimed (Queued or In
Progress), then Todo, then a member in the repository most members name as `seam_repository`,
then the most evidence (the most evidence paths, then the longest evidence), then the oldest;
any survivor the agent implies is ignored. A member this batch did not select is ranked by
what `snapshot.json` holds of it, its repositories and creation time, and is never folded. Everything not closed or folded is `kept`, and an `unresolved` one is also listed under
`unresolved`. **A plan is input**: the `plan.json` a batch holds when check runs is what publish
sends, so check holds it to the protection rules beside the plan it computes, and refuses one
that breaks them without writing over it; once it is removed or corrected, check passes and
writes its computed plan.

**The `groom` record** is `groom publish`'s to write, through `onefollowup::formats::groom`:
`<namespace>.groom` (`orchestrator.groom` by default) set to `{"schema_version": 1, "batch",
"verified_at", "verdict", "repository", "commit", "evidence_paths", "content_digest",
"overlap_comments": [{"id", "edited_at"}]}`. `content_digest` is `sha256:<hex>` over the title
and the content as the store reads it, without its metadata block, so recording never makes a
ticket look changed; comments are not in it. `overlap_comments` holds each comment, not
grooming's own, that prepare read for the ticket created or last edited inside the overlap
before `verified_at`, by id and last-edited time (created time when never edited).

**Publish.** `groom publish --batch <B>` sends what the batch's `plan.json` holds, refusing (exit 2)
a batch no passing check planned. Before its first store operation it writes `receipts.json`:
every ticket of the plan with what publish does with it, the `groom` record its write carries
(its `verified_at`, `content_digest` and `overlap_comments` taken from the batch), and whether its
verdict is **definitely outdated** — `fixed` or `obsolete` at `high` confidence, its
`deciding_commit` verified an ancestor of the fetched `main` in its clone. From then on
`receipts.json` and `plan.json` are all a re-run reads.

1. The batch is admitted at the store's chokepoint (`Store::admit_batch`, where the quota
   governor waits until its estimate admits the status read and the closes together — reserving
   nothing, each request still admitted on its own as it is sent), then **one batched status
   read** of the tickets to close or fold, so no governor wait falls between that read and the
   closes.
2. **One `update-many`.** A ticket that read finds at a non-claimable open status (the
   `proposal` or `deferred` role's, or one no role maps: Draft, Backlog, Unknown) is closed:
   `fixed` at the `done` role's status, `obsolete` at the `cancelled` role's, a duplicate by the
   store's duplicate close (`cancelled` with a `duplicate` link to its survivor; on GitHub, closed
   as `DUPLICATE` and marked on the survivor's timeline). A ticket still at the `accepted` role's
   status (Todo) is closed only on a definitely-outdated verdict, and never folded. Each close is
   held to the version the status read found. The same call sets every verified ticket's
   `groom` record through the store's metadata set, which keeps every other key.
3. **One `comment add-many`** built from that write's receipts: an evidence comment on each
   ticket shown closed, then each survivor's **consolidation comment**, once every duplicate of
   its cluster is settled, carrying each closed duplicate's distinct evidence and link, naming
   only the duplicates shown closed, and saying when the survivor is Todo, Queued or In
   Progress. A survivor with none closed gets none.

**Left for a person.** Any other ticket that read finds — Todo short of definitely outdated,
Queued, In Progress, no longer open, or changed after the read so its held close is refused —
gets no comment, status, duplicate link or `groom` key, is receipted as left, and keeps its older
record, so the next pass selects it again. Kept tickets get no comment, and no body text or other
metadata key changes. `report.md` and `report.json` list every ticket with a closing or duplicate
verdict that check or publish left open — a Todo duplicate, one below `high` confidence and a
skipped duplicate included — with its status, verdict, confidence, deciding commit, evidence and
any survivor. Grooming closes only tickets nobody can claim, or Todo tickets definitely
outdated, and needs no undo. Accepted residuals, which nothing reopens: on a destination that
cannot hold a write to its version, a Backlog ticket moved to Queued or In Progress by hand
between the status read and its close is closed, and a claim made on a definitely-outdated Todo
ticket in that moment is overwritten.

**A re-run reads the receipts** and sends only what none confirms. A close no receipt confirms —
never sent, refused, or its answer lost — is re-read in the same order before the writes still
sent, and left for a person unless that read still admits it; a read finding the ticket at its
planned close holding this batch's record confirms a close whose answer was lost. The re-run's
read also reads the comments of every ticket owed one, so a comment whose answer was lost is
receipted rather than posted again: a closed ticket gets its comment once, a survivor its
consolidation comment once. Every close is sent before any comment about it, so no ticket is
ever open with a comment saying it was closed, and a closed ticket lacks its comment only until
the batch or its re-run finishes. A record is written only in the `update-many` carrying its
ticket's outcome, so a batch never published moves no record, an interrupted or refused publish
moves exactly the receipted tickets', and a ticket whose write failed never gets a newer record
from a later item.

**The read-back**, one batched read of every ticket changed, holds each closed ticket to its
status category, duplicate link, comment and record, each recorded ticket to its record, and each
survivor to its comment; a ticket left for a person is expected unchanged and never read as a
mismatch. A closed ticket's issue closing, beyond its card, rests on the store: onetaskgraph's
terminal status write closes the issue, and its duplicate close reads `cancelled`. A read reports
one status per ticket, so the read-back cannot see an issue left open under a terminal card.

**Grooming comments.** `groom.comment_template` renders the visible part of each, with `comment`:
`batch`, `verdict` (`fixed`, `obsolete`, `duplicate` or `survivor`), `ticket`, `title` and `url`;
on a closed ticket `confidence`, `evidence`, `deciding_commit`, `repository`, `commit`,
`evidence_paths`, `root_cause` and, for a duplicate, `survivor` (`ticket`, `title`, `url`); on a
survivor `status` (its status name), `claimed_as` (`todo`, `queued`, `in-progress` or none),
`duplicates` (each closed one's `ticket`, `title`, `url`, `root_cause` and `evidence_paths`) and
`evidence` (their distinct evidence). onefollowup appends a blank line and the marker
`<!-- <namespace>:follow-up-comment run="<batch>" kind="groom" verdict="<verdict>" -->`, which
names no root cause, so `comments gather` leaves a grooming comment out as maintenance, never as
a person's to answer. Every comment a run could post is rendered before its first store
operation, so a template that cannot render, or writes a marker itself, is refused (exit 2)
before any ticket is closed.

## Record formats

Every record is declared once as a Rust type in `onefollowup::formats` with its parse and
render, and each reads and writes **only its current schema**, refusing any other with an
error naming the format, the schema found and the schema it reads. There is no migration.
The formats are those on boards and disks today, so nothing already written is rewritten;
the batch id stands where they say `run`.

| Format | Schema | Where |
|---|---|---|
| Ticket record | 14 | The ticket's `<namespace>.follow-up` metadata. Optional keys onefollowup adds within 14, each omitted when empty: `draft_sources` (qualified draft id → `author <kind>`, then `; node <node>` and `; member <member>` where the draft names them; written by onefollowup from each draft, after `drafts`), `transcripts` (qualified draft id → the command that reads its turns; written by onefollowup, after `draft_sources`), `witnesses` (`["<origin> <path>:<line> at <commit>"]`, the origin a key of `basis` and the commit the one `basis` holds for it; written by the verifying agent, after `basis`), `kind`, `fields`, `edges` (`[{"relation", "to"}]`), `board_assignee`, `impact` (rubric impact-line label → value; written by the verifying agent, required of an estimated kind's ticket, the one input its priority is estimated from, and kept whole; a bound copy or re-estimate of a record without it writes what the legacy read of its body found) and `projected` (projection name → rendered value; written by onefollowup alone at `ticket copy` and `publish`, and refused in an authored record). The names, shapes and order of `draft_sources`, `transcripts` and `witnesses` are those of the schema-15 records an orchestrator's `followups` board held, relabelled 14, whose `metadata_fields` read them: never rename one. |
| Draft record | 1 | A draft's `<namespace>.follow-up-draft` metadata. |
| Comment markers | — | `<!-- <namespace>:follow-up-comment run="…" root_cause="…" -->`, the last line of an evidence comment or (with `kind="reply" answers="…" verdict="…"`) a reply, which alone names the comment's owner and kind; a grooming comment's `<!-- <namespace>:follow-up-comment run="…" kind="groom" verdict="…" -->`, which names no root cause; the quoted-comment and feedback anchors. |
| Receipt journal | 3 | `receipts/<batch>.jsonl`, one line per write outcome. Two line kinds onefollowup adds within 3, written only under relations or an assignee: `relate` (an edge's `relation`, `store`, `from`, `to`, `to_card`, `derived`, `action` of `written`, `exists` or `failed`, and its `error`) and `assign` (the `assignee`, the `when` it was held to, `action` of `written`, `held` or `failed`, and its `error`, `assignee-held` when a person's assignment stood). |
| Dispositions | 2 | `dispositions/<batch>.json`; each entry may carry `kind`, which `check` requires, and from 2 `dismissed` (`[{"item", "reason"}]`, the duplicate candidates it dismisses), omitted when empty. Schema 1, the same without `dismissed`, is still read, and written back as 2. |
| Carried dispositions | 2 | `dispositions/<batch>.carried.json`; its entries are the dispositions' own, and schema 1 is still read. |
| Responses | 3 | `feedback/<batch>/<stamp>.responses.json`. |
| Snapshot | 2 | `feedback/.snapshots/<stamp>/snapshot.json`, with its `launched.json` (1). An item's `snapshot_id` names its file under the snapshot's `last-written/`, and its `version` the version its batch's receipts last recorded. |
| Gathering | 1 | `feedback/.snapshots/<stamp>/gathering.json`, new in onefollowup: each destination's comments-since listing, what onefollowup last wrote to each selected item, and — under a rollup — the relations and related tickets read. |
| Watermark | 1 | `feedback/.watermark-<board>.json`. |
| Pending writes | 1 | `feedback/pending.json`. |
| Existing tickets | 1 | `existing-tickets.json`. |
| Outbox | 1 | `outbox/<batch>.json`: the staged writes `publish` sends. |
| Groom record | 1 | A groomed ticket's `<namespace>.groom` metadata (Grooming, above), keyed `schema_version`. |
| Verdict record | 1 | `groom/<batch>/<shard>/verdicts.jsonl`, one per line, keyed `schema_version`. |
| Grooming batch, snapshot and plan | 1 | `groom/<batch>/batch.json`, `snapshot.json` and `plan.json`, onefollowup's own working files, keyed `schema_version`. |
| Grooming receipts and report | 1 | `groom/<batch>/receipts.json`, what `groom publish` sent and heard back, and `report.json`, what it did, beside `report.md`; keyed `schema_version`. |
| Governor state | 1 | `<governor.state_dir>/<backend kind>.json`: the quota governor's host-level state (The quota governor, above). A file of another schema, or one that cannot be read, is read as no history and replaced, never refused. |
| Batch state | 1 | `state/<batch>.json`: onefollowup's own working file — the input set, prepare's reads and each publish. An optional member onefollowup adds within 1: `relations` (prepare's `task relations` read of the tickets it found), omitted when empty; and on each input, `author` (a draft's source, as `draft_sources` holds it) and `transcript`, each omitted when absent. |

## Development

`just bootstrap` from a clean clone, then `just check`, the deterministic gate; `just --list`
names every recipe.
